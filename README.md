# plenary

A framework for **safe stateful macro expansion** on stable Rust. One standard
way to introduce state (records inside *sources*), one standard way to consume
it (*collections*), and plugins that do the actual work. The name says how it
works: a plenary session is one where every member is present; a collection is
a plenary expansion, where every record is present before anything is decided.
`SPEC.md` is the contract; this file is the quick start.

```
#[demo_macros::collect] pub mod users    { #[stq::table(..)] struct Users; .. }   ─┐  each source gets a
#[demo_macros::collect] pub mod handlers { #[registry::handler("greet")] fn greet; } ├─ "carrier": a macro_rules!
#[demo_macros::collect(export)] pub mod accounts { .. }     (in another crate)      ─┘  holding its records as tokens
                 │
                 ▼
pub mod plan { plenary::expand!(demo_macros; crate::users::users, crate::handlers::handlers, demo_upstream::accounts); }
                 │   expand! calls the first carrier; each carrier appends its records to an
                 │   accumulator and calls the next listed carrier itself (continuation-passing) ...
                 ▼
demo_macros::__finish! { plenary = [$crate] bundle = [demo_macros] { source [crate::users::users] { ver 2; rec {..} .. } .. } }
                 │   ... and every plugin's global handlers see all records at once:
                 ▼
impl stq_lite::Query<__Collection> for crate::users::users::OrgUsers { const SQL: &str = "SELECT id, email FROM users WHERE org_id = $1"; }
pub enum Handler { Greet = 0, Shout = 1, .. }   impl Handler { pub const ALL: ..; pub fn call(self, &str) -> String; .. }
pub static HANDLERS: &[(&str, fn(&str) -> String)] = &[("greet", __PlCoerce0), ..];
pub fn dispatch(name: &str, input: &str) -> Option<String> { Handler::from_name(name).map(|h| h.call(input)) }
```

Everything a global handler sees arrives as tokens through the normal
expansion of the crate being compiled, so the compiler's rules for when to
expand again are always right: no hidden state, no stale results, no
dependence on expansion order.

A collection is also a **stage**: it exports what its handlers worked out,
and a later `expand!` can list it like a source (see *Stages* below).

## Layout

| path | what |
|---|---|
| `crates/plenary` | the facade apps depend on: `expand!`, the `Collection` trait, `CARRIER_FORMAT_VERSION`. No syn. |
| `crates/plenary-plugin` | the plugin API (`Plugin`, `Record`, `State`, `LocalCx`, `GlobalCx`, diagnostics, obligations, markers, `ExportFact`/`Imported` and the `facts` codecs) and the engine (collector, carrier format, handler ordering, panic capture, finish, rounds and stage carriers), plus `bundle!` |
| `examples/registry`, `examples/registry-plugin` | a non-SQL plugin: `#[registry::handler("name")]` functions, build-time `registry::lookup!("name")` / `registry::variant!("name")`, and the closed `enum Handler` at every collection site |
| `examples/stq-lite`, `examples/stq-lite-plugin` | the stq prototype as a plugin: `#[stq::table]`, `#[stq::query]`, inline `stq::sql!`, column checks, `SELECT *` expansion, type obligations |
| `examples/stats-plugin` | reads the other two plugins' facts and emits `SUMMARY` |
| `examples/demo-macros` | the bundle: `bundle!(stats_plugin::plugin, registry_plugin::plugin, stq_lite_plugin::plugin)` |
| `examples/demo-upstream` | a library exporting sources and running its own collection as an exported stage (`expand!(demo_macros; export; ..)`) |
| `examples/demo-app` | sources in six files + the upstream sources; three collections: `plan` (raw sources), `plan2` (consumes the upstream *stage*), `plan3` (lists `plan` as a stage: a same-crate round); a cargo feature `extra`; tests in `tests/rendered.rs`, UI cases in `tests/ui/` |
| `examples/ui-support/` | plugins and bundles that fail on purpose (panic, fact cycle, missing producer, unregistered export) |
| `scripts/invalidation_demo.sh` | edits one file and shows another file's build result change (G3), including through a stage (G8) |
| `scripts/scale_probe.sh` | where `recursion_limit` bites (125/126 sources at the default limit, 1021/1022 at 1024), and that raising it works |
| `vendor/`, `.cargo/config.toml` | proc-macro2/quote/syn/unicode-ident, vendored because crates.io was unreachable where this was built; delete both to use crates.io |

```
cargo test --workspace                       # engine tests, plugin unit tests, demo-app tests, 24 UI cases
cargo test -p demo-app --features extra      # the feature-gated handler shows up
UI_OVERWRITE=1 cargo test -p demo-app --test ui   # regenerate tests/ui/*.stderr
./scripts/invalidation_demo.sh
./scripts/scale_probe.sh
```

## For application users

1. Depend on `plenary`, your project's bundle crate, and the plugins'
   runtime crates (`stq-lite`, `registry`, ...).
2. Mark inline modules as sources and tag items or macro calls with a plugin
   namespace. Sources must be inline (`mod users { .. }` inside `users.rs`):
   file modules in proc-macro input are unstable.

   ```rust
   #[demo_macros::collect]
   pub mod users {
       #[stq::table(name = "users")]
       pub struct Users { pub id: i64, pub email: String }

       #[stq::query(sql = "SELECT id, email FROM users WHERE org_id = {org_id}")]
       pub struct OrgUsers { pub org_id: i64 }

       pub fn count_sql() -> &'static str { stq::sql!("SELECT id FROM users") }
   }
   ```

   `collect(export)` makes the source usable from other crates.
3. Expand the sources, once, where the results should live:

   ```rust
   pub mod plan {
       plenary::expand!(demo_macros; crate::users::users, crate::handlers::handlers, demo_upstream::accounts);
   }
   ```

   Local sources are spelled from `crate`, `self` or `super`; sources from
   other crates from the crate name. A collection emits a marker type
   `plan::__Collection` plus whatever the plugins produce (`plan::dispatch`,
   `impl stq_lite::Query<plan::__Collection> for ..`, `plan::SUMMARY`, ...).
4. A source that uses markers (`stq::sql!`, `registry::lookup!`) must be in
   exactly one collection of its own crate.
5. Errors and warnings point at your code. Warnings are `deprecated`
   warnings, so `#[allow(deprecated)]` hides them and `-D warnings` denies
   them.
6. Expansion depth: a collection costs `1 + sources + conditional records + 1`
   levels (`expand!`, one carrier per source, one `#[cfg]` carrier per
   conditional record, `__finish`), plus the deepest macro nesting inside
   what the plugins emit (stq-lite's type obligations and the registry's
   `#[derive]` on `Handler` add 1). It must not
   exceed `recursion_limit`. Measured on rustc 1.97 with the default limit of
   128: 125 plain sources build, 126 fail; 62 sources with one conditional
   record each; 41 with two. Past that, add `#![recursion_limit = "1024"]`
   (1021 sources). The framework cannot catch this itself (`__finish` never
   runs when the chain is cut short); rustc reports
   `recursion limit reached while expanding ...` with a `help:` naming the
   attribute to add, see `tests/ui/recursion_limit.rs`.

## Stages

Every `expand!` also leaves a carrier in its module, so another `expand!`
can list the collection like a source. What it gets is not the records the
first collection saw, but what that collection *worked out*:

- the facts its global handlers exported (`cx.export(&resolved)`), and
- the records found in the items it emitted (a handler that emits
  `#[registry::handler("generated_echo")] pub fn ..` has created a record that
  the next collection will see).

```rust
// demo-upstream: an exporting collection, usable from other crates
pub mod plan {
    plenary::expand!(demo_macros; export; crate::accounts, crate::upstream_handlers);
}

// demo-app: no raw upstream source listed, only the stage
pub mod plan2 {
    plenary::expand!(demo_macros; demo_upstream::plan, crate::staged::staged);
}
```

In `plan2`, a `#[stq::query(sql = "SELECT * FROM accounts ..")]` resolves
against the `accounts` table that arrived as an exported fact, including the
column type checks (the imported column types are paths into demo-upstream's
type aliases, rebased from `crate::accounts::..` to `demo_upstream::accounts::..`),
and `plan2::dispatch("upstream_echo", ..)` works through the imported handler
index. `plan3` lists `crate::plan` and can look up `generated_echo`, the
handler `plan`'s registry generated.

Rules of the road (SPEC section 10):

- Within one collection, emitted items are not seen by that collection's own
  handlers: one round per collection, no fixpoint.
- `&Imported<F>` is always available to a handler (possibly empty), ordered by
  stage position then export order; `&F` still means "produced here".
- Without `export`, only collections of the same crate can list the stage.
  One `expand!` per module.
- A stage costs one expansion level, like a source.
- Changing anything upstream of a stage changes the stage carrier's tokens, so
  everything listing it re-expands (G8; `scripts/invalidation_demo.sh`).

For plugin authors: make the fact type exportable and register it.

```rust
use plenary_plugin::facts::{Decode, Encode, Fields};

impl plenary_plugin::ExportFact for Index {
    const NAME: &'static str = "Index";
    fn to_tokens(&self) -> TokenStream { Fields::new().with("handlers", &self.handlers).encode() }
    fn from_tokens(tokens: TokenStream) -> Result<Self> {
        let f = Fields::decode(tokens)?;
        Ok(Index { handlers: f.get("handlers")? })
    }
}
// nested structs implement `Encode`/`Decode` the same way (strings, integers,
// bools, Vec, Option, tuples, syn::Path, Ident, LitStr and Span are built in;
// a LitStr or Span keeps its span, so downstream errors can point upstream)

Plugin::new("registry", ..).exportable::<Index>().global(index) ..

fn index(state: &State, upstream: &Imported<Index>, cx: &mut GlobalCx) -> Result<Index> {
    let mut handlers = Vec::new();
    for (stage, imported) in upstream.iter() { handlers.extend(..) }   // imported first
    for r in state.records_of("registry", "handler") { .. }            // then local records
    let index = Index { handlers };
    cx.export(&index);                                                  // for the next stage
    Ok(index)
}
```

Paths inside exported facts need no special care: the engine rebases
`crate::`/`self::`/`super::` paths to the importing collection's view. Spans
survive when you encode a `LitStr` or a `Span`. Exporting a type nobody
registered is an error naming the plugin and the type; a fact from a plugin
of another major version is an error naming the stage and both versions; a
fact type the bundle does not know is ignored.

## Closed sets

A whole-program pass can mint a closed type over an open set of
declarations, which no run-time registry can. `plenary_plugin::gen::ClosedEnum`
is the standard way (SPEC section 11): the registry plugin emits, at every
collection site,

```rust
#[repr(u16)] #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Handler { Greet = 0, Shout = 1, CountUsersSql = 2, UpstreamEcho = 3 }
impl Handler {
    pub const ALL: &[Self]; pub const COUNT: usize;
    pub const fn name(self) -> &'static str; pub fn from_name(&str) -> Option<Self>;
    pub const fn id(self) -> u16; pub const fn function(self) -> fn(&str) -> String;
    pub fn call(self, input: &str) -> String;
}
impl TryFrom<u16> for Handler { .. }
pub fn dispatch(name: &str, input: &str) -> Option<String> { Handler::from_name(name).map(|h| h.call(input)) }
```

so `match handler { .. }` in `demo-app/src/consumers.rs` is checked for
exhaustiveness: add a `#[registry::handler]` anywhere and the match stops
compiling until it has an arm (`tests/ui/registry_nonexhaustive_match.rs`
shows rustc's `non-exhaustive patterns` error naming the variant). Names that
make no identifier, or two names that make the same variant, are errors
citing the records (`tests/ui/registry_variant_collision.rs`).
`registry::variant!("shout")` resolves to `Handler::Shout` at build time, next
to `registry::lookup!("shout")` which still gives the function pointer.

```rust
let mut e = gen::ClosedEnum::new(Ident::new("Handler", cx.site()));
for h in &index.handlers { e.entry(&h.name, h.name_span); }
e.payload(format_ident!("function"), quote!(fn(&str) -> String), checked_consts);
let built = e.build();                 // tokens, diagnostics, (name, variant) mapping
for d in built.diagnostics { cx.report(d); }
cx.emit(built.tokens);
```

## For bundles

A bundle is a proc-macro crate whose entire source is one line:

```toml
[lib]
proc-macro = true

[dependencies]
plenary-plugin = "0.1"
stq-lite-plugin = "0.1"
registry-plugin = "0.1"
```

```rust
plenary_plugin::bundle!(registry_plugin::plugin, stq_lite_plugin::plugin);
```

It generates `#[collect]` and the hidden `__finish!`. List order only breaks
ties between global handlers with no dependency between them; who runs before
whom comes from handler signatures (R6).

## For plugin authors

A plugin is an ordinary library crate exposing `pub fn plugin() -> Plugin`.
It owns a namespace, registers record kinds with their *local* handlers, and
registers *global* handlers. The engine is written against `proc_macro2`, so
all of it is unit-testable with `plenary_plugin::testing` and no compiler.

```rust
use plenary_plugin::{GlobalCx, LocalCx, Plugin, Record, Result, State};
use quote::quote;

pub fn plugin() -> Plugin {
    Plugin::new("registry", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        .attribute_kind("handler", handler)   // #[registry::handler("name")] fn ..
        .call_kind("lookup", lookup)          // registry::lookup!("name")
        .global(index)                        // fn(&State, &mut GlobalCx) -> Result<Index>
        .global(emit)                         // fn(&Index, &mut GlobalCx) -> Result<()>
}

// Local handlers run inside `collect`, once per record, and see only that record.
fn handler(record: &Record, cx: &mut LocalCx) -> Result<()> {
    let name: syn::LitStr = record.parse_args()?;     // the attribute's arguments
    let item: syn::ItemFn = record.parse_item()?;     // the item, plugin attributes still on it
    cx.annotate("name", &name);                       // travels with the record
    cx.add_item(quote! { .. });                       // items next to the item, in the source
    Ok(())                                            // the item stays, attributes stripped
}

fn lookup(record: &Record, cx: &mut LocalCx) -> Result<()> {
    let name: syn::LitStr = record.parse_args()?;
    let marker = cx.marker();                         // a type at the source root (R4)
    cx.annotate("name", &name);
    cx.set_replacement(quote!(<#marker as ::registry::Lookup>::FN));
    Ok(())
}

// Global handlers run inside `__finish`, once per collection, in dependency order.
pub struct Index { pub names: Vec<(String, syn::Path)> }
impl plenary_plugin::Fact for Index {}

fn index(state: &State, cx: &mut GlobalCx) -> Result<Index> {
    let mut names = Vec::new();
    for r in state.records_of("registry", "handler") {
        if r.is_tainted() { continue; }               // its local handler reported an error
        let name: syn::LitStr = r.annotation_as("name").unwrap()?;
        names.push((name.value(), r.absolute_path()));  // crate::handlers::handlers::greet
    }
    Ok(Index { names })
}

fn emit(index: &Index, cx: &mut GlobalCx) -> Result<()> {
    for (name, path) in &index.names {
        let checked = cx.require_coerces(path, quote!(fn(&str) -> String), path.span()); // obligation
        cx.emit(quote! { .. });                       // items at the collection site
    }
    cx.warn(span, "..");                              // a warning at the user's code
    Ok(())
}
```

Rules of thumb:

- A record's tokens mean what they mean *where they are emitted* (R3). From a
  collection, refer to things in a source through `record.absolute_path()`
  or through handles the local handler created with `cx.add_item` (see the
  `__pl_ty_*` type aliases in stq-lite) or `cx.marker()`.
- Only implement traits for markers of local sources
  (`state.source_of(record).is_local()`), with a trait that has no collection
  parameter. For types from any crate, use a trait with a type parameter and
  pass `cx.collection()` (`__Collection`).
- Facts that only the compiler knows (types, impls, const values) go through
  obligations: `require_same_type`, `require_impl`, `require_coerces`,
  `require_const`.
- Report many errors with `cx.error(..)`; return `Err` only to abort the
  handler (its emitted items are dropped, dependents are skipped).
- No files, env vars, clocks or hash-map iteration order in your output (R8).
- Tests: `testing::collect(plugins, module_tokens)` gives you the records a
  collection would see; `testing::state(..)` builds a `State`;
  `testing::global_cx_with(plugins)` gives a context whose output (items,
  obligations, diagnostics, exports) you can inspect; `testing::imported()` /
  `testing::imported_from(..)` build an `Imported<F>`; `testing::finish(..)`
  runs the whole pipeline and `Finished::stage()` decodes the stage carrier it
  emitted; `testing::finish_blocks(..)` lets you list stages with facts.

## Protocol (carrier format v2)

`expand!` only starts the chain. It calls the first listed source's carrier
and hands it the callback (the bundle's `__finish`), a prefix, an empty
accumulator, the source's own path and the rest of the list:

```
<first>::__pl_carrier! { <bundle>::__finish [ plenary = [ $crate ] bundle = [ <bundle> ] ] { } [ <first> ] <second> , <third> , }
```

A carrier appends its source block to the accumulator, after chaining through
one per-record carrier for each conditional record (a `#[cfg(c)]` /
`#[cfg(not(c))]` pair, so the compiler decides whether the record is in the
state, R7). Then, if the rest of the list begins with `path ,`, it calls that
source's carrier directly:

```
<second>::__pl_carrier! { <callback> [ <prefix> ] { source [ <first> ] { ver 2 ; plugin "stq" "stq-lite-plugin" "0.1.0" ; rec { .. } .. } } [ <second> ] <third> , }
```

and if the rest is empty (or a lone trailing comma) it calls the callback:

```
<bundle>::__finish! { plenary = [ $crate ] bundle = [ <bundle> ] { source [ <first> ] { .. } source [ <second> ] { .. } .. } }
```

One expansion level per source (v1 used two: `expand!` called every carrier
and every carrier called `expand!` back). Identifiers and literals in a
record are the user's original tokens, so their spans point back into the
user's file after any number of `macro_rules!` hops. See
`crates/plenary-plugin/src/carrier.rs` for the record layout. Hidden names
start with `__pl_` (reserved); the hash in them covers the module's name,
tokens and `file:line`, so two identical exported modules in one crate don't
clash (callers only ever use the `__pl_carrier` alias next to the module).
