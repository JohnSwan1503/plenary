// An unknown kind inside a known namespace is an error at the record, with a
// suggestion (R9).

#[demo_macros::collect]
pub mod users {
    #[stq::tabel(name = "users")]
    pub struct Users {
        pub id: i64,
    }
}
