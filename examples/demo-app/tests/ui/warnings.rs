// G7: warnings emitted by plugin rules appear at the user's code, as
// `deprecated` warnings. The expected stderr has no `error`, so this case
// must compile.
//
//  - stq-lite's global handler warns about a table no query reads; its span
//    went through collect, the carrier, expand! and __finish.
//  - registry's local handler warns about a handler name with uppercase
//    letters.

#[demo_macros::collect]
pub mod audit {
    #[stq::table(name = "audit_log")]
    pub struct AuditLog {
        pub id: i64,
    }

    #[registry::handler("Shout")]
    pub fn shout(input: &str) -> String {
        input.to_uppercase()
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::audit);
}
