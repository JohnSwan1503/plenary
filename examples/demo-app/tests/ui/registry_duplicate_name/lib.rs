// Two handlers with the same name in two different files. The error points
// at the second one and names the first.

pub mod a;
pub mod b;

pub mod plan {
    plenary::expand!(demo_macros; crate::a::handlers_a, crate::b::handlers_b);
}
