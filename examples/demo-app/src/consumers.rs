//! Library code using what the collection generated. Everything here is
//! ordinary Rust referring to items that only exist after `expand!` ran:
//! `plan::dispatch`, `plan::HANDLERS`, `plan::SUMMARY`, the
//! `Query<__Collection>` impls and `Collection::SOURCES`. It is also what the
//! rust-analyzer check in the report points at.

use crate::handlers::handlers;
use crate::plan;
use crate::users::users::{AllUsers, OrgUsers};
use plenary::Collection;
use stq_lite::Query;

/// Greets everyone through the generated dispatch table.
pub fn greet_all(names: &[&str]) -> Vec<String> {
    names.iter().filter_map(|n| plan::dispatch("greet", n)).collect()
}

/// The rendered SQL of two queries, through the generated trait impls.
pub fn user_queries() -> [&'static str; 2] {
    [<OrgUsers as Query<plan::__Collection>>::SQL, crate::sql::<AllUsers>()]
}

/// What the `stats` plugin counted.
pub fn summary() -> &'static str {
    plan::SUMMARY
}

/// The sources the collection was built from.
pub fn sources() -> &'static [&'static str] {
    <plan::__Collection as Collection>::SOURCES
}

/// A build-time lookup, used from another module.
pub fn shout(input: &str) -> String {
    handlers::shout_twice(input)
}

/// Every registered handler name.
pub fn handler_names() -> Vec<&'static str> {
    plan::HANDLERS.iter().map(|(name, _)| *name).collect()
}

/// The closed set at work (SPEC section 11): this `match` is exhaustive over
/// every handler the collection knows. Add a `#[registry::handler]` anywhere
/// in the listed sources and this function stops compiling until it has an
/// arm for it; the `extra` arm follows the cargo feature like the handler.
pub fn describe(handler: plan::Handler) -> &'static str {
    match handler {
        plan::Handler::Greet => "greets the input",
        plan::Handler::Shout => "upper-cases the input",
        #[cfg(feature = "extra")]
        plan::Handler::Extra => "only with --features extra",
        plan::Handler::CountUsersSql => "returns the users SQL",
        plan::Handler::UpstreamEcho => "echoes, from demo-upstream",
    }
}

/// `variant!` resolves a name to the enum at build time, like `lookup!`
/// resolves it to the function (see `handlers.rs`).
pub fn shout_variant() -> plan::Handler {
    crate::handlers::handlers::shout_handler()
}
