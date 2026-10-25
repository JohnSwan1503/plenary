//! `registry`: a build-time registry of named handler functions.
//!
//! Proves the framework isn't SQL-shaped. Namespace `registry`, two kinds:
//!
//! - `#[registry::handler("name")] pub fn f(input: &str) -> String` — a
//!   handler. Its signature is checked by the collection with a
//!   `require_coerces` obligation, so a wrong signature is reported at the
//!   user's `fn`.
//! - `registry::lookup!("name")` in expression position — resolved at build
//!   time to that handler's function pointer through a marker (R4).
//! - `registry::variant!("name")` in expression position — resolved at build
//!   time to the variant of the collection's closed `Handler` enum.
//!
//! Global handlers build the [`Index`] fact (duplicate names across sources
//! are an error that mentions both locations; records compiled out by `cfg`
//! never count), emit the closed `pub enum Handler` (SPEC section 11, via
//! `plenary_plugin::gen::ClosedEnum`) with `call`, `pub static HANDLERS`
//! and `pub fn dispatch`, and implement `registry::Lookup` and
//! `registry::Variant` for every local marker.
//!
//! Stages (SPEC section 10): `Index` is exportable, so a collection listing
//! this one starts its registry from the handlers this one knows. `emit`
//! also generates one handler, `generated_echo`, tagged
//! `#[registry::handler]`: it is not in this collection's own registry (one
//! round per collection) but a collection listing this one sees it as an
//! ordinary record.

use plenary_plugin::facts::{Decode, Encode, Fields};
use plenary_plugin::gen::ClosedEnum;
use plenary_plugin::{Diagnostic, ExportFact, GlobalCx, Imported, LocalCx, Plugin, Record, Result, State};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::ParseStream;
use syn::{ItemFn, LitStr};

/// The plugin, for `bundle!`.
pub fn plugin() -> Plugin {
    Plugin::new("registry", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        .attribute_kind("handler", handler)
        .call_kind("lookup", lookup)
        .call_kind("variant", variant)
        .exportable::<Index>()
        .global(index)
        .global(emit)
        .global(lookups)
}

/// The closed enum `emit` minted: which handler name became which variant.
/// Names that made no variant (not an identifier, or colliding with another
/// name's variant) are absent; `emit` reported them.
#[derive(Debug)]
pub struct HandlerEnum {
    pub variants: Vec<(String, syn::Ident)>,
}

impl plenary_plugin::Fact for HandlerEnum {}

impl HandlerEnum {
    pub fn variant_of(&self, name: &str) -> Option<&syn::Ident> {
        self.variants.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// Every handler in the collection, in canonical order: the ones imported
/// from listed stages first, then the ones in listed sources.
#[derive(Debug)]
pub struct Index {
    pub handlers: Vec<HandlerInfo>,
}

impl plenary_plugin::Fact for Index {}

impl ExportFact for Index {
    const NAME: &'static str = "Index";

    fn to_tokens(&self) -> TokenStream {
        Fields::new().with("handlers", &self.handlers).encode()
    }

    fn from_tokens(tokens: TokenStream) -> Result<Self> {
        let f = Fields::decode(tokens)?;
        Ok(Index { handlers: f.get("handlers")? })
    }
}

#[derive(Debug)]
pub struct HandlerInfo {
    pub name: String,
    /// Absolute path of the function.
    pub path: syn::Path,
    /// Span of the `"name"` literal.
    pub name_span: Span,
    /// Span of the function's name.
    pub fn_span: Span,
}

impl Encode for HandlerInfo {
    fn encode(&self, out: &mut TokenStream) {
        // The name keeps its span (a literal), the function's span travels as
        // a bare span, so downstream errors can still point upstream.
        let name = LitStr::new(&self.name, self.name_span);
        Fields::new().with("name", &name).with("path", &self.path).with("fn_span", &self.fn_span).encode_into(out)
    }
}

impl Decode for HandlerInfo {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let f = Fields::parse(input)?;
        let name: LitStr = f.get("name")?;
        Ok(HandlerInfo { name: name.value(), name_span: name.span(), path: f.get("path")?, fn_span: f.get("fn_span")? })
    }
}

impl Index {
    pub fn find(&self, name: &str) -> Option<&HandlerInfo> {
        self.handlers.iter().find(|h| h.name == name)
    }
}

// ───────────────────────────── local handlers ─────────────────────────────

fn handler(record: &Record, cx: &mut LocalCx) -> Result<()> {
    let name: LitStr = record
        .parse_args()
        .map_err(|_| plenary_plugin::Error::new(record.kind().span(), "expected `#[registry::handler(\"name\")]`"))?;
    let item: ItemFn = record
        .parse_item()
        .map_err(|_| plenary_plugin::Error::new(record.kind().span(), "`#[registry::handler]` must be placed on a function"))?;
    if name.value().chars().any(|c| c.is_ascii_uppercase()) {
        // A local warning rule: names are matched case-sensitively at runtime.
        cx.warn(name.span(), format!("handler name `{}` has uppercase letters; names are matched case-sensitively", name.value()));
    }
    cx.annotate("name", &name);
    cx.annotate("fn", &item.sig.ident);
    Ok(())
}

fn lookup(record: &Record, cx: &mut LocalCx) -> Result<()> {
    let name: LitStr = record
        .parse_args()
        .map_err(|_| plenary_plugin::Error::new(record.kind().span(), "expected `registry::lookup!(\"name\")`"))?;
    let marker = cx.marker();
    cx.annotate("name", &name);
    cx.set_replacement(quote!(<#marker as ::registry::Lookup>::FN));
    Ok(())
}

fn variant(record: &Record, cx: &mut LocalCx) -> Result<()> {
    let name: LitStr = record
        .parse_args()
        .map_err(|_| plenary_plugin::Error::new(record.kind().span(), "expected `registry::variant!(\"name\")`"))?;
    let marker = cx.marker();
    cx.annotate("name", &name);
    cx.set_replacement(quote!(<#marker as ::registry::Variant>::HANDLER));
    Ok(())
}

// ───────────────────────────── global handlers ─────────────────────────────

/// Build the index: the handlers of the listed stages first (canonical
/// order), then the listed sources' records. Duplicate names are errors
/// mentioning both locations. The index is exported for the next stage.
pub fn index(state: &State, upstream: &Imported<Index>, cx: &mut GlobalCx) -> Result<Index> {
    let mut handlers: Vec<HandlerInfo> = Vec::new();
    // Where each handler came from: `Some(stage path)` for imported ones.
    let mut origin: Vec<Option<String>> = Vec::new();
    let also = |handlers: &[HandlerInfo], origin: &[Option<String>], prev: usize| match &origin[prev] {
        Some(stage) => format!("also by `{}`, imported from stage `{stage}`", plenary_plugin::path_text(&handlers[prev].path)),
        None => format!("also by `{}`", plenary_plugin::path_text(&handlers[prev].path)),
    };
    for (stage, imported) in upstream.iter() {
        for h in &imported.handlers {
            if let Some(prev) = handlers.iter().position(|p| p.name == h.name) {
                cx.report(
                    Diagnostic::error(
                        state.sources()[stage.index()].path()[0].span(),
                        format!(
                            "handler name `{}` from stage `{}` is registered twice ({})",
                            h.name,
                            stage.display_path(),
                            also(&handlers, &origin, prev)
                        ),
                    )
                    .note(Some(handlers[prev].name_span), "first registered here"),
                );
                continue;
            }
            handlers.push(HandlerInfo { name: h.name.clone(), path: h.path.clone(), name_span: h.name_span, fn_span: h.fn_span });
            origin.push(Some(stage.display_path()));
        }
    }
    for record in state.records_of("registry", "handler") {
        if record.is_tainted() {
            continue;
        }
        let Some(Ok(name)) = record.annotation_as::<LitStr>("name") else { continue };
        let Some(Ok(fn_ident)) = record.annotation_as::<syn::Ident>("fn") else { continue };
        if let Some(prev) = handlers.iter().position(|h| h.name == name.value()) {
            cx.report(
                Diagnostic::error(
                    name.span(),
                    format!("handler name `{}` is registered twice ({})", name.value(), also(&handlers, &origin, prev)),
                )
                .note(Some(handlers[prev].name_span), "first registered here"),
            );
            continue;
        }
        handlers.push(HandlerInfo {
            name: name.value(),
            path: record.absolute_path(),
            name_span: name.span(),
            fn_span: fn_ident.span(),
        });
        origin.push(None);
    }
    let index = Index { handlers };
    cx.export(&index);
    Ok(index)
}

/// Emit one signature obligation per handler, the closed `enum Handler` over
/// them (SPEC section 11) with `call` dispatching through the checked
/// constants, `HANDLERS` and `dispatch` for compatibility, and one generated
/// handler.
pub fn emit(index: &Index, cx: &mut GlobalCx) -> Result<HandlerEnum> {
    // `require_coerces` emits `const __PlCoerceN: fn(&str) -> String = path;`
    // spanned at the user's `fn`, so a wrong signature is reported there,
    // once. Everything below reuses the checked constants.
    let checked: Vec<syn::Ident> =
        index.handlers.iter().map(|h| cx.require_coerces(&h.path, quote!(fn(&str) -> String), h.fn_span)).collect();

    // The closed enum: one variant per handler, dense ids in canonical order.
    // Its name carries the collection site's span, so rustc's notes about
    // the type ("`Handler` defined here") point at the `expand!`.
    let mut closed = ClosedEnum::new(syn::Ident::new("Handler", cx.site()))
        .doc(" Every `#[registry::handler]` this collection can see, as a closed set: a `match` over it is checked for exhaustiveness, `ALL` enumerates it, and `from_name`/`name`/`id`/`TryFrom<u16>` convert.");
    for h in &index.handlers {
        closed.entry(&h.name, h.name_span);
    }
    closed.payload(format_ident!("function"), quote!(fn(&str) -> String), checked.iter().map(|c| quote!(#c)).collect());
    let built = closed.build();
    for d in built.diagnostics {
        cx.report(d);
    }
    cx.emit(built.tokens);

    let names = index.handlers.iter().map(|h| LitStr::new(&h.name, h.name_span));
    // No macro in the generated body: every macro nested in emitted items
    // costs one more level of expansion depth (SPEC section 6). The generated
    // handler's name and ident carry the collection site's span, so a
    // collision reported by a later collection points at this `expand!`.
    let prefix = format!("echo from a registry of {} handlers: ", index.handlers.len());
    let generated_name = LitStr::new("generated_echo", cx.site());
    let generated_fn = syn::Ident::new("generated_echo", cx.site());
    cx.emit(quote! {
        impl Handler {
            /// Run this handler.
            pub fn call(self, input: &str) -> String {
                (self.function())(input)
            }
        }

        /// Every `#[registry::handler]` this collection can see, by name.
        pub static HANDLERS: &[(&str, fn(&str) -> String)] = &[ #( (#names, #checked) ),* ];

        /// Run the handler registered under `name`.
        pub fn dispatch(name: &str, input: &str) -> Option<String> {
            Handler::from_name(name).map(|h| h.call(input))
        }

        /// A handler this collection generated. It carries a plugin attribute,
        /// so it is a record of the *stage* this collection is: not in this
        /// collection's own `HANDLERS` (one round per collection), but in the
        /// registry of any collection that lists this one.
        #[registry::handler(#generated_name)]
        pub fn #generated_fn(input: &str) -> String {
            let mut out = ::std::string::String::from(#prefix);
            out.push_str(input);
            out
        }
    });
    Ok(HandlerEnum { variants: built.variants })
}

/// Implement `registry::Lookup` for every local `lookup!` marker and
/// `registry::Variant` for every local `variant!` marker.
pub fn lookups(state: &State, index: &Index, handler_enum: &HandlerEnum, cx: &mut GlobalCx) -> Result<()> {
    let unknown = |cx: &mut GlobalCx, name: &LitStr| {
        let mut msg = format!("no handler is registered as `{}`", name.value());
        if let Some(s) = plenary_plugin::suggest(&name.value(), index.handlers.iter().map(|h| h.name.as_str())) {
            msg.push_str(&format!("; did you mean `{s}`?"));
        }
        cx.error(name.span(), msg);
    };
    for record in state.records_of("registry", "lookup") {
        if record.is_tainted() || !state.source_of(record).is_local() {
            continue;
        }
        let Some(Ok(name)) = record.annotation_as::<LitStr>("name") else { continue };
        let Some(marker) = record.marker_paths().into_iter().next() else { continue };
        match index.find(&name.value()) {
            Some(h) => {
                let path = &h.path;
                cx.emit(quote! {
                    impl ::registry::Lookup for #marker {
                        const FN: fn(&str) -> String = #path;
                    }
                });
            }
            None => {
                unknown(cx, &name);
                // A placeholder impl, so the user sees this error and not a
                // second one about the missing trait impl.
                cx.emit(quote! {
                    impl ::registry::Lookup for #marker {
                        const FN: fn(&str) -> String = |_| ::std::string::String::new();
                    }
                });
            }
        }
    }
    for record in state.records_of("registry", "variant") {
        if record.is_tainted() || !state.source_of(record).is_local() {
            continue;
        }
        let Some(Ok(name)) = record.annotation_as::<LitStr>("name") else { continue };
        let Some(marker) = record.marker_paths().into_iter().next() else { continue };
        match handler_enum.variant_of(&name.value()) {
            Some(variant) => {
                cx.emit(quote! {
                    impl ::registry::Variant for #marker {
                        type Handler = Handler;
                        const HANDLER: Handler = Handler::#variant;
                    }
                });
            }
            // A registered name with no variant was already reported by `emit`;
            // an empty enum has no placeholder value, so no impl is emitted.
            None if index.find(&name.value()).is_some() => {
                cx.error(name.span(), format!("`{}` has no variant in `Handler` (see the error about its name)", name.value()));
            }
            None => unknown(cx, &name),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use plenary_plugin::testing;

    #[test]
    fn index_and_lookup_as_plain_functions() {
        let a = testing::collect(
            vec![plugin()],
            quote! {
                mod a {
                    #[registry::handler("greet")]
                    pub fn greet(s: &str) -> String { format!("hi {s}") }
                    pub fn call() -> String { registry::lookup!("greet")("x") }
                }
            },
        );
        assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
        let state = testing::state(vec![("crate::a", a.records)]);
        let mut cx = testing::global_cx_with(vec![plugin()]);
        let idx = index(&state, &testing::imported(), &mut cx).unwrap();
        assert_eq!(idx.handlers.len(), 1);
        assert_eq!(idx.handlers[0].name, "greet");
        assert_eq!(cx.exports().len(), 1, "the index is exported");
        let handler_enum = emit(&idx, &mut cx).unwrap();
        assert_eq!(handler_enum.variants.iter().map(|(n, v)| format!("{n}:{v}")).collect::<Vec<_>>(), ["greet:Greet"]);
        lookups(&state, &idx, &handler_enum, &mut cx).unwrap();
        assert!(!cx.diagnostics().has_errors());
        let text = cx.emitted().to_string();
        assert!(text.contains("impl :: registry :: Lookup for crate :: a :: __pl_m0"), "{text}");
        assert!(text.contains("pub enum Handler { Greet = 0 , }"), "{text}");
        assert!(text.contains("pub fn dispatch (name : & str , input : & str) -> Option < String > { Handler :: from_name (name) . map (| h | h . call (input)) }"), "{text}");
    }

    #[test]
    fn variant_lookups_and_bad_variant_names() {
        let a = testing::collect(
            vec![plugin()],
            quote! {
                mod a {
                    #[registry::handler("greet")] pub fn greet(s: &str) -> String { s.into() }
                    #[registry::handler("hello-world")] pub fn hw(s: &str) -> String { s.into() }
                    pub fn v() -> impl Copy { registry::variant!("greet") }
                    pub fn w() -> impl Copy { registry::variant!("hello-world") }
                }
            },
        );
        assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
        let state = testing::state(vec![("crate::a", a.records)]);
        let mut cx = testing::global_cx_with(vec![plugin()]);
        let idx = index(&state, &testing::imported(), &mut cx).unwrap();
        let handler_enum = emit(&idx, &mut cx).unwrap();
        lookups(&state, &idx, &handler_enum, &mut cx).unwrap();
        let msgs: Vec<String> = cx.diagnostics().iter().map(|d| d.message().to_string()).collect();
        assert_eq!(
            msgs,
            [
                "`hello-world` can't be a variant of `Handler`: a name must be letters, digits and underscores, start with a letter, and not be a keyword (it becomes `Handler::<Name>` with each `_`-separated part capitalised)",
                "`hello-world` has no variant in `Handler` (see the error about its name)",
            ]
        );
        let text = cx.emitted().to_string();
        assert!(text.contains("impl :: registry :: Variant for crate :: a :: __pl_m0 { type Handler = Handler ; const HANDLER : Handler = Handler :: Greet ; }"), "{text}");
        assert!(!text.contains("Variant for crate :: a :: __pl_m1"), "{text}");
        // `hello-world` is still dispatchable by name and in HANDLERS, just not a variant.
        assert!(text.contains("(\"hello-world\" , __PlCoerce1)"), "{text}");
    }

    #[test]
    fn duplicate_names_are_reported_once() {
        let a = testing::collect(vec![plugin()], quote! { mod a { #[registry::handler("x")] pub fn a(s: &str) -> String { s.into() } } });
        let b = testing::collect(vec![plugin()], quote! { mod b { #[registry::handler("x")] pub fn b(s: &str) -> String { s.into() } } });
        let state = testing::state(vec![("crate::a", a.records), ("crate::b", b.records)]);
        let mut cx = testing::global_cx_with(vec![plugin()]);
        let idx = index(&state, &testing::imported(), &mut cx).unwrap();
        assert_eq!(idx.handlers.len(), 1);
        let msgs: Vec<String> = cx.diagnostics().iter().map(|d| d.rendered_message()).collect();
        assert_eq!(msgs, vec!["handler name `x` is registered twice (also by `crate::a::a`)\n  = note: first registered here"]);
    }

    #[test]
    fn imported_handlers_come_first_and_collide_with_local_ones() {
        let imported = Index {
            handlers: vec![
                HandlerInfo { name: "up".into(), path: syn::parse_quote!(up::h::up), name_span: Span::call_site(), fn_span: Span::call_site() },
                HandlerInfo { name: "x".into(), path: syn::parse_quote!(up::h::x), name_span: Span::call_site(), fn_span: Span::call_site() },
            ],
        };
        let b = testing::collect(vec![plugin()], quote! { mod b { #[registry::handler("x")] pub fn b(s: &str) -> String { s.into() } #[registry::handler("y")] pub fn y(s: &str) -> String { s.into() } } });
        let state = testing::state(vec![("up::plan", vec![]), ("crate::b", b.records)]);
        let mut cx = testing::global_cx_with(vec![plugin()]);
        let idx = index(&state, &testing::imported_from(vec![("up::plan", imported)]), &mut cx).unwrap();
        let names: Vec<&str> = idx.handlers.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["up", "x", "y"]);
        let msgs: Vec<String> = cx.diagnostics().iter().map(|d| d.message().to_string()).collect();
        assert_eq!(msgs, vec!["handler name `x` is registered twice (also by `up::h::x`, imported from stage `up::plan`)"]);
    }

    #[test]
    fn the_index_round_trips_as_a_fact() {
        let idx = Index {
            handlers: vec![HandlerInfo { name: "greet".into(), path: syn::parse_quote!(crate::h::greet), name_span: Span::call_site(), fn_span: Span::call_site() }],
        };
        let back = Index::from_tokens(idx.to_tokens()).unwrap();
        assert_eq!(back.handlers.len(), 1);
        assert_eq!(back.handlers[0].name, "greet");
        assert_eq!(back.handlers[0].path, idx.handlers[0].path);
    }

    #[test]
    fn the_generated_handler_is_a_record_of_the_next_round_only() {
        let a = testing::collect(vec![plugin()], quote! { mod a { #[registry::handler("greet")] pub fn greet(s: &str) -> String { s.into() } } });
        let out = testing::finish(vec![plugin()], vec![("crate::a", a.records)]);
        assert!(!out.has_errors(), "{:?}", out.errors);
        let text = out.output.to_string();
        assert!(text.contains("pub static HANDLERS : & [(& str , fn (& str) -> String)] = & [(\"greet\" , __PlCoerce0)] ;"), "{text}");
        let stage = out.stage();
        assert_eq!(stage.records.len(), 1);
        assert_eq!(stage.records[0].annotation("name").unwrap().to_string(), "\"generated_echo\"");
        // A collection listing the first one has both.
        let second = testing::finish_blocks(
            vec![plugin()],
            vec![testing::SourceBlock::new("crate::plan").stage().records(stage.records).fact("registry", "Index", env!("CARGO_PKG_VERSION"), stage.facts[0].tokens.clone())],
        );
        assert!(!second.has_errors(), "{:?}", second.errors);
        let text = second.output.to_string();
        assert!(text.contains("& [(\"greet\" , __PlCoerce0) , (\"generated_echo\" , __PlCoerce1)] ;"), "{text}");
        assert!(text.contains("const __PlCoerce1 : fn (& str) -> String = crate :: plan :: generated_echo ;"), "{text}");
    }
}
