//! `plenary-plugin`: the plugin API and engine for safe stateful macro
//! expansion. See `SPEC.md` in the repository for the contract.
//!
//! Three audiences use this crate:
//!
//! - **Plugin authors** write an ordinary library crate exposing
//!   `pub fn plugin() -> Plugin`. A [`Plugin`] owns a namespace, registers
//!   record kinds with their local handlers, and registers global handlers.
//! - **Bundles** are proc-macro crates whose entire source is
//!   `plenary_plugin::bundle!(a_plugin::plugin, b_plugin::plugin);`.
//! - **Applications** depend on the `plenary` facade and the bundle.
//!
//! # Plugin author quick start
//!
//! ```ignore
//! use plenary_plugin::{GlobalCx, LocalCx, Plugin, Record, Result, State};
//!
//! pub struct Index { pub names: Vec<String> }
//! impl plenary_plugin::Fact for Index {}
//!
//! fn handler(record: &Record, cx: &mut LocalCx) -> Result<()> {
//!     let name: syn::LitStr = record.parse_args()?;
//!     cx.annotate("name", &name);
//!     Ok(())
//! }
//!
//! fn index(state: &State, cx: &mut GlobalCx) -> Result<Index> {
//!     let mut names = Vec::new();
//!     for r in state.records_of("registry", "handler") {
//!         let name: syn::LitStr = r.annotation_as("name").unwrap()?;
//!         names.push(name.value());
//!     }
//!     Ok(Index { names })
//! }
//!
//! fn emit(index: &Index, cx: &mut GlobalCx) -> Result<()> {
//!     let n = index.names.len();
//!     cx.emit(quote::quote! { pub const HANDLER_COUNT: usize = #n; });
//!     Ok(())
//! }
//!
//! pub fn plugin() -> Plugin {
//!     Plugin::new("registry", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
//!         .attribute_kind("handler", handler)
//!         .global(index)
//!         .global(emit)
//! }
//! ```
//!
//! The engine is written against `proc_macro2`, so everything above can be
//! unit-tested without a compiler: see [`testing`].
//!
//! Two more pieces (SPEC sections 10 and 11): a collection is also a *stage*
//! that later collections can list, exporting facts that implement
//! [`ExportFact`] (codecs in [`facts`]) and importing them as [`Imported`];
//! and [`gen::ClosedEnum`] turns a record set into a closed enum.

#![forbid(unsafe_code)]

extern crate proc_macro;

mod carrier;
mod collect;
mod diag;
pub mod facts;
mod finish;
pub mod gen;
mod global;
mod local;
mod plugin;
mod record;
pub mod testing;
mod util;

pub use diag::{span_key, span_location, Diagnostic, Diagnostics, Error, Level, Result};
pub use global::GlobalCx;
pub use local::{LocalCx, LocalSourceInfo, Marker};
pub use facts::ExportFact;
pub use plugin::{
    keep, Bundle, BundleSpec, ErasedGlobalHandler, ExportableDef, Fact, FactId, FactOutput, FactStore, GlobalHandler, Imported,
    LocalHandler, Plugin,
};
pub use record::{Form, Kind, PluginVersion, Record, SourceInfo, State};
pub use util::{path_text, suggest};

/// Version of the carrier token format this engine reads and writes.
pub use carrier::CARRIER_FORMAT_VERSION;

/// Engine entry points, used by the code `bundle!` generates and by the
/// engine's own tests. Not part of the stable API.
#[doc(hidden)]
pub mod __private {
    pub use crate::carrier::{decode_source, encode_header, encode_record, encode_source_body, DecodedSource, SourceDesc};
    pub use crate::collect::{collect_full, collect_impl, CollectOutput};
    pub use crate::finish::finish_impl;
    pub use crate::plugin::{Bundle, BundleSpec};
    pub use proc_macro2::TokenStream;

    /// Entry point of the generated `collect` attribute.
    pub fn collect_entry(spec: &BundleSpec, args: TokenStream, item: TokenStream) -> TokenStream {
        match Bundle::from_spec(spec) {
            Ok(bundle) => collect_impl(&bundle, args, item),
            Err(msg) => bundle_error(&msg, Some(item)),
        }
    }

    /// Entry point of the generated `__finish` macro.
    pub fn finish_entry(spec: &BundleSpec, input: TokenStream) -> TokenStream {
        match Bundle::from_spec(spec) {
            Ok(bundle) => finish_impl(&bundle, input),
            Err(msg) => bundle_error(&msg, None),
        }
    }

    fn bundle_error(msg: &str, item: Option<TokenStream>) -> TokenStream {
        let msg = format!("invalid bundle: {msg}");
        let mut out = quote::quote!(::core::compile_error! { #msg });
        if let Some(item) = item {
            out.extend(item);
        }
        out
    }
}

/// Generate a bundle: the `collect` attribute and the `__finish` proc macro
/// for a list of plugins.
///
/// Use it as the entire source of a proc-macro crate:
///
/// ```ignore
/// // Cargo.toml: [lib] proc-macro = true
/// plenary_plugin::bundle!(stq_lite_plugin::plugin, registry_plugin::plugin);
/// ```
///
/// Plugins are listed by the path of their `plugin()` constructor. List order
/// only breaks ties between global handlers that don't depend on each other
/// (R6); dependencies come from handler signatures.
#[macro_export]
macro_rules! bundle {
    ($($plugin:expr),+ $(,)?) => {
        const __PL_BUNDLE: $crate::BundleSpec = $crate::BundleSpec {
            crate_name: env!("CARGO_CRATE_NAME"),
            plugins: &[$($plugin),+],
        };

        /// Mark an inline module as a source. Items and macro calls tagged with
        /// a plugin namespace inside it become records. `#[collect(export)]`
        /// makes the source usable from other crates.
        #[proc_macro_attribute]
        pub fn collect(args: ::proc_macro::TokenStream, item: ::proc_macro::TokenStream) -> ::proc_macro::TokenStream {
            $crate::__private::collect_entry(&__PL_BUNDLE, args.into(), item.into()).into()
        }

        /// Called by `plenary::expand!`. Not for direct use.
        #[doc(hidden)]
        #[proc_macro]
        pub fn __finish(input: ::proc_macro::TokenStream) -> ::proc_macro::TokenStream {
            $crate::__private::finish_entry(&__PL_BUNDLE, input.into()).into()
        }
    };
}
