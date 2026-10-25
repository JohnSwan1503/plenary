//! Runtime half of the `stq-lite` example plugin: the traits that
//! collections implement for query types and inline `stq::sql!` markers.

/// Implemented by a collection for every `#[stq::query]` struct it can see,
/// including structs from other crates. `C` is the collection's
/// `__Collection` marker, which makes the impl local to the collecting crate
/// (orphan rule).
pub trait Query<C> {
    /// The rendered SQL: `SELECT *` expanded to the table's columns and
    /// `{param}` placeholders turned into positional `$n` parameters.
    const SQL: &'static str;
}

/// Implemented by a collection for every `stq::sql!("...")` marker in the
/// collecting crate (R4). The call expands to `<marker as InlineSql>::SQL`.
pub trait InlineSql {
    const SQL: &'static str;
}

/// Types that can be sent as query parameters. Query parameter fields must
/// implement it; the collection checks this with a `require_impl` obligation.
pub trait Param {}

impl Param for i64 {}
impl Param for i32 {}
impl Param for bool {}
impl Param for String {}
impl Param for &str {}

/// `sql::<Q, C>()` without naming the marker: `Q::SQL`.
pub fn sql<Q: Query<C>, C>() -> &'static str {
    Q::SQL
}
