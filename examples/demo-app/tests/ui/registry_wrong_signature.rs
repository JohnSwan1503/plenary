// A handler with the wrong signature. The collection emits
// `const _: fn(&str) -> String = crate::h::h::bad;` spanned at the user's
// `fn`, so the compiler reports the mismatch there.

#[demo_macros::collect]
pub mod h {
    #[registry::handler("bad")]
    pub fn bad(input: &str, times: usize) -> String {
        input.repeat(times)
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::h);
}
