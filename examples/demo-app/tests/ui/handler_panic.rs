// A global handler that panics (R11): the error names the plugin, the
// handler and the panic message, and nothing else goes wrong.

#[panic_bundle::collect]
pub mod things {
    #[boom::item]
    pub struct Thing;
}

pub mod plan {
    plenary::expand!(panic_bundle; crate::things);
}
