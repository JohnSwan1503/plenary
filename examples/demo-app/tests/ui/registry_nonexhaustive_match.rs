// The closed set at work (SPEC section 11): `plan::Handler` has one variant
// per handler the collection knows, so a `match` that forgets one is rustc's
// own `non-exhaustive patterns` error, naming the missing variant. Nothing
// else (a run-time registry, linkme, inventory) can give this check.

#[demo_macros::collect]
pub mod h {
    #[registry::handler("greet")]
    pub fn greet(input: &str) -> String {
        format!("hello, {input}")
    }

    #[registry::handler("shout")]
    pub fn shout(input: &str) -> String {
        input.to_uppercase()
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::h);
}

pub fn describe(handler: plan::Handler) -> &'static str {
    match handler {
        plan::Handler::Greet => "greets",
    }
}
