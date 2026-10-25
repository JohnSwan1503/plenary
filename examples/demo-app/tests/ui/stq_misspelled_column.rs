// A query referencing a column that doesn't exist on the table. The table is
// in another source, so only the collection can catch it. The error must
// point at the `sql = "..."` literal in THIS file: the span survives
// collect (proc) -> carrier (macro_rules) -> expand! (several hops) ->
// __finish (proc).

#[demo_macros::collect]
pub mod users {
    #[stq::table(name = "users")]
    pub struct Users {
        pub id: i64,
        pub email: String,
    }
}

#[demo_macros::collect]
pub mod orders {
    #[stq::query(sql = "SELECT * FROM users WHERE emial = {email}")]
    pub struct ByEmail {
        pub email: String,
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::users, crate::orders);
}
