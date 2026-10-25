// A query referencing a table no listed source defines. The upstream crate's
// `accounts` is listed, so the message should mention it as visible.

#[demo_macros::collect]
pub mod users {
    #[stq::table(name = "users")]
    pub struct Users {
        pub id: i64,
    }

    #[stq::query(sql = "SELECT id FROM userz")]
    pub struct Oops {}
}

pub mod plan {
    plenary::expand!(demo_macros; crate::users, demo_upstream::accounts);
}
