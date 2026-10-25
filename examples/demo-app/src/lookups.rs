//! A source listed only by `plan3`, which lists this crate's first collection
//! (`crate::plan`) as a stage. `generated_echo` is a handler the registry
//! plugin *generated* inside `plan`; it is a record of `plan`'s stage (one
//! round per collection), so `plan3` can resolve a lookup for it.

#[demo_macros::collect]
pub mod lookups {
    pub fn generated(input: &str) -> String {
        registry::lookup!("generated_echo")(input)
    }

    /// A handler known to `plan3` through the stage (it is in `plan`'s index).
    pub fn upstream_via_stage(input: &str) -> String {
        registry::lookup!("upstream_echo")(input)
    }
}
