// What the user sees when a collection is deeper than `recursion_limit`.
//
// Expansion depth of a collection = 1 (`expand!`) + one per source (its
// carrier) + one per conditional record (its `#[cfg]` carrier pair) + 1
// (`__finish`) + the deepest macro nesting inside what the plugins emit
// (0 here; stq-lite's type obligations add 1). It must not exceed
// `recursion_limit`. With the limit at 15, 13 sources fit (1 + 13 + 1 = 15)
// and these 14 do not: the 16th expansion trips the limit.
//
// `__finish` never runs when the chain is cut short, so plenary cannot
// report this itself; rustc does, with its own help text, naming the macro
// that tripped the limit. Here that is `__finish!` and the error points at
// the `expand!` invocation. When a carrier trips it instead (the list is
// longer still), rustc names `crate::<source>::__pl_carrier!` and points at
// the previous source's `#[collect]` attribute, whose span the chaining call
// inside that carrier carries, with `expand!` as the "in this macro
// invocation" note.
//
// THE FIX: raise the limit in this crate's root, i.e. replace the attribute
// below with `#![recursion_limit = "32"]` (anything >= 1 + 14 + 1 works; the
// README recommends "1024" for big collections).
#![recursion_limit = "15"]

#[demo_macros::collect] pub mod s0 { #[registry::handler("h0")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s1 { #[registry::handler("h1")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s2 { #[registry::handler("h2")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s3 { #[registry::handler("h3")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s4 { #[registry::handler("h4")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s5 { #[registry::handler("h5")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s6 { #[registry::handler("h6")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s7 { #[registry::handler("h7")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s8 { #[registry::handler("h8")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s9 { #[registry::handler("h9")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s10 { #[registry::handler("h10")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s11 { #[registry::handler("h11")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s12 { #[registry::handler("h12")] pub fn h(input: &str) -> String { input.to_string() } }
#[demo_macros::collect] pub mod s13 { #[registry::handler("h13")] pub fn h(input: &str) -> String { input.to_string() } }

pub mod plan {
    plenary::expand!(demo_macros; crate::s0, crate::s1, crate::s2, crate::s3, crate::s4, crate::s5, crate::s6, crate::s7, crate::s8, crate::s9, crate::s10, crate::s11, crate::s12, crate::s13);
}
