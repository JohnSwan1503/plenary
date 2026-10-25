// v0 limit (R7): `cfg_attr` may not wrap a `cfg` or a plugin attribute.

#[demo_macros::collect]
pub mod h {
    #[cfg_attr(unix, registry::handler("maybe"))]
    pub fn maybe(input: &str) -> String {
        input.to_string()
    }

    #[cfg_attr(unix, cfg(any()))]
    #[registry::handler("sometimes")]
    pub fn sometimes(input: &str) -> String {
        input.to_string()
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::h);
}
