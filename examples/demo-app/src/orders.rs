#[demo_macros::collect]
pub mod orders {
    #[stq::table(name = "orders")]
    pub struct Orders {
        pub id: i64,
        pub user_id: i64,
        pub total_cents: i64,
        pub placed_at: String,
    }

    #[stq::query(sql = "SELECT * FROM orders WHERE placed_at > {since}")]
    pub struct RecentOrders {
        pub since: String,
    }

    /// Cross-module reference: this query lives in `orders.rs` but reads the
    /// `users` table defined in `users.rs`.
    #[stq::query(sql = "SELECT email FROM users WHERE id = {user_id}")]
    pub struct OrderOwnerEmail {
        pub user_id: i64,
    }

    /// Cross-crate reference: `accounts` is defined in `demo-upstream`.
    #[stq::query(sql = "SELECT * FROM accounts WHERE id = {account_id}")]
    pub struct AccountForOrder {
        pub account_id: i64,
    }
}
