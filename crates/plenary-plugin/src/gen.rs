//! Code generation helpers for global handlers (SPEC section 11).
//!
//! [`ClosedEnum`] turns an *open* set of declarations (records found across
//! a whole program) into a *closed* type: an enum with one variant per
//! entry, dense discriminants in canonical order, and the lookups a closed
//! set affords (`ALL`, `name`, `from_name`, `id`, `TryFrom<u16>`). Only a
//! whole-program pass can mint such a type; run-time registries (linkme,
//! inventory, ...) cannot, because nothing can enumerate them at compile time.

use crate::diag::Diagnostic;
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::{quote, quote_spanned, ToTokens};

/// One entry of the set: the name the set knows it by and where it was
/// declared.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    span: Span,
}

/// A per-variant constant accessor: `pub const fn <name>(self) -> <ty>`.
#[derive(Clone, Debug)]
struct Payload {
    name: Ident,
    ty: TokenStream,
    values: Vec<TokenStream>,
}

/// Builder for a closed enum over an ordered set of named entries.
///
/// ```ignore
/// let mut e = ClosedEnum::new(format_ident!("Handler"));
/// for h in &index.handlers {
///     e.entry(&h.name, h.name_span);
/// }
/// e.payload(format_ident!("function"), quote!(fn(&str) -> String), checked_consts);
/// let built = e.build();
/// for d in built.diagnostics { cx.report(d); }
/// cx.emit(built.tokens);
/// ```
///
/// Variant names are derived from entry names: `greet` → `Greet`,
/// `count_users_sql` → `CountUsersSql`. An entry whose name does not make a
/// valid identifier, or two entries whose names make the same variant, are
/// errors citing the entries' spans; those entries are left out so the rest
/// still compiles. Discriminants are `0, 1, 2, ..` in the order entries were
/// added (give them canonical order, R5, and the type is deterministic, G1).
#[derive(Clone, Debug)]
pub struct ClosedEnum {
    name: Ident,
    vis: TokenStream,
    entries: Vec<Entry>,
    payloads: Vec<Payload>,
    derives: TokenStream,
    doc: Option<String>,
}

/// What [`ClosedEnum::build`] produced.
#[derive(Debug)]
pub struct Built {
    /// The enum and its impls.
    pub tokens: TokenStream,
    /// Errors about entries that were left out (invalid or colliding names).
    pub diagnostics: Vec<Diagnostic>,
    /// `(entry name, variant ident)` for every variant that made it, in
    /// discriminant order.
    pub variants: Vec<(String, Ident)>,
}

impl ClosedEnum {
    /// A `pub enum <name>`.
    pub fn new(name: Ident) -> Self {
        ClosedEnum {
            name,
            vis: quote!(pub),
            entries: Vec::new(),
            payloads: Vec::new(),
            derives: quote!(#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]),
            doc: None,
        }
    }

    /// Change the visibility (default `pub`).
    pub fn vis(mut self, vis: impl ToTokens) -> Self {
        self.vis = vis.to_token_stream();
        self
    }

    /// Replace the derive list (default `Clone, Copy, PartialEq, Eq, Hash, Debug`).
    pub fn derives(mut self, derives: impl ToTokens) -> Self {
        self.derives = derives.to_token_stream();
        self
    }

    /// A doc comment for the enum.
    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = Some(doc.into());
        self
    }

    /// Add an entry. `span` is where the entry was declared (the name
    /// literal of its record, usually); errors about the name point there,
    /// and the variant ident carries it.
    pub fn entry(&mut self, name: &str, span: Span) -> &mut Self {
        self.entries.push(Entry { name: name.to_string(), span });
        self
    }

    /// Add a per-variant constant: `pub const fn <name>(self) -> <ty>`, with
    /// one value per entry in the same order as the entries were added
    /// (`values.len()` must equal the number of entries).
    pub fn payload(&mut self, name: Ident, ty: impl ToTokens, values: Vec<TokenStream>) -> &mut Self {
        self.payloads.push(Payload { name, ty: ty.to_token_stream(), values });
        self
    }

    /// The variant ident an entry name maps to, if it makes a valid one:
    /// split on `_`, capitalise each part, join. `None` for a name that
    /// would not be an identifier (empty parts, characters outside
    /// `[A-Za-z0-9_]`, a leading digit) or that is a keyword.
    pub fn variant_name(name: &str) -> Option<String> {
        if name.is_empty() {
            return None;
        }
        let mut out = String::new();
        for part in name.split('_') {
            if part.is_empty() {
                continue;
            }
            if !part.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            let mut chars = part.chars();
            let first = chars.next()?;
            out.push(first.to_ascii_uppercase());
            out.push_str(chars.as_str());
        }
        if out.is_empty() || out.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
            return None;
        }
        // Keywords (`Self`, ...) are not identifiers.
        if syn::parse_str::<Ident>(&out).is_err() {
            return None;
        }
        Some(out)
    }

    /// Generate the enum and its impls.
    pub fn build(&self) -> Built {
        let mut diagnostics = Vec::new();
        let mut variants: Vec<(String, Ident, Span, usize)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let Some(variant) = Self::variant_name(&e.name) else {
                diagnostics.push(Diagnostic::error(
                    e.span,
                    format!(
                        "`{}` can't be a variant of `{}`: a name must be letters, digits and underscores, start with a letter, and not be a keyword (it becomes `{}::<Name>` with each `_`-separated part capitalised)",
                        e.name, self.name, self.name
                    ),
                ));
                continue;
            };
            if let Some((prev_name, _, prev_span, _)) = variants.iter().find(|(_, v, _, _)| v == &variant) {
                diagnostics.push(
                    Diagnostic::error(
                        e.span,
                        format!("`{}` and `{}` both map to the variant `{}::{}`", prev_name, e.name, self.name, variant),
                    )
                    .note(Some(*prev_span), format!("`{prev_name}` is declared here")),
                );
                continue;
            }
            variants.push((e.name.clone(), Ident::new(&variant, e.span), e.span, i));
        }

        let name = &self.name;
        let vis = &self.vis;
        let derives = &self.derives;
        let doc = self.doc.as_ref().map(|d| quote!(#[doc = #d]));
        let idents: Vec<&Ident> = variants.iter().map(|(_, v, _, _)| v).collect();
        let discriminants: Vec<Literal> = (0..variants.len()).map(|i| Literal::u16_unsuffixed(i as u16)).collect();
        let names: Vec<Literal> = variants.iter().map(|(n, _, _, _)| Literal::string(n)).collect();
        let count = variants.len();

        let payloads = self.payloads.iter().map(|p| {
            let pname = &p.name;
            let ty = &p.ty;
            let values = variants.iter().map(|(_, _, _, i)| p.values.get(*i).cloned().unwrap_or_else(|| {
                quote_spanned!(self.name.span()=> ::core::compile_error!("ClosedEnum::payload: one value per entry is required"))
            }));
            quote! {
                #[allow(unreachable_patterns)]
                #vis const fn #pname(self) -> #ty {
                    match self { #( Self::#idents => #values, )* }
                }
            }
        });

        // `#[repr(u16)]` on a zero-variant enum is an error (E0084), so an
        // empty set is a plain empty enum.
        let repr = (!variants.is_empty()).then(|| quote!(#[repr(u16)]));
        let tokens = quote! {
            #doc
            #repr
            #[allow(dead_code, clippy::enum_variant_names)]
            #derives
            #vis enum #name {
                #( #idents = #discriminants, )*
            }

            #[allow(dead_code)]
            impl #name {
                /// Every variant, in declaration (canonical) order.
                #vis const ALL: &'static [Self] = &[ #( Self::#idents, )* ];

                /// How many variants there are.
                #vis const COUNT: usize = #count;

                /// The name this variant was declared under.
                #vis const fn name(self) -> &'static str {
                    match self { #( Self::#idents => #names, )* }
                }

                /// The variant declared under `name`, if any. A `match` on
                /// string literals; a perfect hash is future work.
                #vis fn from_name(name: &str) -> ::core::option::Option<Self> {
                    match name {
                        #( #names => ::core::option::Option::Some(Self::#idents), )*
                        _ => ::core::option::Option::None,
                    }
                }

                /// The dense discriminant (`0..COUNT`).
                #vis const fn id(self) -> u16 {
                    match self { #( Self::#idents => #discriminants, )* }
                }

                #( #payloads )*
            }

            impl ::core::convert::TryFrom<u16> for #name {
                /// The rejected id.
                type Error = u16;
                fn try_from(id: u16) -> ::core::result::Result<Self, u16> {
                    match id {
                        #( #discriminants => ::core::result::Result::Ok(Self::#idents), )*
                        _ => ::core::result::Result::Err(id),
                    }
                }
            }
        };
        Built { tokens, diagnostics, variants: variants.into_iter().map(|(n, v, _, _)| (n, v)).collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::format_ident;

    #[test]
    fn variant_names() {
        assert_eq!(ClosedEnum::variant_name("greet").as_deref(), Some("Greet"));
        assert_eq!(ClosedEnum::variant_name("count_users_sql").as_deref(), Some("CountUsersSql"));
        assert_eq!(ClosedEnum::variant_name("Greet").as_deref(), Some("Greet"));
        assert_eq!(ClosedEnum::variant_name("a__b").as_deref(), Some("AB"));
        assert_eq!(ClosedEnum::variant_name("v2").as_deref(), Some("V2"));
        assert_eq!(ClosedEnum::variant_name("hello-world"), None);
        assert_eq!(ClosedEnum::variant_name("2fast"), None);
        assert_eq!(ClosedEnum::variant_name(""), None);
        assert_eq!(ClosedEnum::variant_name("_"), None);
        assert_eq!(ClosedEnum::variant_name("self"), None, "a keyword");
        assert_eq!(ClosedEnum::variant_name("with space"), None);
    }

    #[test]
    fn builds_a_dense_enum_in_order() {
        let mut e = ClosedEnum::new(format_ident!("Handler"));
        e.entry("greet", Span::call_site()).entry("count_users_sql", Span::call_site());
        e.payload(format_ident!("len"), quote!(usize), vec![quote!(5), quote!(15)]);
        let built = e.build();
        assert!(built.diagnostics.is_empty());
        let text = built.tokens.to_string();
        assert!(text.contains("pub enum Handler { Greet = 0 , CountUsersSql = 1 , }"), "{text}");
        assert!(text.contains("pub const ALL : & 'static [Self] = & [Self :: Greet , Self :: CountUsersSql ,] ;"), "{text}");
        assert!(text.contains("pub const COUNT : usize = 2usize ;"), "{text}");
        assert!(text.contains("\"count_users_sql\" => :: core :: option :: Option :: Some (Self :: CountUsersSql)"), "{text}");
        assert!(text.contains("pub const fn len (self) -> usize { match self { Self :: Greet => 5 , Self :: CountUsersSql => 15 , } }"), "{text}");
        assert!(text.contains("impl :: core :: convert :: TryFrom < u16 > for Handler"), "{text}");
        assert_eq!(built.variants.iter().map(|(n, v)| format!("{n}:{v}")).collect::<Vec<_>>(), ["greet:Greet", "count_users_sql:CountUsersSql"]);
    }

    #[test]
    fn invalid_and_colliding_names_are_errors_citing_the_entries() {
        let mut e = ClosedEnum::new(format_ident!("Handler"));
        e.entry("greet", Span::call_site()).entry("hello-world", Span::call_site()).entry("Greet", Span::call_site()).entry("ok", Span::call_site());
        let built = e.build();
        let msgs: Vec<String> = built.diagnostics.iter().map(|d| d.rendered_message()).collect();
        assert_eq!(
            msgs,
            [
                "`hello-world` can't be a variant of `Handler`: a name must be letters, digits and underscores, start with a letter, and not be a keyword (it becomes `Handler::<Name>` with each `_`-separated part capitalised)",
                "`greet` and `Greet` both map to the variant `Handler::Greet`\n  = note: `greet` is declared here",
            ]
        );
        // The bad entries are left out; the others keep dense ids.
        assert_eq!(built.variants.iter().map(|(_, v)| v.to_string()).collect::<Vec<_>>(), ["Greet", "Ok"]);
        assert!(built.tokens.to_string().contains("Greet = 0 , Ok = 1 ,"));
    }

    #[test]
    fn an_empty_set_is_an_empty_enum() {
        let built = ClosedEnum::new(format_ident!("Nothing")).build();
        let text = built.tokens.to_string();
        assert!(text.contains("pub enum Nothing { }"), "{text}");
        assert!(!text.contains("repr"), "no `#[repr(u16)]` on a zero-variant enum (E0084): {text}");
        assert!(text.contains("const ALL : & 'static [Self] = & [] ;"), "{text}");
    }
}
