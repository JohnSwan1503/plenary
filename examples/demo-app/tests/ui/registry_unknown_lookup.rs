// A lookup of a name nothing registered, with a close match available.

#[demo_macros::collect]
pub mod h {
    #[registry::handler("greet")]
    pub fn greet(input: &str) -> String {
        format!("hello, {input}")
    }

    pub fn call(input: &str) -> String {
        registry::lookup!("gret")(input)
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::h);
}
