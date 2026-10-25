// A source can't contain another source (R12). The outer source is marked
// failed, so the collection skips it instead of failing on a missing macro.

#[demo_macros::collect]
pub mod outer {
    #[stq::table(name = "outer")]
    pub struct Outer {
        pub id: i64,
    }

    #[demo_macros::collect]
    pub mod inner {
        #[stq::table(name = "inner")]
        pub struct Inner {
            pub id: i64,
        }
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::outer);
}
