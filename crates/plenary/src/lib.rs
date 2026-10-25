//! `plenary`: the application-facing half of a framework for *safe stateful
//! macro expansion*.
//!
//! The framework keeps all expansion state where the compiler can see it:
//!
//! - A **source** is an inline module marked with a bundle's `#[collect]`
//!   attribute. Items and macro calls tagged with a plugin namespace inside it
//!   become **records**, which the attribute copies into a **carrier** (an
//!   ordinary `macro_rules!` emitted next to the module).
//! - A **collection** is a [`expand!`] invocation. It pulls the records of the
//!   listed sources through their carriers and hands all of them to the bundle's
//!   `__finish` proc macro at once, which runs every plugin's global handlers.
//!
//! This crate holds what an application needs: [`expand!`], the
//! [`Collection`] trait that every collection's `__Collection` marker
//! implements, and the carrier format version.
//!
//! Plugin authors depend on `plenary-plugin` instead, and bundles are
//! one-line proc-macro crates built with `plenary_plugin::bundle!`.
//!
//! # Quick start (application)
//!
//! ```ignore
//! // users.rs
//! #[demo_macros::collect]
//! pub mod users {
//!     #[stq::table(name = "users")]
//!     pub struct Users { pub id: i64, pub email: String }
//! }
//!
//! // lib.rs
//! pub mod plan {
//!     plenary::expand!(demo_macros; crate::users::users, demo_upstream::accounts);
//! }
//! ```

#![no_std]
#![forbid(unsafe_code)]

/// Version of the token format a carrier macro emits.
///
/// `__finish` only accepts carriers of exactly this version (R10). Bump it when
/// the shape of the carrier tokens or the carrier calling convention changes.
///
/// History: v1 had `expand!` call every carrier itself (two expansion levels
/// per source); v2 carriers chain to the next source's carrier directly (one
/// level per source).
pub const CARRIER_FORMAT_VERSION: u32 = 2;

/// Implemented by every collection's `__Collection` marker type.
///
/// It lists the sources the collection was built from, spelled the way
/// they were listed in [`expand!`]. Useful for tests and introspection.
pub trait Collection {
    /// The listed source paths, in list order (this is also the canonical
    /// record order, R5).
    const SOURCES: &'static [&'static str];
}

/// Expand a set of sources into a collection.
///
/// ```ignore
/// pub mod plan {
///     plenary::expand!(demo_macros; crate::users::users, crate::orders::orders, demo_upstream::accounts);
/// }
/// ```
///
/// The first argument is the path of the **bundle** crate (the proc-macro crate
/// built with `plenary_plugin::bundle!`). It must be a path, because a
/// proc-macro crate cannot export `macro_rules!` and so `expand!` has to live
/// here and call the bundle's `__finish` itself.
///
/// Every following argument is the path of a source module, or of a module
/// holding another collection (a *stage*, `SPEC.md` section 10: the facts
/// its handlers exported and the records found in the items it emitted). A
/// local one is spelled from `crate`, `self` or `super`; one from another
/// crate is spelled from that crate's name and must have been marked
/// `collect(export)` (a source) or `expand!(bundle; export; ..)` (a stage).
///
/// `export` between the bundle and the list makes this collection's stage
/// carrier usable from other crates (`#[macro_export]`); without it the
/// collection can still be listed by other collections of the same crate:
///
/// ```ignore
/// pub mod plan {
///     plenary::expand!(demo_macros; export; crate::accounts::accounts);
/// }
/// ```
///
/// Every collection emits a `__pl_carrier` alias in its module, so put one
/// `expand!` per module.
///
/// # Protocol
///
/// `expand!` only starts a chain: it calls the carrier alias the collector
/// left in the first listed module and hands it the rest of the list.
///
/// ```text
/// <first>::__pl_carrier! { <bundle>::__finish [ plenary = [ $crate ] bundle = [ <bundle> ] ] { } [ <first> ] <second> , <third> , }
/// ```
///
/// Every carrier appends its own `source [..] { .. }` block to the accumulator
/// in braces and, after chaining through one per-record carrier for each
/// conditional record (R7), calls the *next* source's carrier directly:
///
/// ```text
/// <second>::__pl_carrier! { <callback> [ <prefix> ] { source [ <first> ] { .. } } [ <second> ] <third> , }
/// ```
///
/// The last carrier (its remaining list is empty) calls the callback, which is
/// the bundle's `__finish` proc macro:
///
/// ```text
/// <bundle>::__finish! { plenary = [ $crate ] bundle = [ <bundle> ] { source [..] { .. } source [..] { .. } } }
/// ```
///
/// (`bundle = [..]` carries user-written tokens, so the engine knows the span
/// of the collection site for errors that belong to the whole collection;
/// `export` follows it for an exporting collection.)
///
/// `__finish` emits, next to its items, a carrier with this same calling
/// convention (alias `__pl_carrier`) whose block is marked `stage ;` and
/// holds the exported facts and the records of the emitted items. That is
/// what lets a later `expand!` list this module.
///
/// So a collection costs `1 + (number of sources) + (number of conditional
/// records) + 1` levels of expansion depth, counted against `recursion_limit`:
/// one for `expand!`, one per carrier, one per conditional record's carrier
/// and one for `__finish`, plus the deepest macro nesting inside what the
/// plugins emit (a `#[derive]` or a `#[diagnostic]` attribute adds one). On
/// rustc 1.97 the default limit of 128 allows 125 unconditional sources with
/// the example plugins. Past that, add `#![recursion_limit = "1024"]` to the
/// crate: the compiler reports `recursion limit reached` itself, because
/// `__finish` never runs when the chain is cut short.
#[macro_export]
macro_rules! expand {
    // Empty list: straight to the bundle.
    ($($bundle:ident)::+ ; $(,)?) => {
        $($bundle)::+::__finish! { plenary = [ $crate ] bundle = [ $($bundle)::+ ] { } }
    };
    ($($bundle:ident)::+ ; export ; $(,)?) => {
        $($bundle)::+::__finish! { plenary = [ $crate ] bundle = [ $($bundle)::+ ] export { } }
    };

    // An exporting collection (a stage other crates can list): the same
    // chain, with `export` in the prefix so `__finish` marks the stage
    // carrier `#[macro_export]`.
    ($($bundle:ident)::+ ; export ; $($first:ident)::+ $(, $($rest:tt)*)?) => {
        $($first)::+::__pl_carrier! {
            $($bundle)::+::__finish [ plenary = [ $crate ] bundle = [ $($bundle)::+ ] export ] { } [ $($first)::+ ] $($($rest)*)? ,
        }
    };

    // Start the chain at the first source's carrier. The trailing comma makes
    // sure the last path is followed by one (carriers consume `path ,`).
    ($($bundle:ident)::+ ; $($first:ident)::+ $(, $($rest:tt)*)?) => {
        $($first)::+::__pl_carrier! {
            $($bundle)::+::__finish [ plenary = [ $crate ] bundle = [ $($bundle)::+ ] ] { } [ $($first)::+ ] $($($rest)*)? ,
        }
    };
}
