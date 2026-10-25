//! The context a global handler gets, obligations, and handler ordering.

use crate::carrier::EncodedFact;
use crate::diag::{Diagnostic, Diagnostics, Error};
use crate::facts::ExportFact;
use crate::plugin::{Bundle, ErasedGlobalHandler, FactId, FactStore};
use crate::util::respan;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, quote_spanned, ToTokens};
use std::any::TypeId;
use std::fmt;

/// What `GlobalCx::export` needs to know about a registered exportable type.
#[derive(Clone, Debug)]
pub(crate) struct ExportableInfo {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
    pub(crate) namespace: &'static str,
    pub(crate) version: &'static str,
}

impl ExportableInfo {
    pub(crate) fn of_bundle(bundle: &Bundle) -> Vec<Self> {
        bundle
            .exportables()
            .map(|(p, e)| ExportableInfo { id: e.id, name: e.name, namespace: p.namespace, version: p.version })
            .collect()
    }
}

/// Everything a global handler may do, besides reading facts.
///
/// Items and obligations are emitted at the collection site. Diagnostics point
/// at whatever span they are given, usually a token from a record.
pub struct GlobalCx {
    collection: Ident,
    plenary: TokenStream,
    site: Span,
    items: Vec<TokenStream>,
    obligations: Vec<TokenStream>,
    exports: Vec<EncodedFact>,
    diagnostics: Diagnostics,
    obligation_counter: usize,
    exportables: Vec<ExportableInfo>,
    /// The plugin whose handler is running (for messages about `export`).
    current_plugin: Option<&'static str>,
}

impl GlobalCx {
    pub(crate) fn new(plenary: TokenStream, site: Span, exportables: Vec<ExportableInfo>) -> Self {
        GlobalCx {
            collection: format_ident!("__Collection"),
            plenary,
            site,
            items: Vec::new(),
            obligations: Vec::new(),
            exports: Vec::new(),
            diagnostics: Diagnostics::new(),
            obligation_counter: 0,
            exportables,
            current_plugin: None,
        }
    }

    pub(crate) fn set_current_plugin(&mut self, plugin: Option<&'static str>) {
        self.current_plugin = plugin;
    }

    /// Export a fact from this collection (SPEC section 10). It travels in
    /// the collection's stage carrier, so a later `expand!` that lists this
    /// collection (in this crate or, with `expand!(bundle; export; ..)`,
    /// downstream) receives it through `&Imported<F>`.
    ///
    /// `F` must have been registered with `Plugin::exportable::<F>()`;
    /// otherwise an error naming the type and the plugin is reported and
    /// nothing is exported. Like items, exports of a handler that fails are
    /// dropped.
    pub fn export<F: ExportFact>(&mut self, fact: &F) {
        let id = TypeId::of::<F>();
        match self.exportables.iter().find(|e| e.id == id) {
            Some(info) => {
                let tokens = fact.to_tokens();
                self.exports.push(EncodedFact {
                    namespace: info.namespace.to_string(),
                    name: info.name.to_string(),
                    version: info.version.to_string(),
                    tokens,
                    span: self.site,
                });
            }
            None => {
                let plugin = self.current_plugin.unwrap_or("?");
                self.diagnostics.error(
                    self.site,
                    format!(
                        "plugin `{plugin}` exported a `{}`, but no plugin in this bundle registered that type with \
                         `.exportable::<{}>()`; add the registration to the plugin that defines the type",
                        std::any::type_name::<F>(),
                        F::NAME
                    ),
                );
            }
        }
    }

    /// The collection's marker type, `__Collection`. Use it as the type
    /// parameter of your traits so a crate can implement them for types from
    /// other crates (section 4).
    pub fn collection(&self) -> &Ident {
        &self.collection
    }

    /// A path to the `plenary` facade crate, valid at the collection site.
    pub fn plenary(&self) -> &TokenStream {
        &self.plenary
    }

    /// The span of the collection site (the `expand!` invocation), for
    /// diagnostics that belong to the whole collection rather than to one
    /// record.
    pub fn site(&self) -> Span {
        self.site
    }

    /// Emit items at the collection site.
    pub fn emit(&mut self, items: impl ToTokens) {
        self.items.push(items.to_token_stream());
    }

    /// Report an error at `span`. Reporting does not stop the handler; return
    /// `Err` for that.
    pub fn error(&mut self, span: Span, message: impl fmt::Display) {
        self.diagnostics.error(span, message);
    }

    /// Report a warning at `span` (rendered as a `deprecated` warning).
    pub fn warn(&mut self, span: Span, message: impl fmt::Display) {
        self.diagnostics.warning(span, message);
    }

    /// Report a prepared diagnostic.
    pub fn report(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Report every diagnostic of an error without failing the handler.
    pub fn report_error(&mut self, error: Error) {
        self.diagnostics.extend_from_error(error);
    }

    /// Obligation: `a` and `b` must be the same type. `message` is a
    /// `#[diagnostic::on_unimplemented]` format string: `{Self}` is `a`,
    /// `{T}` is `b`; write other braces as `{{` and `}}`. The error appears
    /// at `span` as `error[E0277]: <message>` with the label
    /// "`a` and `b` are different types".
    pub fn require_same_type(&mut self, a: impl ToTokens, b: impl ToTokens, span: Span, message: &str) {
        let a = a.to_token_stream();
        let b = b.to_token_stream();
        let tokens = quote! {
            const _: () = {
                #[diagnostic::on_unimplemented(message = #message, label = "`{Self}` and `{T}` are different types")]
                trait SameTypeAs<T> {}
                impl<T> SameTypeAs<T> for T {}
                const fn check<A: SameTypeAs<B>, B>() {}
                check::<#a, #b>();
            };
        };
        self.obligations.push(respan(tokens, span));
    }

    /// Obligation: `ty` must implement `trait_`. `message` is a
    /// `#[diagnostic::on_unimplemented]` format string where `{Self}` is
    /// `ty`. The error appears at `span`.
    ///
    /// This is the best stable Rust allows: a hidden helper trait
    /// `MustImplement` with a blanket impl for everything that implements
    /// `trait_`. The user sees
    ///
    /// ```text
    /// error[E0277]: <message>
    ///   --> src/users.rs:26:20
    ///    |
    /// 26 |         pub email: Vec<u8>,
    ///    |                    ^^^ `Vec<u8>` does not implement `::stq_lite::Param`
    ///    = help: the following other types implement trait `Param`: ...
    /// note: required for `Vec<u8>` to implement `MustImplement`
    /// ```
    ///
    /// The `help` and `note` lines are the compiler's own and can't be
    /// removed.
    pub fn require_impl(&mut self, ty: impl ToTokens, trait_: impl ToTokens, span: Span, message: &str) {
        let ty = ty.to_token_stream();
        let trait_ = trait_.to_token_stream();
        let trait_text = match syn::parse2::<syn::Path>(trait_.clone()) {
            Ok(p) => crate::util::path_text(&p),
            Err(_) => trait_.to_string(),
        };
        let label = format!("`{{Self}}` does not implement `{trait_text}`");
        let tokens = quote! {
            const _: () = {
                #[diagnostic::on_unimplemented(message = #message, label = #label)]
                trait MustImplement {}
                impl<T: ?Sized + #trait_> MustImplement for T {}
                const fn check<A: ?Sized + MustImplement>() {}
                check::<#ty>();
            };
        };
        self.obligations.push(respan(tokens, span));
    }

    /// Obligation: `expr` must coerce to `ty`. Emitted as a hidden constant
    /// `const __PlCoerceN: ty = expr;` whose name is returned, so the checked
    /// value can be used by emitted items without a second coercion (and a
    /// second error). A mismatch is reported by the compiler at `span`.
    pub fn require_coerces(&mut self, expr: impl ToTokens, ty: impl ToTokens, span: Span) -> Ident {
        let expr = expr.to_token_stream();
        let ty = ty.to_token_stream();
        let name = self.next_obligation_name("Coerce");
        let tokens = quote! {
            #[doc(hidden)]
            #[allow(non_upper_case_globals, dead_code)]
            const #name: #ty = #expr;
        };
        self.obligations.push(respan(tokens, span));
        name
    }

    /// Obligation: `expr` must be a true constant expression
    /// (`const _: () = assert!(expr, message);`).
    pub fn require_const(&mut self, expr: impl ToTokens, span: Span, message: &str) {
        let expr = expr.to_token_stream();
        let tokens = quote! { const _: () = ::core::assert!(#expr, #message); };
        self.obligations.push(respan(tokens, span));
    }

    /// Emit a warning as an item, spanned at `span` (same as [`warn`](Self::warn)).
    pub fn warn_at(&mut self, span: Span, message: &str) {
        self.diagnostics.push(Diagnostic::warning(span, message));
    }

    fn next_obligation_name(&mut self, what: &str) -> Ident {
        let n = self.obligation_counter;
        self.obligation_counter += 1;
        format_ident!("__Pl{}{}", what, n)
    }

    /// Items emitted so far (tests).
    pub fn emitted(&self) -> TokenStream {
        self.items.iter().cloned().collect()
    }

    /// Obligations emitted so far (tests).
    pub fn obligations(&self) -> TokenStream {
        self.obligations.iter().cloned().collect()
    }

    /// Diagnostics reported so far (tests).
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// Facts exported so far (tests): `(namespace, name, tokens)`.
    pub fn exports(&self) -> Vec<(String, String, TokenStream)> {
        self.exports.iter().map(|e| (e.namespace.clone(), e.name.clone(), e.tokens.clone())).collect()
    }

    pub(crate) fn take_output(&mut self) -> HandlerOutput {
        HandlerOutput {
            items: std::mem::take(&mut self.items),
            obligations: std::mem::take(&mut self.obligations),
            exports: std::mem::take(&mut self.exports),
            diagnostics: std::mem::take(&mut self.diagnostics),
        }
    }

    pub(crate) fn restore_counter(&mut self, counter: usize) {
        self.obligation_counter = counter;
    }

    pub(crate) fn counter(&self) -> usize {
        self.obligation_counter
    }
}

/// What one handler produced.
#[derive(Default)]
pub(crate) struct HandlerOutput {
    pub(crate) items: Vec<TokenStream>,
    pub(crate) obligations: Vec<TokenStream>,
    pub(crate) exports: Vec<EncodedFact>,
    pub(crate) diagnostics: Diagnostics,
}

/// A global handler with its registration position.
pub(crate) struct Registered<'b> {
    pub(crate) plugin: &'b crate::Plugin,
    pub(crate) handler: &'b dyn ErasedGlobalHandler,
    /// Position in bundle order: plugin index, then handler index.
    pub(crate) order: (usize, usize),
}

impl Registered<'_> {
    pub(crate) fn describe(&self) -> String {
        format!("`{}` (plugin `{}`)", self.handler.name(), self.plugin.name())
    }
}

/// Collect every global handler of the bundle in registration order.
pub(crate) fn registered(bundle: &Bundle) -> Vec<Registered<'_>> {
    let mut out = Vec::new();
    for (pi, p) in bundle.plugins.iter().enumerate() {
        for (hi, h) in p.globals.iter().enumerate() {
            out.push(Registered { plugin: p, handler: h.as_ref(), order: (pi, hi) });
        }
    }
    out
}

/// Order handlers so that each runs after the producers of the facts it reads
/// (R6). Ties keep registration order. `State` and every `Imported<F>` are
/// built in and need no producer (`builtin` lists them).
///
/// Errors name the handlers involved: a fact with no producer, a fact with two
/// producers, or a cycle.
pub(crate) fn order_handlers<'a, 'b>(
    handlers: &'a [Registered<'b>],
    builtin: &[FactId],
) -> std::result::Result<Vec<&'a Registered<'b>>, Vec<String>> {
    let mut errors = Vec::new();
    let state_id = FactId::of::<crate::State>();
    let is_builtin = |id: FactId| id == state_id || builtin.contains(&id);

    // producer of each fact type
    let mut producers: Vec<(FactId, usize)> = Vec::new();
    for (i, h) in handlers.iter().enumerate() {
        if let Some(out) = h.handler.output() {
            if is_builtin(out) {
                errors.push(format!("handler {} produces `{}`, which is built in and can't be produced", h.describe(), short_name(out.name())));
                continue;
            }
            if let Some((_, prev)) = producers.iter().find(|(f, _)| *f == out) {
                errors.push(format!(
                    "fact `{}` has two producers: {} and {}",
                    out.name(),
                    handlers[*prev].describe(),
                    h.describe()
                ));
            } else {
                producers.push((out, i));
            }
        }
    }

    // edges: producer -> consumer
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); handlers.len()];
    for (i, h) in handlers.iter().enumerate() {
        for input in h.handler.inputs() {
            if is_builtin(input) {
                continue;
            }
            match producers.iter().find(|(f, _)| *f == input) {
                Some((_, p)) => deps[i].push(*p),
                None if input.name().contains("::Imported<") => errors.push(format!(
                    "handler {} reads `{}`, but no plugin in this bundle registered that fact type with `.exportable::<..>()`",
                    h.describe(),
                    short_name(input.name())
                )),
                None => errors.push(format!(
                    "handler {} reads fact `{}`, but no handler in this bundle produces it",
                    h.describe(),
                    input.name()
                )),
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // Kahn's algorithm with registration order as the tie-break.
    let n = handlers.len();
    let mut indegree: Vec<usize> = deps.iter().map(|d| d.len()).collect();
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    loop {
        let next = (0..n).filter(|&i| !done[i] && indegree[i] == 0).min_by_key(|&i| handlers[i].order);
        let Some(i) = next else { break };
        done[i] = true;
        order.push(i);
        for (j, d) in deps.iter().enumerate() {
            if !done[j] && d.contains(&i) {
                indegree[j] -= 1;
            }
        }
    }
    if order.len() < n {
        let stuck: Vec<usize> = (0..n).filter(|&i| !done[i]).collect();
        // Describe one cycle: walk from the first stuck handler through its
        // unresolved dependencies until we come back around.
        let mut chain = vec![stuck[0]];
        let mut cur = stuck[0];
        loop {
            let next = deps[cur].iter().copied().find(|d| !done[*d]).expect("stuck handler has a stuck dependency");
            if let Some(pos) = chain.iter().position(|&c| c == next) {
                chain.push(next);
                chain.drain(..pos);
                break;
            }
            chain.push(next);
            cur = next;
        }
        let names: Vec<String> = chain.iter().map(|&i| handlers[i].describe()).collect();
        errors.push(format!(
            "global handlers depend on each other in a cycle: {}",
            names.join(" -> ")
        ));
        return Err(errors);
    }
    Ok(order.into_iter().map(|i| &handlers[i]).collect())
}

/// `plenary_plugin::plugin::Imported<stq_lite_plugin::Resolved>` ->
/// `Imported<stq_lite_plugin::Resolved>`, for messages.
fn short_name(type_name: &str) -> String {
    match type_name.find('<') {
        Some(lt) => {
            let head = &type_name[..lt];
            let head = head.rsplit("::").next().unwrap_or(head);
            format!("{head}{}", &type_name[lt..])
        }
        None => type_name.to_string(),
    }
}

/// Run one handler with panic capture. On success the fact (if any) goes into
/// the store. On `Err` or panic the handler's items, obligations and exports
/// are dropped, its diagnostics are kept, and `false` is returned so
/// dependents can be skipped.
pub(crate) fn run_handler(reg: &Registered<'_>, facts: &mut FactStore, cx: &mut GlobalCx, out: &mut HandlerOutput) -> bool {
    let counter = cx.counter();
    cx.set_current_plugin(Some(reg.plugin.name));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reg.handler.run(facts, cx)));
    cx.set_current_plugin(None);
    let produced = cx.take_output();
    match result {
        Ok(Ok(fact)) => {
            out.items.extend(produced.items);
            out.obligations.extend(produced.obligations);
            out.exports.extend(produced.exports);
            out.diagnostics.extend(produced.diagnostics);
            if let (Some(fact), Some(id)) = (fact, reg.handler.output()) {
                facts.insert_boxed(id.id, fact);
            }
            true
        }
        Ok(Err(e)) => {
            cx.restore_counter(counter);
            out.diagnostics.extend(produced.diagnostics);
            out.diagnostics.extend_from_error(e);
            false
        }
        Err(payload) => {
            cx.restore_counter(counter);
            out.diagnostics.extend(produced.diagnostics);
            out.diagnostics.push(Diagnostic::error(
                cx.site(),
                format!(
                    "plugin `{}` panicked in global handler `{}`: {}",
                    reg.plugin.name(),
                    reg.handler.name(),
                    panic_message(payload.as_ref())
                ),
            ));
            false
        }
    }
}

/// The text of a panic payload.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(non-string panic payload)".to_string()
    }
}

/// The `__Collection` marker and its `Collection` impl.
pub(crate) fn collection_marker(plenary: &TokenStream, sources: &[String]) -> TokenStream {
    let lits = sources.iter().map(|s| proc_macro2::Literal::string(s));
    quote_spanned! {Span::call_site()=>
        /// Marker type for this `expand!` invocation.
        ///
        /// Plugins use it as a type parameter on their traits, so that this crate
        /// can implement them for types from other crates.
        #[allow(non_camel_case_types, dead_code)]
        pub struct __Collection;
        impl #plenary::Collection for __Collection {
            const SOURCES: &'static [&'static str] = &[#(#lits),*];
        }
    }
}
