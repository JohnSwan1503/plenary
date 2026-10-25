//! `stq-lite`: the stq carrier prototype rebuilt as a plenary plugin.
//!
//! Namespace `stq`, three kinds:
//!
//! - `#[stq::table(name = "users")] struct Users { .. }` — a table and its
//!   columns.
//! - `#[stq::query(sql = "SELECT .. FROM .. WHERE col = {param}")] struct Q { .. }`
//!   — a query whose fields are its parameters.
//! - `stq::sql!("SELECT ..")` inside a function body — an inline query,
//!   resolved through a marker to a `&'static str`.
//!
//! Local handlers check what one record can check on its own (placeholders
//! are fields, SQL parses) and create type-alias handles for every field so a
//! collection can talk about the field types by absolute path (R3).
//!
//! The global handler [`link`] matches queries to tables across every source
//! in the collection and produces the [`Resolved`] fact; [`render`] turns it
//! into `impl stq_lite::Query<__Collection>` consts, `InlineSql` impls for
//! local markers, and type obligations.
//!
//! `Resolved` is exportable (SPEC section 10): `link` exports it, so a
//! collection that lists this one as a stage starts from its tables and
//! queries (`&Imported<Resolved>`) and can resolve local queries against
//! tables it only knows through the stage.

mod sql;

use plenary_plugin::facts::{Decode, Encode, Fields as FactFields};
use plenary_plugin::{ExportFact, GlobalCx, Imported, LocalCx, Plugin, Record, Result, State};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::ParseStream;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct, LitStr};

/// The plugin, for `bundle!`.
pub fn plugin() -> Plugin {
    Plugin::new("stq", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        .attribute_kind("table", table)
        .attribute_kind("query", query)
        .call_kind("sql", inline_sql)
        .exportable::<Resolved>()
        .global(link)
        .global(render)
}

/// Everything `link` worked out. Other plugins (see `stats-plugin`) can read it
/// by taking `&Resolved` as a handler parameter; collections listing this one
/// as a stage get it as `&Imported<Resolved>`.
#[derive(Debug)]
pub struct Resolved {
    pub tables: Vec<Table>,
    pub queries: Vec<Query>,
    pub inline: Vec<Inline>,
    /// How many errors `link` reported. Lint-style warnings are skipped when
    /// this is non-zero, so they don't pile on top of errors. Not exported
    /// (a stage with errors never built).
    pub errors: usize,
}

impl plenary_plugin::Fact for Resolved {}

impl ExportFact for Resolved {
    const NAME: &'static str = "Resolved";

    fn to_tokens(&self) -> TokenStream {
        FactFields::new().with("tables", &self.tables).with("queries", &self.queries).with("inline", &self.inline).encode()
    }

    fn from_tokens(tokens: TokenStream) -> Result<Self> {
        let f = FactFields::decode(tokens)?;
        Ok(Resolved { tables: f.get("tables")?, queries: f.get("queries")?, inline: f.get("inline")?, errors: 0 })
    }
}

#[derive(Debug)]
pub struct Table {
    /// SQL name.
    pub name: String,
    /// Absolute path of the struct.
    pub path: syn::Path,
    pub columns: Vec<Column>,
    /// Span of the `name = "..."` literal.
    pub name_span: Span,
    /// Whether any query in the collection reads it.
    pub used: bool,
    /// Came from a stage this collection lists, not from a listed source.
    /// Such tables get no "never used" warning here.
    pub imported: bool,
}

impl Encode for Table {
    fn encode(&self, out: &mut TokenStream) {
        // The name travels as a literal with its span, so a duplicate-table
        // error downstream can still point at the upstream `name = ".."`.
        let name = LitStr::new(&self.name, self.name_span);
        FactFields::new().with("name", &name).with("path", &self.path).with("columns", &self.columns).with("used", &self.used).encode_into(out)
    }
}

impl Decode for Table {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let f = FactFields::parse(input)?;
        let name: LitStr = f.get("name")?;
        Ok(Table {
            name: name.value(),
            name_span: name.span(),
            path: f.get("path")?,
            columns: f.get("columns")?,
            used: f.get("used")?,
            imported: true,
        })
    }
}

#[derive(Debug)]
pub struct Column {
    pub name: String,
    /// Absolute path of the type alias the local handler created for the
    /// column's type.
    pub ty: syn::Path,
}

impl Encode for Column {
    fn encode(&self, out: &mut TokenStream) {
        FactFields::new().with("name", &self.name).with("ty", &self.ty).encode_into(out)
    }
}

impl Decode for Column {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let f = FactFields::parse(input)?;
        Ok(Column { name: f.get("name")?, ty: f.get("ty")? })
    }
}

#[derive(Debug)]
pub struct Query {
    /// Absolute path of the query struct.
    pub path: syn::Path,
    /// The rendered SQL.
    pub sql: String,
    /// Span of the `sql = "..."` literal.
    pub sql_span: Span,
    /// Type checks the collection must prove. Not exported: the collection
    /// that resolved the query proved them; an importing collection only
    /// re-emits the `Query` impl.
    pub checks: Vec<ParamCheck>,
}

impl Encode for Query {
    fn encode(&self, out: &mut TokenStream) {
        let sql = LitStr::new(&self.sql, self.sql_span);
        FactFields::new().with("path", &self.path).with("sql", &sql).encode_into(out)
    }
}

impl Decode for Query {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let f = FactFields::parse(input)?;
        let sql: LitStr = f.get("sql")?;
        Ok(Query { path: f.get("path")?, sql: sql.value(), sql_span: sql.span(), checks: Vec::new() })
    }
}

#[derive(Debug)]
pub struct ParamCheck {
    pub param: String,
    /// Absolute path of the parameter's type alias.
    pub param_ty: syn::Path,
    /// Absolute path of the compared column's type alias, when the parameter
    /// is compared with a column.
    pub column: Option<(String, syn::Path)>,
    /// Span of the parameter field's type.
    pub span: Span,
}

#[derive(Debug)]
pub struct Inline {
    /// Absolute path of the marker.
    pub marker: syn::Path,
    pub sql: String,
    pub span: Span,
    /// Only local markers get an impl (R4). Imported ones never are.
    pub local: bool,
}

impl Encode for Inline {
    fn encode(&self, out: &mut TokenStream) {
        let sql = LitStr::new(&self.sql, self.span);
        FactFields::new().with("marker", &self.marker).with("sql", &sql).encode_into(out)
    }
}

impl Decode for Inline {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let f = FactFields::parse(input)?;
        let sql: LitStr = f.get("sql")?;
        Ok(Inline { marker: f.get("marker")?, sql: sql.value(), span: sql.span(), local: false })
    }
}

// ───────────────────────────── local handlers ─────────────────────────────

fn str_arg(record: &Record, key: &str) -> Result<LitStr> {
    let mut out = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident(key) {
            out = Some(meta.value()?.parse::<LitStr>()?);
            Ok(())
        } else {
            Err(meta.error(format!("expected `{key} = \"...\"`")))
        }
    });
    record.parse_args_with(parser)?;
    out.ok_or_else(|| plenary_plugin::Error::new(record.kind().span(), format!("missing `{key} = \"...\"`")))
}

/// Named fields of a table or query struct, rejecting generics (the type
/// aliases below could not name their parameters).
fn named_fields(record: &Record) -> Result<(ItemStruct, Vec<syn::Field>)> {
    let item: ItemStruct = record.parse_item().map_err(|_| {
        plenary_plugin::Error::new(record.kind().span(), format!("`#[{}]` must be placed on a struct", record.kind()))
    })?;
    if !item.generics.params.is_empty() {
        return Err(plenary_plugin::Error::new(
            item.generics.span(),
            format!("`#[{}]` structs can't have generic parameters", record.kind()),
        ));
    }
    let fields = match &item.fields {
        Fields::Named(f) => f.named.iter().cloned().collect(),
        _ => {
            return Err(plenary_plugin::Error::new(
                item.ident.span(),
                format!("`#[{}]` structs must have named fields", record.kind()),
            ))
        }
    };
    Ok((item, fields))
}

/// Name of the type alias a local handler emits for a field, next to the
/// struct: `__pl_ty_<Struct>_<field>`.
pub fn alias_name(strukt: &syn::Ident, field: &syn::Ident) -> syn::Ident {
    format_ident!("__pl_ty_{}_{}", strukt, field)
}

/// Emit one `pub type` alias per field so the collection can name the field
/// types by absolute path. Tokens copied out of a record only mean something
/// where they were written (R3), and this alias is written there.
fn emit_aliases(cx: &mut LocalCx, item: &ItemStruct, fields: &[syn::Field]) {
    for f in fields {
        let name = alias_name(&item.ident, f.ident.as_ref().expect("named"));
        let ty = &f.ty;
        cx.add_item(quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types, dead_code, private_interfaces, unused_qualifications)]
            pub type #name = #ty;
        });
    }
}

fn table(record: &Record, cx: &mut LocalCx) -> Result<()> {
    let name = str_arg(record, "name")?;
    let (item, fields) = named_fields(record)?;
    cx.annotate("name", &name);
    emit_aliases(cx, &item, &fields);
    Ok(())
}

fn query(record: &Record, cx: &mut LocalCx) -> Result<()> {
    let lit = str_arg(record, "sql")?;
    let (item, fields) = named_fields(record)?;
    cx.annotate("sql", &lit);
    emit_aliases(cx, &item, &fields);
    // Local check: the SQL must parse and every `{param}` must be a field.
    let parsed = sql::parse(&lit.value()).map_err(|m| plenary_plugin::Error::new(lit.span(), m))?;
    for p in &parsed.params {
        if !fields.iter().any(|f| f.ident.as_ref().map(|i| i == p).unwrap_or(false)) {
            cx.error(lit.span(), format!("placeholder `{{{p}}}` is not a field of `{}`", item.ident));
        }
    }
    Ok(())
}

fn inline_sql(record: &Record, cx: &mut LocalCx) -> Result<()> {
    let lit: LitStr = record.parse_args().map_err(|_| {
        plenary_plugin::Error::new(record.kind().span(), "`stq::sql!` takes one string literal")
    })?;
    sql::parse(&lit.value()).map_err(|m| plenary_plugin::Error::new(lit.span(), m))?;
    let marker = cx.marker();
    cx.annotate("sql", &lit);
    cx.set_replacement(quote!(<#marker as ::stq_lite::InlineSql>::SQL));
    Ok(())
}

// ───────────────────────────── global handlers ─────────────────────────────

/// Match every query (and inline `sql!`) to the tables of the collection.
///
/// The catalog starts with the tables the listed stages exported (in
/// canonical order), so a local query can read a table this collection only
/// knows through a stage; imported queries and inline queries are carried
/// along so `render` gives them this collection's `Query` impls too. The
/// result is exported for the next stage.
///
/// Errors are reported through `cx` so that every broken query in a build is
/// reported at once; the fact still comes back with whatever resolved.
pub fn link(state: &State, upstream: &Imported<Resolved>, cx: &mut GlobalCx) -> Result<Resolved> {
    // Catalog of tables by SQL name, in canonical order: imported first.
    let mut tables: Vec<Table> = Vec::new();
    let mut table_origin: Vec<String> = Vec::new();
    let mut queries = Vec::new();
    let mut inline = Vec::new();
    for (stage, resolved) in upstream.iter() {
        for t in &resolved.tables {
            if let Some(prev) = tables.iter().position(|p| p.name == t.name) {
                cx.report(
                    plenary_plugin::Diagnostic::error(
                        state.sources()[stage.index()].path()[0].span(),
                        format!("table `{}` from stage `{}` is defined twice (also {})", t.name, stage.display_path(), table_origin[prev]),
                    )
                    .note(Some(tables[prev].name_span), "first defined here"),
                );
                continue;
            }
            tables.push(Table {
                name: t.name.clone(),
                path: t.path.clone(),
                columns: t.columns.iter().map(|c| Column { name: c.name.clone(), ty: c.ty.clone() }).collect(),
                name_span: t.name_span,
                used: t.used,
                imported: true,
            });
            table_origin.push(format!("in stage `{}`", stage.display_path()));
        }
        queries.extend(resolved.queries.iter().map(|q| Query { path: q.path.clone(), sql: q.sql.clone(), sql_span: q.sql_span, checks: Vec::new() }));
        inline.extend(resolved.inline.iter().map(|i| Inline { marker: i.marker.clone(), sql: i.sql.clone(), span: i.span, local: false }));
    }
    for record in state.records_of("stq", "table") {
        if record.is_tainted() {
            continue;
        }
        let Some(Ok(name)) = record.annotation_as::<LitStr>("name") else { continue };
        let item: ItemStruct = record.parse_item()?;
        if let Some(prev) = tables.iter().position(|t| t.name == name.value()) {
            cx.report(
                plenary_plugin::Diagnostic::error(
                    name.span(),
                    format!("table `{}` is defined twice (also {})", name.value(), table_origin[prev]),
                )
                .note(Some(tables[prev].name_span), "first defined here"),
            );
            continue;
        }
        let path = record.absolute_path();
        let columns = match &item.fields {
            Fields::Named(f) => f
                .named
                .iter()
                .map(|f| {
                    let ident = f.ident.as_ref().expect("named");
                    let alias = alias_name(&item.ident, ident);
                    let source = record.source_path();
                    let rel = record.relative_path();
                    let rel = &rel[..rel.len().saturating_sub(1)];
                    Column { name: ident.to_string(), ty: syn::parse2(quote!(#(#source)::* :: #(#rel ::)* #alias)).expect("path") }
                })
                .collect(),
            _ => Vec::new(),
        };
        tables.push(Table { name: name.value(), path, columns, name_span: name.span(), used: false, imported: false });
        table_origin.push(format!("as `{}`", record.display_path()));
    }
    let known_tables = |tables: &[Table]| tables.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join(", ");

    for record in state.records_of("stq", "query") {
        if record.is_tainted() {
            continue;
        }
        let Some(Ok(lit)) = record.annotation_as::<LitStr>("sql") else { continue };
        let item: ItemStruct = record.parse_item()?;
        let qname = &item.ident;
        let parsed = match sql::parse(&lit.value()) {
            Ok(p) => p,
            Err(m) => {
                cx.error(lit.span(), m);
                continue;
            }
        };
        let Some(ti) = tables.iter().position(|t| t.name == parsed.from) else {
            cx.error(
                lit.span(),
                format!(
                    "query `{qname}` references unknown table `{}` (tables in this collection: {})",
                    parsed.from,
                    known_tables(&tables)
                ),
            );
            continue;
        };
        tables[ti].used = true;
        let table = &tables[ti];
        let mut ok = true;
        for col in parsed.referenced_columns() {
            if !table.columns.iter().any(|c| c.name == col) {
                let have = table.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
                cx.error(
                    lit.span(),
                    format!("query `{qname}`: column `{col}` does not exist on table `{}` (columns: {have})", parsed.from),
                );
                ok = false;
            }
        }
        if !ok {
            continue;
        }
        let all_cols: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
        let rendered = parsed.render(&all_cols);

        // Parameter checks: every param type must be a `Param`, and a param
        // compared with a column must have the column's type.
        let source = record.source_path();
        let rel = record.relative_path();
        let rel = &rel[..rel.len().saturating_sub(1)];
        let mut checks = Vec::new();
        if let Fields::Named(f) = &item.fields {
            for field in &f.named {
                let ident = field.ident.as_ref().expect("named");
                let alias = alias_name(qname, ident);
                let param_ty: syn::Path = syn::parse2(quote!(#(#source)::* :: #(#rel ::)* #alias)).expect("path");
                let column = parsed
                    .comparisons
                    .iter()
                    .find(|(_, p)| p == &ident.to_string())
                    .and_then(|(c, _)| table.columns.iter().find(|col| &col.name == c))
                    .map(|col| (format!("{}.{}", table.name, col.name), col.ty.clone()));
                checks.push(ParamCheck { param: ident.to_string(), param_ty, column, span: field.ty.span() });
            }
        }
        queries.push(Query { path: record.absolute_path(), sql: rendered, sql_span: lit.span(), checks });
    }

    for record in state.records_of("stq", "sql") {
        if record.is_tainted() {
            continue;
        }
        let Some(Ok(lit)) = record.annotation_as::<LitStr>("sql") else { continue };
        let Some(marker) = record.marker_paths().into_iter().next() else { continue };
        let local = state.source_of(record).is_local();
        // On an error, a local marker still gets a placeholder impl so the
        // user sees one error, not a second one about a missing impl.
        let placeholder = |cx: &mut GlobalCx| {
            if local {
                cx.emit(quote! { impl ::stq_lite::InlineSql for #marker { const SQL: &'static str = ""; } });
            }
        };
        let parsed = match sql::parse(&lit.value()) {
            Ok(p) => p,
            Err(m) => {
                cx.error(lit.span(), m);
                placeholder(cx);
                continue;
            }
        };
        let Some(ti) = tables.iter().position(|t| t.name == parsed.from) else {
            cx.error(
                lit.span(),
                format!(
                    "inline query references unknown table `{}` (tables in this collection: {})",
                    parsed.from,
                    known_tables(&tables)
                ),
            );
            placeholder(cx);
            continue;
        };
        tables[ti].used = true;
        let table = &tables[ti];
        let mut ok = true;
        for col in parsed.referenced_columns() {
            if !table.columns.iter().any(|c| c.name == col) {
                let have = table.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
                cx.error(lit.span(), format!("inline query: column `{col}` does not exist on table `{}` (columns: {have})", parsed.from));
                ok = false;
            }
        }
        if !ok {
            placeholder(cx);
            continue;
        }
        let all_cols: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
        inline.push(Inline { marker, sql: parsed.render(&all_cols), span: lit.span(), local });
    }

    let errors = cx.diagnostics().iter().filter(|d| d.level() == plenary_plugin::Level::Error).count();
    let resolved = Resolved { tables, queries, inline, errors };
    cx.export(&resolved);
    Ok(resolved)
}

/// Emit the `Query<__Collection>` impls, the `InlineSql` impls for local
/// markers, the type obligations, and a warning for tables nothing reads.
pub fn render(resolved: &Resolved, cx: &mut GlobalCx) -> Result<()> {
    let collection = cx.collection().clone();
    for q in &resolved.queries {
        let path = &q.path;
        let lit = LitStr::new(&q.sql, q.sql_span);
        cx.emit(quote! {
            impl ::stq_lite::Query<#collection> for #path {
                const SQL: &'static str = #lit;
            }
        });
        for check in &q.checks {
            let param_ty = &check.param_ty;
            cx.require_impl(
                param_ty,
                quote!(::stq_lite::Param),
                check.span,
                &format!("parameter `{}` has type `{{Self}}`, which can't be sent to the database", check.param),
            );
            if let Some((column, column_ty)) = &check.column {
                cx.require_same_type(
                    column_ty,
                    param_ty,
                    check.span,
                    &format!("column `{column}` has type `{{Self}}`, but parameter `{}` has type `{{T}}`", check.param),
                );
            }
        }
    }
    for i in &resolved.inline {
        if !i.local {
            continue;
        }
        let marker = &i.marker;
        let lit = LitStr::new(&i.sql, i.span);
        cx.emit(quote! {
            impl ::stq_lite::InlineSql for #marker {
                const SQL: &'static str = #lit;
            }
        });
    }
    if resolved.errors == 0 {
        for t in &resolved.tables {
            if !t.used && !t.imported {
                cx.warn(t.name_span, format!("table `{}` is never used by a query in this collection", t.name));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use plenary_plugin::testing;
    use quote::ToTokens;

    /// The plugin's logic, tested on parsed records with no compiler involved.
    #[test]
    fn link_resolves_queries_across_sources() {
        let users = testing::collect(
            vec![plugin()],
            quote! {
                mod users {
                    #[stq::table(name = "users")]
                    pub struct Users { pub id: i64, pub email: String }
                }
            },
        );
        assert!(users.diagnostics.is_empty(), "{:?}", users.diagnostics);
        let orders = testing::collect(
            vec![plugin()],
            quote! {
                mod orders {
                    #[stq::query(sql = "SELECT * FROM users WHERE id = {user_id}")]
                    pub struct ByUser { pub user_id: i64 }
                    pub fn body() -> &'static str { stq::sql!("SELECT email FROM users") }
                }
            },
        );
        assert!(orders.diagnostics.is_empty(), "{:?}", orders.diagnostics);
        assert_eq!(orders.records.len(), 2);
        assert_eq!(orders.records[1].markers().len(), 1);

        let state = testing::state(vec![("crate::users", users.records), ("crate::orders", orders.records)]);
        let mut cx = testing::global_cx_with(vec![plugin()]);
        let resolved = link(&state, &testing::imported(), &mut cx).unwrap();
        assert!(!cx.diagnostics().has_errors());
        assert_eq!(cx.exports().len(), 1, "link exports Resolved");
        assert_eq!(resolved.tables.len(), 1);
        assert!(resolved.tables[0].used);
        assert_eq!(resolved.queries[0].sql, "SELECT id, email FROM users WHERE id = $1");
        assert_eq!(resolved.queries[0].path.to_token_stream().to_string(), "crate :: orders :: ByUser");
        let check = &resolved.queries[0].checks[0];
        assert_eq!(check.param, "user_id");
        assert_eq!(check.column.as_ref().unwrap().0, "users.id");
        assert_eq!(check.column.as_ref().unwrap().1.to_token_stream().to_string(), "crate :: users :: __pl_ty_Users_id");
        assert_eq!(resolved.inline[0].sql, "SELECT email FROM users");
        assert_eq!(resolved.inline[0].marker.to_token_stream().to_string(), "crate :: orders :: __pl_m0");
    }

    #[test]
    fn link_reports_misspelled_columns_without_failing() {
        let m = testing::collect(
            vec![plugin()],
            quote! {
                mod m {
                    #[stq::table(name = "users")]
                    pub struct Users { pub id: i64 }
                    #[stq::query(sql = "SELECT emial FROM users")]
                    pub struct Oops {}
                }
            },
        );
        let state = testing::state(vec![("crate::m", m.records)]);
        let mut cx = testing::global_cx_with(vec![plugin()]);
        let resolved = link(&state, &testing::imported(), &mut cx).unwrap();
        assert!(resolved.queries.is_empty());
        let errors: Vec<String> = cx.diagnostics().iter().map(|d| d.message().to_string()).collect();
        assert_eq!(errors, vec!["query `Oops`: column `emial` does not exist on table `users` (columns: id)"]);
    }

    #[test]
    fn local_placeholder_check_taints_the_record() {
        let m = testing::collect(
            vec![plugin()],
            quote! {
                mod m {
                    #[stq::query(sql = "SELECT id FROM users WHERE org_id = {org}")]
                    pub struct OrgUsers { pub org_id: i64 }
                }
            },
        );
        assert!(m.records[0].is_tainted());
        assert_eq!(m.diagnostics, vec!["error: placeholder `{org}` is not a field of `OrgUsers`"]);
    }

    #[test]
    fn a_query_resolves_against_a_table_imported_through_a_stage() {
        // What an upstream collection exported: one table, one query.
        let upstream = Resolved {
            tables: vec![Table {
                name: "accounts".into(),
                path: syn::parse_quote!(up::accounts::Accounts),
                columns: vec![
                    Column { name: "id".into(), ty: syn::parse_quote!(up::accounts::__pl_ty_Accounts_id) },
                    Column { name: "balance".into(), ty: syn::parse_quote!(up::accounts::__pl_ty_Accounts_balance) },
                ],
                name_span: Span::call_site(),
                used: true,
                imported: true,
            }],
            queries: vec![Query { path: syn::parse_quote!(up::accounts::ByEmail), sql: "SELECT id FROM accounts".into(), sql_span: Span::call_site(), checks: vec![] }],
            inline: vec![],
            errors: 0,
        };
        let local = testing::collect(
            vec![plugin()],
            quote! {
                mod staged {
                    #[stq::query(sql = "SELECT * FROM accounts WHERE balance > {floor}")]
                    pub struct Rich { pub floor: i64 }
                    #[stq::table(name = "accounts")]
                    pub struct Clash { pub id: i64 }
                }
            },
        );
        let state = testing::state(vec![("up::plan", vec![]), ("crate::staged", local.records)]);
        let mut cx = testing::global_cx_with(vec![plugin()]);
        let resolved = link(&state, &testing::imported_from(vec![("up::plan", upstream)]), &mut cx).unwrap();
        // The imported table is the catalog; the local query expands `*`
        // against its columns and compares `floor` with the imported column type.
        assert_eq!(resolved.tables.len(), 1);
        assert_eq!(resolved.queries.len(), 2, "the imported query is carried along");
        assert_eq!(resolved.queries[1].sql, "SELECT id, balance FROM accounts WHERE balance > $1");
        assert_eq!(resolved.queries[1].checks[0].column.as_ref().unwrap().1.to_token_stream().to_string(), "up :: accounts :: __pl_ty_Accounts_balance");
        // The local table of the same name is a duplicate citing the stage.
        let msgs: Vec<String> = cx.diagnostics().iter().map(|d| d.message().to_string()).collect();
        assert_eq!(msgs, vec!["table `accounts` is defined twice (also in stage `up::plan`)"]);
        // And the merged result is exported again.
        let (ns, name, tokens) = cx.exports().remove(0);
        assert_eq!((ns.as_str(), name.as_str()), ("stq", "Resolved"));
        let back = Resolved::from_tokens(tokens).unwrap();
        assert_eq!(back.tables.len(), 1);
        assert_eq!(back.queries.len(), 2);
        assert!(back.queries[1].checks.is_empty(), "obligations are not exported");
    }

    #[test]
    fn whole_pipeline_renders_impls() {
        let m = testing::collect(
            vec![plugin()],
            quote! {
                mod m {
                    #[stq::table(name = "users")]
                    pub struct Users { pub id: i64 }
                    #[stq::query(sql = "SELECT * FROM users")]
                    pub struct All {}
                }
            },
        );
        let out = testing::finish(vec![plugin()], vec![("crate::m", m.records)]);
        assert!(!out.has_errors(), "{:?}", out.errors);
        let text = out.output.to_string();
        assert!(text.contains("impl :: stq_lite :: Query < __Collection > for crate :: m :: All"), "{text}");
        assert!(text.contains("\"SELECT id FROM users\""), "{text}");
    }
}
