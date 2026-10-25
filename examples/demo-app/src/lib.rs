//! The example application: sources in several files, two plugins, plus the
//! upstream crate's sources, all brought together in one place.
//!
//! Each file holds one inline source module of the same name (sources must be
//! inline modules, SPEC section 6), hence the allow below.

#![allow(clippy::module_inception)]

pub mod consumers;
pub mod handlers;
pub mod lookups;
pub mod mixed;
pub mod orders;
pub mod staged;
pub mod users;

/// The collection. `__finish` sees every table, query and handler at once, so
/// a query in `orders.rs` can be checked against a table in `users.rs` or in
/// `demo-upstream`.
pub mod plan {
    plenary::expand!(
        demo_macros;
        crate::users::users,
        crate::orders::orders,
        crate::handlers::handlers,
        crate::mixed::mixed,
        demo_upstream::accounts,
        demo_upstream::upstream_handlers,
    );
}

/// A second collection that consumes demo-upstream's *stage* instead of its
/// raw sources (SPEC section 10): the `accounts` table and the
/// `upstream_echo` handler arrive as exported facts, and the handler
/// demo-upstream's registry generated arrives as a record. `staged.rs` is
/// listed here only.
pub mod plan2 {
    plenary::expand!(demo_macros; demo_upstream::plan, crate::staged::staged);
}

/// A same-crate round: `plan` is listed as a stage. Its exported `Resolved`
/// and `Index` are merged, and the `generated_echo` handler it emitted is a
/// record here, so `lookups.rs` can look it up.
pub mod plan3 {
    plenary::expand!(demo_macros; crate::plan, crate::lookups::lookups);
}

/// `sql::<Q>()` without naming the collection marker.
pub fn sql<Q: stq_lite::Query<plan::__Collection>>() -> &'static str {
    Q::SQL
}

/// The same for the stage-consuming collection.
pub fn sql2<Q: stq_lite::Query<plan2::__Collection>>() -> &'static str {
    Q::SQL
}
