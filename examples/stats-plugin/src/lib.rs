//! `stats`: a cross-plugin consumer. It reads `stq_lite_plugin::Resolved` and
//! `registry_plugin::Index` (by depending on those library crates) and emits
//! `pub const SUMMARY: &str` at the collection site.
//!
//! It owns no record kinds. Its one global handler depends on two facts, so
//! the engine must run it after `stq-lite`'s `link` and `registry`'s `index`
//! no matter where the plugin is listed in the bundle (R6).

use plenary_plugin::{GlobalCx, Plugin, Result};
use quote::quote;
use registry_plugin::Index;
use stq_lite_plugin::Resolved;

/// The plugin, for `bundle!`.
pub fn plugin() -> Plugin {
    Plugin::new("stats", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).global(summarize)
}

/// The text `summarize` emits for these counts.
pub fn summary(tables: usize, queries: usize, inline: usize, handlers: usize) -> String {
    format!("tables: {tables}, queries: {queries}, inline queries: {inline}, handlers: {handlers}")
}

fn summarize(resolved: &Resolved, index: &Index, cx: &mut GlobalCx) -> Result<()> {
    let text = summary(resolved.tables.len(), resolved.queries.len(), resolved.inline.len(), index.handlers.len());
    cx.emit(quote! {
        /// What this collection contains, as counted by the `stats` plugin.
        pub const SUMMARY: &str = #text;
    });
    Ok(())
}
