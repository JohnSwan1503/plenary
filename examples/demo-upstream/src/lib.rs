//! A library that publishes sources for other crates. `collect(export)` makes
//! each carrier `#[macro_export]`, which is what lets a downstream crate list
//! `demo_upstream::accounts` in its own `expand!`.
//!
//! The crate also has its own collection, because sources that use markers
//! (`stq::sql!`, `registry::lookup!`) must be listed in exactly one
//! collection of their own crate (R4). A downstream collection sees those
//! records too but, as they are not local to it, leaves their markers alone.
//!
//! That collection is an exporting one (`expand!(demo_macros; export; ..)`),
//! so it is also a *stage* (SPEC section 10): `demo_upstream::plan` can be
//! listed by a downstream `expand!` instead of the raw sources. What it
//! provides is the `Resolved` and `Index` facts its handlers exported and the
//! records of the items it emitted (the registry's generated handler).

#[demo_macros::collect(export)]
pub mod accounts {
    #[stq::table(name = "accounts")]
    pub struct Accounts {
        pub id: i64,
        pub owner_email: String,
        pub balance_cents: i64,
    }

    #[stq::query(sql = "SELECT id, balance_cents FROM accounts WHERE owner_email = {email}")]
    pub struct AccountByEmail {
        pub email: String,
    }

    /// An inline query inside a function body, resolved by this crate's own
    /// collection (see `plan` below).
    pub fn richest_sql() -> &'static str {
        stq::sql!("SELECT id FROM accounts WHERE balance_cents > {floor}")
    }
}

#[demo_macros::collect(export)]
pub mod upstream_handlers {
    #[registry::handler("upstream_echo")]
    pub fn echo(input: &str) -> String {
        format!("upstream says {input}")
    }
}

/// This crate's own collection, exported as a stage for downstream crates.
pub mod plan {
    plenary::expand!(demo_macros; export; crate::accounts, crate::upstream_handlers);
}
