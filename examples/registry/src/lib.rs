//! Runtime half of the `registry` example plugin.
//!
//! The plugin collects functions tagged `#[registry::handler("name")]` across
//! a project, mints a closed `enum Handler` over them at every collection
//! site (SPEC section 11), and resolves `registry::lookup!("name")` and
//! `registry::variant!("name")` calls at build time. This crate only holds
//! what generated code refers to.

#![no_std]

extern crate alloc;
use alloc::string::String;

/// The signature every handler must have.
pub type HandlerFn = fn(&str) -> String;

/// Implemented by a collection for every `registry::lookup!("name")` marker
/// in the collecting crate (R4). The call expands to `<marker as Lookup>::FN`.
pub trait Lookup {
    /// The handler registered under the looked-up name.
    const FN: HandlerFn;
}

/// Implemented by a collection for every `registry::variant!("name")` marker
/// in the collecting crate (R4). The call expands to
/// `<marker as Variant>::HANDLER`, a variant of the collection's closed
/// `Handler` enum.
pub trait Variant {
    /// The collection's `Handler` enum.
    type Handler: Copy;
    /// The variant registered under the looked-up name.
    const HANDLER: Self::Handler;
}
