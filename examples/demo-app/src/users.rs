#[demo_macros::collect]
pub mod users {
    /// A type alias: the column's type is spelled `UserId` here and the
    /// collection still checks parameters against it (R3 handles, obligations).
    pub type UserId = i64;

    #[stq::table(name = "users")]
    pub struct Users {
        pub id: UserId,
        pub email: String,
        pub org_id: i64,
    }

    #[stq::query(sql = "SELECT id, email FROM users WHERE org_id = {org_id}")]
    pub struct OrgUsers {
        pub org_id: i64,
    }

    #[stq::query(sql = "SELECT * FROM users")]
    pub struct AllUsers {}

    /// Records inside a nested inline module work too.
    pub mod admin {
        #[stq::query(sql = "SELECT email FROM users WHERE id = {id}")]
        pub struct EmailById {
            pub id: i64,
        }
    }
}
