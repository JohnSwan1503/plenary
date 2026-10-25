// A query against a table this collection knows only through a stage
// (`demo_upstream::plan`, SPEC section 10), with a misspelled column. The
// error must point at the `sql = ".."` literal in THIS file: the table came
// in as an exported fact (one more hop than a record), the query's span
// still survived collect -> carrier -> expand! -> __finish.

#[demo_macros::collect]
pub mod reports {
    #[stq::query(sql = "SELECT ownr_email FROM accounts WHERE id = {id}")]
    pub struct OwnerOf {
        pub id: i64,
    }
}

pub mod plan {
    plenary::expand!(demo_macros; demo_upstream::plan, crate::reports);
}
