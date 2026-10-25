// A handler generated inside demo-upstream's collection (`generated_echo`,
// a record of the stage `demo_upstream::plan`, SPEC section 10) collides
// with a local handler of the same name. The error is at the local name and
// cites the generated one; the note's location is where demo-upstream's
// `expand!` sits, because that is where the generated item was emitted.

#[demo_macros::collect]
pub mod h {
    #[registry::handler("generated_echo")]
    pub fn mine(input: &str) -> String {
        input.into()
    }
}

pub mod plan {
    plenary::expand!(demo_macros; demo_upstream::plan, crate::h);
}
