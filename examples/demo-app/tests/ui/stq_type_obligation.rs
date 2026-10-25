// Two obligations the compiler proves at the user's code:
//  - `require_same_type`: a parameter compared with a column must have the
//    column's type. `users.id` is `UserId` (an alias of i64) and the
//    parameter is `i32`, so the check fails. The error points at the
//    parameter's type.
//  - `require_impl`: parameters must implement `stq_lite::Param`;
//    `Vec<u8>` doesn't.

#[demo_macros::collect]
pub mod users {
    pub type UserId = i64;

    #[stq::table(name = "users")]
    pub struct Users {
        pub id: UserId,
        pub email: String,
    }

    #[stq::query(sql = "SELECT email FROM users WHERE id = {user_id}")]
    pub struct EmailById {
        pub user_id: i32,
    }

    #[stq::query(sql = "SELECT id FROM users WHERE email = {email}")]
    pub struct IdByEmail {
        pub email: Vec<u8>,
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::users);
}
