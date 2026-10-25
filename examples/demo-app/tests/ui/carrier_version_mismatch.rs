// A carrier produced by an older plenary than the one expanding (R10).
// This is what a `collect` of carrier format v1 emitted: the block layout is
// the same as today's, but it says `ver 1` and follows the v1 calling
// convention (it hands the rest of the list back to the callback instead of
// chaining to the next carrier). As the only listed source it reaches
// `__finish`, which reports the version mismatch and names the source.

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

pub mod plan {
    plenary::expand!(demo_macros; crate::legacy);
}
