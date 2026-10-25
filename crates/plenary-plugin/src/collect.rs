//! The `collect` attribute: find records in a source, run local handlers,
//! re-emit the module and attach its carrier(s).

use crate::carrier::{encode_header, encode_record, SourceDesc};
use crate::diag::{span_key, Diagnostic, Diagnostics};
use crate::global::panic_message;
use crate::local::{LocalCx, LocalSourceInfo};
use crate::plugin::{Bundle, KindDef};
use crate::record::{Form, Kind, PluginVersion, Record};
use crate::util::{digest, respan, suggest};
use proc_macro2::{Delimiter, Group, Ident, Punct, Spacing, Span, TokenStream, TokenTree};
use quote::{format_ident, quote, quote_spanned, ToTokens};
use syn::visit_mut::VisitMut;
use syn::{Attribute, Item, ItemMod, Meta};

/// Run the collector on `#[collect(args)] item`.
pub fn collect_impl(bundle: &Bundle, args: TokenStream, item: TokenStream) -> TokenStream {
    collect_full(bundle, args, item).output
}

/// What the collector produced, for tests: the output plus the records and
/// diagnostics it holds in memory.
pub struct CollectOutput {
    pub output: TokenStream,
    pub records: Vec<Record>,
    pub diagnostics: Diagnostics,
}

/// [`collect_impl`] with the in-memory records and diagnostics.
pub fn collect_full(bundle: &Bundle, args: TokenStream, item: TokenStream) -> CollectOutput {
    let mut diagnostics = Diagnostics::new();
    let export = parse_args(args, &mut diagnostics);

    let module: ItemMod = match syn::parse2(item.clone()) {
        Ok(m) => m,
        Err(e) => {
            diagnostics.extend_from_error(e.into());
            let output = failed_output(bundle, item, export, diagnostics.clone());
            return CollectOutput { output, records: Vec::new(), diagnostics };
        }
    };
    let Some((_brace, _)) = &module.content else {
        diagnostics.error(
            module.ident.span(),
            "`collect` must be placed on an inline module (`mod name { ... }`); \
             an out-of-line `mod name;` does not expose its items to the attribute",
        );
        let mut output = diagnostics.to_tokens();
        output.extend(item);
        return CollectOutput { output, records: Vec::new(), diagnostics };
    };

    // Hidden names cover the module's location as well as its name and
    // tokens, so two exported sources with the same name and body in one
    // crate don't clash at the crate root (SPEC section 9, item 11).
    let hash = source_hash(&module.ident, &item);
    let mut collector = Collector::new(bundle, module.ident.clone(), export, hash);
    collector.diagnostics.extend(diagnostics);
    collector.run(module)
}

fn parse_args(args: TokenStream, diagnostics: &mut Diagnostics) -> bool {
    let mut export = false;
    let mut expect_comma = false;
    for tt in args {
        match (&tt, expect_comma) {
            (TokenTree::Ident(i), false) if i == "export" => {
                export = true;
                expect_comma = true;
            }
            (TokenTree::Punct(p), true) if p.as_char() == ',' => expect_comma = false,
            _ => {
                diagnostics.error(tt.span(), "unknown `collect` option; expected `export`");
                break;
            }
        }
    }
    export
}

/// The module could not be parsed: emit it unchanged (so the compiler reports
/// the syntax error too), the diagnostics, and a carrier marked `failed` so
/// collections skip the source instead of failing on a missing macro (R11).
fn failed_output(bundle: &Bundle, item: TokenStream, export: bool, diagnostics: Diagnostics) -> TokenStream {
    let tokens: Vec<TokenTree> = item.into_iter().collect();
    let name = tokens.iter().enumerate().find_map(|(i, tt)| match tt {
        TokenTree::Ident(id) if id == "mod" => match tokens.get(i + 1) {
            Some(TokenTree::Ident(name)) => Some(name.clone()),
            _ => None,
        },
        _ => None,
    });
    let body_pos = tokens.iter().rposition(|tt| matches!(tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace));
    let (Some(name), Some(body_pos)) = (name, body_pos) else {
        let mut out = diagnostics.to_tokens();
        out.extend(tokens);
        return out;
    };
    let hash = source_hash(&name, &tokens.iter().cloned().collect::<TokenStream>());
    let desc = SourceDesc { export, failed: true, plugins: plugin_versions(bundle), ..SourceDesc::default() };
    let carrier = carrier_items(&hash, export, &desc, &[], &[]);
    let mut out = TokenStream::new();
    for (i, tt) in tokens.into_iter().enumerate() {
        if i == body_pos {
            let TokenTree::Group(g) = tt else { unreachable!() };
            let mut body = g.stream();
            body.extend(diagnostics.to_tokens());
            body.extend(carrier.clone());
            let mut ng = Group::new(Delimiter::Brace, body);
            ng.set_span(g.span());
            out.extend(std::iter::once(TokenTree::Group(ng)));
        } else {
            out.extend(std::iter::once(tt));
        }
    }
    out
}

/// The hash in a source's hidden names: its module name, its tokens and
/// where the module is (`file:line` of the module's name inside the
/// compiler, see [`span_key`]).
pub(crate) fn source_hash(module_name: &Ident, tokens: &TokenStream) -> String {
    digest(&format!("{} {} {}", span_key(module_name.span()), module_name, tokens))
}

/// What a *round* produced (SPEC section 10): the items a collection emitted,
/// processed like a source's body (plugin attributes stripped, call-form
/// records replaced, markers planted), plus the records found in them.
pub(crate) struct RoundOutput {
    pub(crate) items: TokenStream,
    pub(crate) records: Vec<Record>,
    pub(crate) diagnostics: Diagnostics,
    /// The collector failed (R12 inside emitted items); the stage carries no
    /// records then.
    pub(crate) failed: bool,
}

/// Run the collector over the items a collection is about to emit, as if
/// they were the body of a source at the collection site. `export` is what
/// local handlers see as `cx.source().is_export()`.
pub(crate) fn collect_round(bundle: &Bundle, items: &[TokenStream], site: Span, export: bool) -> RoundOutput {
    let body: TokenStream = items.iter().cloned().collect();
    let module_name = Ident::new("__pl_stage", site);
    let module: ItemMod = match syn::parse2(quote!(mod #module_name { #body })) {
        Ok(m) => m,
        Err(e) => {
            // A plugin emitted tokens syn can't read as items. Emit them as
            // they are (rustc will say what is wrong with them) and report
            // that no records could be looked for.
            let mut diagnostics = Diagnostics::new();
            diagnostics.error(site, format!("the items this collection emits could not be parsed to look for records: {e}"));
            return RoundOutput { items: body, records: Vec::new(), diagnostics, failed: false };
        }
    };
    let (_, items) = module.content.expect("inline module");
    let mut collector = Collector::new(bundle, module_name, export, String::new());
    let processed = collector.run_body(items);
    RoundOutput { items: processed, records: collector.records, diagnostics: collector.diagnostics, failed: collector.failed }
}

pub(crate) fn plugin_versions(bundle: &Bundle) -> Vec<PluginVersion> {
    bundle
        .plugins
        .iter()
        .map(|p| PluginVersion { namespace: p.namespace.to_string(), name: p.name.to_string(), version: p.version.to_string() })
        .collect()
}

/// Where a record sits: nesting depth, module path and enclosing cfgs.
#[derive(Clone, Default)]
struct Scope {
    depth: usize,
    path: Vec<Ident>,
    cfg: Vec<TokenStream>,
}

impl Scope {
    fn condition(&self) -> Option<TokenStream> {
        combine_cfg(&self.cfg)
    }
}

/// Combine cfg predicates with `all(...)`.
fn combine_cfg(preds: &[TokenStream]) -> Option<TokenStream> {
    match preds.len() {
        0 => None,
        1 => Some(preds[0].clone()),
        _ => Some(quote!(all(#(#preds),*))),
    }
}

struct Collector<'b> {
    bundle: &'b Bundle,
    source: LocalSourceInfo,
    hash: String,
    next_seq: u32,
    next_marker: usize,
    records: Vec<Record>,
    markers: Vec<(Ident, Option<TokenStream>)>,
    diagnostics: Diagnostics,
    failed: bool,
}

impl<'b> Collector<'b> {
    fn new(bundle: &'b Bundle, module_name: Ident, export: bool, hash: String) -> Self {
        Collector {
            bundle,
            source: LocalSourceInfo { module_name, export },
            hash,
            next_seq: 0,
            next_marker: 0,
            records: Vec::new(),
            markers: Vec::new(),
            diagnostics: Diagnostics::new(),
            failed: false,
        }
    }

    fn run(mut self, mut module: ItemMod) -> CollectOutput {
        let (brace, items) = module.content.take().expect("inline module");
        let mut body = self.run_body(items);
        body.extend(self.diagnostics.to_tokens());

        let desc = SourceDesc { export: self.source.export, failed: self.failed, plugins: plugin_versions(self.bundle), ..SourceDesc::default() };
        let (unconditional, conditional): (Vec<&Record>, Vec<&Record>) =
            self.records.iter().partition(|r| r.condition.is_none());
        let (unconditional, conditional) = if self.failed { (Vec::new(), Vec::new()) } else { (unconditional, conditional) };
        body.extend(carrier_items(&self.hash, self.source.export, &desc, &unconditional, &conditional));

        module.content = Some((brace, Vec::new()));
        let mut out = TokenStream::new();
        for attr in &module.attrs {
            attr.to_tokens(&mut out);
        }
        module.vis.to_tokens(&mut out);
        module.unsafety.to_tokens(&mut out);
        module.mod_token.to_tokens(&mut out);
        module.ident.to_tokens(&mut out);
        let mut g = Group::new(Delimiter::Brace, body);
        g.set_span(brace.span.join());
        out.extend(std::iter::once(TokenTree::Group(g)));
        CollectOutput { output: out, records: self.records, diagnostics: self.diagnostics }
    }

    /// Process the items of a source: find records, run local handlers, and
    /// return the processed items followed by the marker types they asked
    /// for. Records and diagnostics stay on `self`.
    fn run_body(&mut self, items: Vec<Item>) -> TokenStream {
        let items = self.process_items(items, &Scope::default());
        let mut body = TokenStream::new();
        for item in &items {
            item.to_tokens(&mut body);
        }
        for (name, cond) in &self.markers {
            let cfg = cond.as_ref().map(|c| quote!(#[cfg(#c)]));
            body.extend(quote! {
                #cfg
                #[doc(hidden)]
                #[allow(non_camel_case_types, dead_code)]
                pub struct #name;
            });
        }
        body
    }

    fn process_items(&mut self, items: Vec<Item>, scope: &Scope) -> Vec<Item> {
        let mut out = Vec::with_capacity(items.len());
        for mut item in items {
            // R12: no nested sources.
            if let Item::Mod(m) = &mut item {
                if let Some(pos) = m.attrs.iter().position(|a| is_collect_attr(a, self.bundle)) {
                    let attr = m.attrs.remove(pos);
                    self.diagnostics.error(
                        attr.path().span_of_last(),
                        format!(
                            "a source can't contain another source (R12): remove this `collect` from `{}` or move the module out of `{}`",
                            m.ident, self.source.module_name
                        ),
                    );
                    self.failed = true;
                }
            }

            let mut item_scope = scope.clone();
            if let Some(attrs) = item_attrs_mut(&mut item) {
                self.reject_cfg_attrs(attrs);
                let mut cfgs = Vec::new();
                self.scan_cfg(attrs, &mut cfgs);
                item_scope.cfg.extend(cfgs);
            }

            // Attribute-form records on this item.
            let plugin_attrs = match item_attrs_mut(&mut item) {
                Some(attrs) if attrs.iter().any(|a| self.is_plugin_attr(a)) => {
                    let snapshot = item.to_token_stream();
                    let attrs = item_attrs_mut(&mut item).expect("has attrs");
                    let taken = take_plugin_attrs(attrs, self.bundle);
                    Some((snapshot, taken))
                }
                _ => None,
            };

            let mut replacement: Option<TokenStream> = None;
            let mut additions: Vec<TokenStream> = Vec::new();
            if let Some((snapshot, attrs)) = plugin_attrs {
                let mut relative_path = scope.path.clone();
                if let Some(ident) = item_ident(&item) {
                    relative_path.push(ident.clone());
                }
                for attr in attrs {
                    let Some((kind, def)) = self.resolve_kind(attr.path(), Form::Attribute) else { continue };
                    let args = attr_args(&attr);
                    let record = Record::new(
                        kind,
                        Form::Attribute,
                        args,
                        Some(snapshot.clone()),
                        relative_path.clone(),
                        self.next_seq(),
                        item_scope.condition(),
                    );
                    let outcome = self.run_local(record, def, Form::Attribute, item_scope.depth);
                    if let Some(r) = outcome.replacement_item {
                        if replacement.is_some() {
                            self.diagnostics.error(
                                attr.path().span_of_last(),
                                "two local handlers want to replace this item; only one plugin attribute may replace an item",
                            );
                        }
                        replacement = Some(r);
                    }
                    additions.extend(outcome.added_items);
                    self.diagnostics.extend(outcome.diagnostics);
                    self.records.push(outcome.record);
                }
            }

            if replacement.is_none() {
                // Sub-attributes, call-form records and nested items.
                let enclosing: Vec<Ident> = item_ident(&item).cloned().into_iter().collect();
                let mut visitor = ItemVisitor { collector: self, scope: item_scope.clone(), enclosing, additions: Vec::new() };
                match &mut item {
                    Item::Macro(m) if visitor.collector.is_plugin_path(&m.mac.path) => {
                        let mac = m.mac.clone();
                        let r = visitor.handle_call(&mac, Position::Item);
                        item = Item::Verbatim(r);
                    }
                    Item::Mod(_) => {}
                    other => syn::visit_mut::visit_item_mut(&mut visitor, other),
                }
                additions.extend(visitor.additions);

                if let Item::Mod(m) = &mut item {
                    if let Some((_, content)) = &mut m.content {
                        let mut inner = item_scope.clone();
                        inner.depth += 1;
                        inner.path.push(m.ident.clone());
                        let inner_items = self.process_items(std::mem::take(content), &inner);
                        *content = inner_items;
                    }
                }
            }

            match replacement {
                Some(r) => out.push(Item::Verbatim(r)),
                None => out.push(item),
            }
            for a in additions {
                out.push(Item::Verbatim(a));
            }
        }
        out
    }

    fn next_seq(&mut self) -> u32 {
        let s = self.next_seq;
        self.next_seq += 1;
        s
    }

    fn is_plugin_attr(&self, attr: &Attribute) -> bool {
        self.is_plugin_path(attr.path())
    }

    fn is_plugin_path(&self, path: &syn::Path) -> bool {
        path.leading_colon.is_none()
            && path.segments.len() == 2
            && self.bundle.knows_namespace(&path.segments[0].ident.to_string())
    }

    /// Resolve `ns::kind` to its definition, reporting unknown kinds (R9) and
    /// form mismatches.
    fn resolve_kind(&mut self, path: &syn::Path, form: Form) -> Option<(Kind, &'b KindDef)> {
        let ns = path.segments[0].ident.clone();
        let kind = path.segments[1].ident.clone();
        let bundle: &'b Bundle = self.bundle;
        let plugin = bundle.plugin_for(&ns.to_string())?;
        match plugin.kind(&kind.to_string()) {
            None => {
                let mut msg = format!("unknown kind `{kind}` in namespace `{ns}`");
                if let Some(s) = suggest(&kind.to_string(), plugin.kind_names()) {
                    msg.push_str(&format!("; did you mean `{s}`?"));
                }
                let known: Vec<_> = plugin.kind_names().collect();
                msg.push_str(&format!(" (kinds of `{ns}`: {})", known.join(", ")));
                self.diagnostics.error(kind.span(), msg);
                None
            }
            Some(def) if def.form != form => {
                let hint = match def.form {
                    Form::Attribute => format!("write it as an attribute: `#[{ns}::{kind}(...)]`"),
                    Form::Call => format!("write it as a call: `{ns}::{kind}!(...)`"),
                };
                self.diagnostics.error(kind.span(), format!("`{ns}::{kind}` is an {} kind; {hint}", def.form));
                None
            }
            Some(def) => Some((Kind::new(ns, kind), def)),
        }
    }

    /// Run a local handler with panic capture and apply its outcome.
    fn run_local(&mut self, mut record: Record, def: &KindDef, form: Form, depth: usize) -> LocalOutcome {
        let bundle: &'b Bundle = self.bundle;
        let plugin = bundle.plugin_for(&record.kind.ns.to_string()).expect("known namespace");
        let mut cx = LocalCx::new(&self.source, form, depth, &mut self.next_marker);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (def.local)(&record, &mut cx)));
        let mut diagnostics = std::mem::take(&mut cx.diagnostics);
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => diagnostics.extend_from_error(e),
            Err(payload) => diagnostics.error(
                record.kind.span(),
                format!(
                    "plugin `{}` panicked in the local handler for `{}`: {}",
                    plugin.name,
                    record.kind,
                    panic_message(payload.as_ref())
                ),
            ),
        }
        record.tainted = diagnostics.has_errors();
        record.annotations = std::mem::take(&mut cx.annotations);
        record.markers = cx.markers.clone();
        let cond = record.condition.clone();
        for m in &cx.markers {
            self.markers.push((m.clone(), cond.clone()));
        }
        let outcome = LocalOutcome {
            replacement_item: cx.replacement_item.take(),
            added_items: std::mem::take(&mut cx.added_items),
            call_replacement: cx.call_replacement.take(),
            keep_spans: cx.keep_spans,
            diagnostics,
            record,
        };
        drop(cx);
        outcome
    }

    /// Collect `#[cfg(..)]` predicates.
    fn scan_cfg(&mut self, attrs: &[Attribute], out: &mut Vec<TokenStream>) {
        for attr in attrs {
            if attr.path().is_ident("cfg") {
                if let Meta::List(l) = &attr.meta {
                    out.push(l.tokens.clone());
                }
            }
        }
    }

    /// Reject (and remove) `cfg_attr` wrapping a cfg or a plugin attribute
    /// (R7, v0). Removing it avoids a second error from the compiler trying
    /// to resolve the wrapped plugin attribute.
    fn reject_cfg_attrs(&mut self, attrs: &mut Vec<Attribute>) {
        let mut rejected = Vec::new();
        for (i, attr) in attrs.iter().enumerate() {
            if !attr.path().is_ident("cfg_attr") {
                continue;
            }
            let Meta::List(l) = &attr.meta else { continue };
            let parsed = l.parse_args_with(syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated);
            let Ok(metas) = parsed else { continue };
            for inner in metas.iter().skip(1) {
                let path = inner.path();
                if path.is_ident("cfg") || self.is_plugin_path(path) {
                    self.diagnostics.error(
                        attr.path().span_of_last(),
                        format!(
                            "`cfg_attr` wrapping `{}` isn't supported in v0 (R7); write the attribute directly with its own `#[cfg(..)]`",
                            crate::util::path_text(path)
                        ),
                    );
                    rejected.push(i);
                    break;
                }
            }
        }
        for i in rejected.into_iter().rev() {
            attrs.remove(i);
        }
    }
}

struct LocalOutcome {
    replacement_item: Option<TokenStream>,
    added_items: Vec<TokenStream>,
    call_replacement: Option<TokenStream>,
    keep_spans: bool,
    diagnostics: Diagnostics,
    record: Record,
}

/// Where a call-form record was found.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    Item,
    Statement,
    Expression,
    Type,
    Pattern,
}

/// Walks one module-level item: strips plugin sub-attributes, tracks cfgs on
/// nested nodes, and turns `ns::kind!(..)` calls into call-form records.
struct ItemVisitor<'c, 'b> {
    collector: &'c mut Collector<'b>,
    scope: Scope,
    enclosing: Vec<Ident>,
    additions: Vec<TokenStream>,
}

impl ItemVisitor<'_, '_> {
    /// Handle a `ns::kind!(..)` call. Returns the tokens that replace it.
    fn handle_call(&mut self, mac: &syn::Macro, position: Position) -> TokenStream {
        let Some((kind, def)) = self.collector.resolve_kind(&mac.path, Form::Call) else {
            return TokenStream::new();
        };
        let mut relative_path = self.scope.path.clone();
        relative_path.extend(self.enclosing.iter().cloned());
        let kind_span = kind.span();
        let record = Record::new(
            kind,
            Form::Call,
            mac.tokens.clone(),
            None,
            relative_path,
            self.collector.next_seq(),
            self.scope.condition(),
        );
        let mut outcome = self.collector.run_local(record, def, Form::Call, self.scope.depth);
        self.additions.append(&mut outcome.added_items);
        let replacement = match outcome.call_replacement.take() {
            // The replacement gets the span of the call's kind ident, so that
            // compiler errors about the expansion point at the user's call.
            Some(r) => if outcome.keep_spans { r } else { respan(r, kind_span) },
            None => {
                let needs_value = matches!(position, Position::Expression | Position::Type | Position::Pattern);
                if needs_value {
                    let first = outcome.diagnostics.iter().find(|d| d.level() == crate::Level::Error).cloned();
                    if let Some(first) = first {
                        // Put the first error where the value should be, so
                        // the compiler reports nothing extra there.
                        let msg = first.rendered_message();
                        let span = first.span();
                        let rest: Vec<Diagnostic> = outcome
                            .diagnostics
                            .iter()
                            .filter(|d| d.message() != first.message())
                            .cloned()
                            .collect();
                        let mut kept = Diagnostics::new();
                        for d in rest {
                            kept.push(d);
                        }
                        outcome.diagnostics = kept;
                        quote_spanned!(span=> ::core::compile_error! { #msg })
                    } else {
                        let kind = outcome.record.kind.to_string();
                        let msg = format!(
                            "`{kind}!` is used where a value is needed, but its local handler set no replacement"
                        );
                        quote_spanned!(kind_span=> ::core::compile_error! { #msg })
                    }
                } else {
                    TokenStream::new()
                }
            }
        };
        self.collector.diagnostics.extend(outcome.diagnostics);
        self.collector.records.push(outcome.record);
        replacement
    }

    fn with_cfg<F: FnOnce(&mut Self)>(&mut self, attrs: &[Attribute], f: F) {
        let mut cfgs = Vec::new();
        self.collector.scan_cfg(attrs, &mut cfgs);
        let n = cfgs.len();
        self.scope.cfg.extend(cfgs);
        f(self);
        let keep = self.scope.cfg.len() - n;
        self.scope.cfg.truncate(keep);
    }

    /// Reject `cfg_attr` wrapping cfg/plugin attributes and strip plugin
    /// sub-attributes (they stay on the record's item tokens).
    fn strip(&mut self, attrs: &mut Vec<Attribute>) {
        self.collector.reject_cfg_attrs(attrs);
        attrs.retain(|a| !self.collector.is_plugin_attr(a));
    }
}

impl VisitMut for ItemVisitor<'_, '_> {
    // Items nested in bodies: plugin attributes are not records there.
    fn visit_item_mut(&mut self, item: &mut Item) {
        match item {
            Item::Mod(_) => {}
            Item::Macro(m) if self.collector.is_plugin_path(&m.mac.path) => {
                let mac = m.mac.clone();
                let r = self.handle_call(&mac, Position::Item);
                *item = Item::Verbatim(r);
            }
            other => {
                if let Some(attrs) = item_attrs_mut(other) {
                    for a in attrs.iter() {
                        if self.collector.is_plugin_attr(a) {
                            self.collector.diagnostics.error(
                                a.path().span_of_last(),
                                "attribute-form records must be module-level items; this item is inside a body",
                            );
                        }
                    }
                    self.strip(attrs);
                }
                let attrs = item_attrs(other).cloned().unwrap_or_default();
                let pushed = item_ident(other).cloned();
                if let Some(id) = pushed.clone() {
                    self.enclosing.push(id);
                }
                self.with_cfg(&attrs, |this| syn::visit_mut::visit_item_mut(this, other));
                if pushed.is_some() {
                    self.enclosing.pop();
                }
            }
        }
    }

    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        if let syn::Expr::Macro(m) = expr {
            if self.collector.is_plugin_path(&m.mac.path) {
                let mac = m.mac.clone();
                let r = self.handle_call(&mac, Position::Expression);
                *expr = syn::Expr::Verbatim(r);
                return;
            }
        }
        syn::visit_mut::visit_expr_mut(self, expr);
    }

    fn visit_stmt_mut(&mut self, stmt: &mut syn::Stmt) {
        match stmt {
            syn::Stmt::Macro(m) if self.collector.is_plugin_path(&m.mac.path) => {
                let attrs = m.attrs.clone();
                let mac = m.mac.clone();
                let semi = m.semi_token;
                let position = if semi.is_some() { Position::Statement } else { Position::Expression };
                let mut r = TokenStream::new();
                self.with_cfg(&attrs, |this| r = this.handle_call(&mac, position));
                *stmt = if r.is_empty() {
                    syn::Stmt::Item(Item::Verbatim(TokenStream::new()))
                } else {
                    syn::Stmt::Expr(syn::Expr::Verbatim(r), semi)
                };
            }
            syn::Stmt::Local(l) => {
                let attrs = l.attrs.clone();
                self.strip(&mut l.attrs);
                self.with_cfg(&attrs, |this| syn::visit_mut::visit_stmt_mut(this, stmt));
            }
            _ => syn::visit_mut::visit_stmt_mut(self, stmt),
        }
    }

    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        if let syn::Type::Macro(m) = ty {
            if self.collector.is_plugin_path(&m.mac.path) {
                let mac = m.mac.clone();
                let r = self.handle_call(&mac, Position::Type);
                *ty = syn::Type::Verbatim(r);
                return;
            }
        }
        syn::visit_mut::visit_type_mut(self, ty);
    }

    fn visit_pat_mut(&mut self, pat: &mut syn::Pat) {
        if let syn::Pat::Macro(m) = pat {
            if self.collector.is_plugin_path(&m.mac.path) {
                let mac = m.mac.clone();
                let r = self.handle_call(&mac, Position::Pattern);
                *pat = syn::Pat::Verbatim(r);
                return;
            }
        }
        syn::visit_mut::visit_pat_mut(self, pat);
    }

    fn visit_field_mut(&mut self, f: &mut syn::Field) {
        self.strip(&mut f.attrs);
        syn::visit_mut::visit_field_mut(self, f);
    }

    fn visit_variant_mut(&mut self, v: &mut syn::Variant) {
        self.strip(&mut v.attrs);
        syn::visit_mut::visit_variant_mut(self, v);
    }

    fn visit_pat_type_mut(&mut self, p: &mut syn::PatType) {
        self.strip(&mut p.attrs);
        syn::visit_mut::visit_pat_type_mut(self, p);
    }

    fn visit_receiver_mut(&mut self, r: &mut syn::Receiver) {
        self.strip(&mut r.attrs);
        syn::visit_mut::visit_receiver_mut(self, r);
    }

    fn visit_impl_item_mut(&mut self, i: &mut syn::ImplItem) {
        let attrs = match i {
            syn::ImplItem::Const(c) => Some(&mut c.attrs),
            syn::ImplItem::Fn(f) => Some(&mut f.attrs),
            syn::ImplItem::Type(t) => Some(&mut t.attrs),
            syn::ImplItem::Macro(m) => Some(&mut m.attrs),
            _ => None,
        };
        let mut cfg_attrs = Vec::new();
        if let Some(attrs) = attrs {
            cfg_attrs = attrs.clone();
            self.strip(attrs);
        }
        let name = match i {
            syn::ImplItem::Fn(f) => Some(f.sig.ident.clone()),
            syn::ImplItem::Const(c) => Some(c.ident.clone()),
            _ => None,
        };
        if let Some(n) = name.clone() {
            self.enclosing.push(n);
        }
        self.with_cfg(&cfg_attrs, |this| syn::visit_mut::visit_impl_item_mut(this, i));
        if name.is_some() {
            self.enclosing.pop();
        }
    }

    fn visit_trait_item_mut(&mut self, i: &mut syn::TraitItem) {
        let attrs = match i {
            syn::TraitItem::Const(c) => Some(&mut c.attrs),
            syn::TraitItem::Fn(f) => Some(&mut f.attrs),
            syn::TraitItem::Type(t) => Some(&mut t.attrs),
            syn::TraitItem::Macro(m) => Some(&mut m.attrs),
            _ => None,
        };
        let mut cfg_attrs = Vec::new();
        if let Some(attrs) = attrs {
            cfg_attrs = attrs.clone();
            self.strip(attrs);
        }
        self.with_cfg(&cfg_attrs, |this| syn::visit_mut::visit_trait_item_mut(this, i));
    }

    fn visit_arm_mut(&mut self, a: &mut syn::Arm) {
        let attrs = a.attrs.clone();
        self.strip(&mut a.attrs);
        self.with_cfg(&attrs, |this| syn::visit_mut::visit_arm_mut(this, a));
    }

    fn visit_generic_param_mut(&mut self, g: &mut syn::GenericParam) {
        match g {
            syn::GenericParam::Type(t) => self.strip(&mut t.attrs),
            syn::GenericParam::Lifetime(l) => self.strip(&mut l.attrs),
            syn::GenericParam::Const(c) => self.strip(&mut c.attrs),
        }
        syn::visit_mut::visit_generic_param_mut(self, g);
    }
}

/// `true` for the bundle's own `collect` attribute (`#[demo_macros::collect]`
/// or an imported `#[collect]`).
fn is_collect_attr(attr: &Attribute, bundle: &Bundle) -> bool {
    let path = attr.path();
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    match segs.as_slice() {
        [single] => single == "collect",
        [first, last] => last == "collect" && *first == bundle.crate_name,
        _ => false,
    }
}

fn take_plugin_attrs(attrs: &mut Vec<Attribute>, bundle: &Bundle) -> Vec<Attribute> {
    let mut taken = Vec::new();
    attrs.retain(|a| {
        let p = a.path();
        let is_plugin = p.leading_colon.is_none()
            && p.segments.len() == 2
            && bundle.knows_namespace(&p.segments[0].ident.to_string());
        if is_plugin {
            taken.push(a.clone());
        }
        !is_plugin
    });
    taken
}

fn attr_args(attr: &Attribute) -> TokenStream {
    match &attr.meta {
        Meta::Path(_) => TokenStream::new(),
        Meta::List(l) => l.tokens.clone(),
        Meta::NameValue(nv) => nv.value.to_token_stream(),
    }
}

fn item_attrs(item: &Item) -> Option<&Vec<Attribute>> {
    Some(match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => return None,
    })
}

fn item_attrs_mut(item: &mut Item) -> Option<&mut Vec<Attribute>> {
    Some(match item {
        Item::Const(i) => &mut i.attrs,
        Item::Enum(i) => &mut i.attrs,
        Item::ExternCrate(i) => &mut i.attrs,
        Item::Fn(i) => &mut i.attrs,
        Item::ForeignMod(i) => &mut i.attrs,
        Item::Impl(i) => &mut i.attrs,
        Item::Macro(i) => &mut i.attrs,
        Item::Mod(i) => &mut i.attrs,
        Item::Static(i) => &mut i.attrs,
        Item::Struct(i) => &mut i.attrs,
        Item::Trait(i) => &mut i.attrs,
        Item::TraitAlias(i) => &mut i.attrs,
        Item::Type(i) => &mut i.attrs,
        Item::Union(i) => &mut i.attrs,
        Item::Use(i) => &mut i.attrs,
        _ => return None,
    })
}

fn item_ident(item: &Item) -> Option<&Ident> {
    Some(match item {
        Item::Const(i) => &i.ident,
        Item::Enum(i) => &i.ident,
        Item::ExternCrate(i) => &i.ident,
        Item::Fn(i) => &i.sig.ident,
        Item::Macro(i) => i.ident.as_ref()?,
        Item::Mod(i) => &i.ident,
        Item::Static(i) => &i.ident,
        Item::Struct(i) => &i.ident,
        Item::Trait(i) => &i.ident,
        Item::TraitAlias(i) => &i.ident,
        Item::Type(i) => &i.ident,
        Item::Union(i) => &i.ident,
        _ => return None,
    })
}

trait SpanOfLast {
    fn span_of_last(&self) -> Span;
}

impl SpanOfLast for syn::Path {
    fn span_of_last(&self) -> Span {
        self.segments.last().map(|s| s.ident.span()).unwrap_or_else(Span::call_site)
    }
}

/// The carrier `macro_rules!` items for a source: the main carrier plus one
/// pair of per-record carriers for each conditional record (R7).
///
/// This is the engine's half of the protocol documented on
/// `plenary::expand!`. A carrier receives
///
/// ```text
/// <callback> [ <prefix> ] { <accumulator> } [ <own path> ] <rest of the list>
/// ```
///
/// and, once its source block is complete (after the per-record carriers of
/// its conditional records), either chains to the next listed source's
/// carrier (when the rest begins with `path ,`) or, when the rest is empty
/// or a lone trailing comma, calls the callback with the full accumulator.
/// Chaining directly costs one expansion level per source instead of two.
pub(crate) fn carrier_items(hash: &str, export: bool, desc: &SourceDesc, unconditional: &[&Record], conditional: &[&Record]) -> TokenStream {
    let header = encode_header(desc);
    let recs = unconditional.iter().map(|r| encode_record(r));
    let initial = quote!(#header #(#recs)*);
    let conditional: Vec<(TokenStream, TokenStream, u32)> =
        conditional.iter().map(|r| (r.condition.clone().expect("conditional"), encode_record(r), r.sequence)).collect();
    carrier_macros(hash, export, initial, &conditional)
}

/// The carrier chain for one source block whose unconditional part is
/// `initial`, with one `#[cfg]` carrier pair per `(condition, record tokens,
/// sequence)` in `conditional`. The macros are named `__pl_carrier_<hash>`
/// and `__pl_rec_<seq>_<hash>`, with the aliases `__pl_carrier` and
/// `__pl_rec_<seq>`.
pub(crate) fn carrier_macros(hash: &str, export: bool, initial: TokenStream, conditional: &[(TokenStream, TokenStream, u32)]) -> TokenStream {
    let dollar = Punct::new('$', Spacing::Alone);
    let d = &dollar;
    let export_attr = export.then(|| quote!(#[macro_export]));
    let alias_vis = if export { quote!(pub) } else { quote!(pub(crate)) };
    let carrier_name = format_ident!("__pl_carrier_{}", hash);
    let rec_name = |seq: u32| format_ident!("__pl_rec_{}_{}", seq, hash);
    let rec_alias = |seq: u32| format_ident!("__pl_rec_{}", seq);

    // The common start of every matcher.
    let head = quote! {
        #d cb:path
        [ #d ( #d pre:tt )* ]
        { #d ( #d acc:tt )* }
        [ #d ( #d seg:tt )* ]
    };
    // The two ways a complete source block leaves this source: on to the next
    // listed source's carrier, or back to the callback (`__finish`).
    let chain_on = |src: &TokenStream| {
        quote! {
            #d ( #d next )::+ :: __pl_carrier ! {
                #d cb [ #d ( #d pre )* ] { #d ( #d acc )* source [ #d ( #d seg )* ] { #src } } [ #d ( #d next )::+ ] #d ( #d rest )*
            }
        }
    };
    let finish = |src: &TokenStream| {
        quote! {
            #d cb ! {
                #d ( #d pre )*
                { #d ( #d acc )* source [ #d ( #d seg )* ] { #src } }
            }
        }
    };
    let final_rules = |matcher_mid: &TokenStream, src: &TokenStream| {
        let on = chain_on(src);
        let fin = finish(src);
        quote! {
            ( #head #matcher_mid #d ( #d next:ident )::+ , #d ( #d rest:tt )* ) => { #on };
            ( #head #matcher_mid #d ( , )? ) => { #fin };
        }
    };
    // Hand a partial block to the next per-record carrier of this source.
    let chain_rec = |next: &Ident, src: &TokenStream| {
        quote! {
            #d ( #d seg )* :: #next ! {
                #d cb [ #d ( #d pre )* ] { #d ( #d acc )* } [ #d ( #d seg )* ] { #src } #d ( #d rest )*
            }
        }
    };

    let nothing = TokenStream::new();
    let main_rules = match conditional.first() {
        None => final_rules(&nothing, &initial),
        Some((_, _, first)) => {
            let body = chain_rec(&rec_alias(*first), &initial);
            quote! { ( #head #d ( #d rest:tt )* ) => { #body }; }
        }
    };
    let mut out = quote! {
        #[doc(hidden)]
        #[allow(unused_macros)]
        #export_attr
        macro_rules! #carrier_name { #main_rules }
        #[doc(hidden)]
        #[allow(unused_imports)]
        #alias_vis use #carrier_name as __pl_carrier;
    };

    let partial = quote!({ #d ( #d src:tt )* });
    for (i, (cond, encoded, seq)) in conditional.iter().enumerate() {
        let name = rec_name(*seq);
        let alias = rec_alias(*seq);
        let next = conditional.get(i + 1).map(|(_, _, n)| rec_alias(*n));
        let rules = |with_record: bool| {
            let src = if with_record { quote!(#d ( #d src )* #encoded) } else { quote!(#d ( #d src )*) };
            match &next {
                Some(n) => {
                    let body = chain_rec(n, &src);
                    quote! { ( #head #partial #d ( #d rest:tt )* ) => { #body }; }
                }
                None => final_rules(&partial, &src),
            }
        };
        let live = rules(true);
        let dead = rules(false);
        out.extend(quote! {
            #[cfg(#cond)]
            #[doc(hidden)]
            #[allow(unused_macros)]
            #export_attr
            macro_rules! #name { #live }
            #[cfg(not(#cond))]
            #[doc(hidden)]
            #[allow(unused_macros)]
            #export_attr
            macro_rules! #name { #dead }
            #[doc(hidden)]
            #[allow(unused_imports)]
            #alias_vis use #name as #alias;
        });
    }
    out
}
