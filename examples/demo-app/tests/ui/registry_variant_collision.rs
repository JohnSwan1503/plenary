// Handler names that can't become variants of the closed `Handler` enum
// (SPEC section 11): `greet` and `Greet` map to the same variant (the error
// cites both), and `hello-world` is not an identifier. The handlers still
// exist by name (`dispatch`, `HANDLERS`); only the enum leaves them out.
// (`Greet` also gets the registry's own warning about uppercase names.)

#[demo_macros::collect]
pub mod h {
    #[registry::handler("greet")]
    pub fn greet(input: &str) -> String {
        format!("hello, {input}")
    }

    #[registry::handler("Greet")]
    pub fn greet_loud(input: &str) -> String {
        format!("HELLO, {input}")
    }

    #[registry::handler("hello-world")]
    pub fn hello_world(_input: &str) -> String {
        "hello, world".into()
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::h);
}
