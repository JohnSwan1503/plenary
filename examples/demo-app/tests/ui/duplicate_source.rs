// The same source listed twice in one expand!.

#[demo_macros::collect]
pub mod users {
    #[stq::table(name = "users")]
    pub struct Users {
        pub id: i64,
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::users, crate::users);
}
