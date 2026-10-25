//! Deliberately broken plugins for the UI (compile-fail) tests.

use plenary_plugin::{keep, ExportFact, GlobalCx, Plugin, Result, State};

/// A plugin whose global handler panics (R11).
pub mod panicking {
    use super::*;

    fn explode(_state: &State, _cx: &mut GlobalCx) -> Result<()> {
        panic!("the handler tripped over record {}", 7)
    }

    pub fn plugin() -> Plugin {
        Plugin::new("boom", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .attribute_kind("item", keep)
            .global(explode)
    }
}

/// Two handlers that each need the other's fact (R6).
pub mod cycle {
    use super::*;

    pub struct Left;
    impl plenary_plugin::Fact for Left {}
    pub struct Right;
    impl plenary_plugin::Fact for Right {}

    fn left(_right: &Right, _cx: &mut GlobalCx) -> Result<Left> {
        Ok(Left)
    }
    fn right(_left: &Left, _cx: &mut GlobalCx) -> Result<Right> {
        Ok(Right)
    }

    pub fn plugin() -> Plugin {
        Plugin::new("cyc", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).global(left).global(right)
    }
}

/// A handler that reads a fact nothing produces (R6).
pub mod missing {
    use super::*;

    pub struct Orphan;
    impl plenary_plugin::Fact for Orphan {}

    fn needs_orphan(_orphan: &Orphan, _cx: &mut GlobalCx) -> Result<()> {
        Ok(())
    }

    pub fn plugin() -> Plugin {
        Plugin::new("mis", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).global(needs_orphan)
    }
}

/// A handler that exports a fact type nobody registered (SPEC section 10).
pub mod unregistered_export {
    use super::*;

    pub struct Secret;
    impl plenary_plugin::Fact for Secret {}
    impl ExportFact for Secret {
        const NAME: &'static str = "Secret";
        fn to_tokens(&self) -> proc_macro2::TokenStream {
            Default::default()
        }
        fn from_tokens(_: proc_macro2::TokenStream) -> Result<Self> {
            Ok(Secret)
        }
    }

    fn leak(_state: &State, cx: &mut GlobalCx) -> Result<()> {
        cx.export(&Secret);
        Ok(())
    }

    pub fn plugin() -> Plugin {
        Plugin::new("leaky", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).attribute_kind("item", keep).global(leak)
    }
}
