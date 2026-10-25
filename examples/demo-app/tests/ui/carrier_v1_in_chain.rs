// The same v1 carrier as in carrier_version_mismatch.rs, but followed by
// another source. A v1 carrier does not chain: it hands `crate::users ,`
// back to `__finish` instead of calling `crate::users::__pl_carrier!`, so the
// list arrives unfinished. `__finish` names the source whose carrier did
// that and both versions (R10) instead of failing on the leftover tokens.

pub mod legacy {
    macro_rules! __pl_carrier_legacy {
        ($cb:path [$($pre:tt)*] { $($acc:tt)* } [$($seg:tt)*] $($rest:tt)*) => {
            $cb! {
                $($pre)*
                { $($acc)* source [ $($seg)* ] { ver 1 ; } }
                $($rest)*
            }
        };
    }
    pub(crate) use __pl_carrier_legacy as __pl_carrier;
}

#[demo_macros::collect]
pub mod users {
    #[stq::table(name = "users")]
    pub struct Users {
        pub id: i64,
    }
}

pub mod plan {
    plenary::expand!(demo_macros; crate::legacy, crate::users);
}
