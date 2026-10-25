// G5: a handler name used twice, but one of them is compiled out. The
// compiled-out record is not in the state, so there is no duplicate.
// (No .stderr file: this must compile cleanly.)

#[demo_macros::collect]
pub mod h {
    #[registry::handler("greet")]
    pub fn greet(input: &str) -> String {
        format!("hello, {input}")
    }

    #[cfg(any())]
    #[registry::handler("greet")]
    pub fn greet_disabled(input: &str) -> String {
        format!("never, {input}")
    }

    #[cfg(all())]
    #[registry::handler("always")]
    pub fn always(input: &str) -> String {
        format!("always, {input}")
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::h);
}

pub fn check() {
    assert_eq!(plan::HANDLERS.len(), 2);
}
