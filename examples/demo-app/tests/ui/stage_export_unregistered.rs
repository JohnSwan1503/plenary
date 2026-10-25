// A global handler exports a fact type that no plugin registered with
// `.exportable::<..>()` (SPEC section 10). The error names the plugin and the
// type and points at the collection site.

pub mod plan {
    plenary::expand!(export_bundle;);
}
