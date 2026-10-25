//! A source listed only by `plan2`, which knows the `accounts` table solely
//! through the stage `demo_upstream::plan` (SPEC section 10): no raw upstream
//! source is listed there.

#[demo_macros::collect]
pub mod staged {
    /// Resolved against a table that reached the collection as an exported
    /// fact, not as a record. The column type check still happens: the
    /// imported column types are paths into demo-upstream's type aliases.
    #[stq::query(sql = "SELECT owner_email FROM accounts WHERE balance_cents > {floor}")]
    pub struct RichOwners {
        pub floor: i64,
    }

    /// `SELECT *` expands against the imported column list.
    #[stq::query(sql = "SELECT * FROM accounts WHERE id = {id}")]
    pub struct AccountById {
        pub id: i64,
    }
}
