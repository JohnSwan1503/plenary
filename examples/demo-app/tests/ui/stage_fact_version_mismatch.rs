// A stage exporting a fact built by another major version of a plugin (R10
// for exported facts, SPEC section 10): the error names the stage, the fact,
// the plugin and both versions, and points at the listed stage path.
//
// The carrier is written by hand: its `plugin` line matches this bundle (so
// the stage-level R10 check passes) and only the fact's `ver` differs, which
// is the situation the fact-level check exists for. In a real build the two
// agree, and the stage-level check reports first with
// `stage `..` was collected by plugin `stq-lite-plugin` v9.0.0, but this
// bundle has `stq-lite-plugin` v0.1.0 (major versions differ)` (covered by
// the engine tests).

pub mod old_plan {
    macro_rules! __pl_carrier_old_plan {
        ($cb:path [$($pre:tt)*] { $($acc:tt)* } [$($seg:tt)*] $(,)?) => {
            $cb! {
                $($pre)*
                { $($acc)* source [ $($seg)* ] {
                    ver 2 ; stage ;
                    plugin "stq" "stq-lite-plugin" "0.1.0" ;
                    fact { ns "stq" type "Resolved" ver "9.0.0" { { tables : [] , queries : [] , inline : [] , } } }
                } }
            }
        };
    }
    pub(crate) use __pl_carrier_old_plan as __pl_carrier;
}

pub mod plan {
    plenary::expand!(demo_macros; crate::old_plan);
}
