#[demo_macros::collect]
pub mod handlers {
    #[registry::handler("greet")]
    pub fn greet(input: &str) -> String {
        format!("hello, {input}")
    }

    #[registry::handler("shout")]
    pub fn shout(input: &str) -> String {
        input.to_uppercase()
    }

    /// Only part of the registry when the `extra` feature is on (G5). The
    /// record travels through its own pair of carriers; the compiler picks
    /// the live one.
    #[cfg(feature = "extra")]
    #[registry::handler("extra")]
    pub fn extra(input: &str) -> String {
        format!("extra: {input}")
    }

    /// A lookup inside a function body: resolved at build time to `shout`'s
    /// function pointer through a marker (R4).
    pub fn shout_twice(input: &str) -> String {
        let f = registry::lookup!("shout");
        let once = f(input);
        f(&once)
    }

    /// The same name as a variant of the collection's closed `Handler` enum
    /// (R4: a source can *name* what the collection defines).
    pub fn shout_handler() -> crate::plan::Handler {
        registry::variant!("shout")
    }
}
