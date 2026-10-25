//! One source using both plugins, with call-form records inside bodies.

#[demo_macros::collect]
pub mod mixed {
    #[registry::handler("count_users_sql")]
    pub fn count_users_sql(_input: &str) -> String {
        // Inline SQL checked against `users` from users.rs; `SELECT *` is
        // expanded to the table's columns at build time.
        let sql: &'static str = stq::sql!("SELECT * FROM users WHERE org_id = {org}");
        sql.to_string()
    }

    pub fn greet_via_lookup(input: &str) -> String {
        registry::lookup!("greet")(input)
    }

    /// A nested module with its own lookup, to exercise `super::` marker paths.
    pub mod deeper {
        pub fn upstream(input: &str) -> String {
            registry::lookup!("upstream_echo")(input)
        }
    }
}
