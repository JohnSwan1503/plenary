# Progress log

Plain-language record of what changed in each push, what was measured, and
what was logged in `SPEC.md` section 9. Baseline before any push:
`cargo test --workspace` = 53 passed, `cargo test -p demo-app --features
extra` = 15 passed, scale probe 62 OK / 63 fail (31/32 with two conditional
records per source) at the default `recursion_limit`.

## Push 5: two cheap fixes (done)

### 5a. Carriers chain directly (carrier format v2)

- `expand!` (`crates/plenary/src/lib.rs`) no longer walks the list. It
  calls the first source's carrier once and hands it the callback
  (`<bundle>::__finish`), the prefix (`plenary = [$crate] bundle = [..]`),
  an empty accumulator, the first path and the rest of the list with a
  trailing comma appended. An empty list goes straight to `__finish`.
- Every carrier (`collect.rs::carrier_macros`) now has two final rules: when
  the rest begins with `$($next:ident)::+ ,` it calls
  `$($next)::+::__pl_carrier!` with the accumulator plus its own block; when
  the rest is empty or a lone `,` it calls the callback. Conditional-record
  chains are unchanged inside a source; the *last* `__pl_rec_N` carrier of a
  source carries the two final rules. The source block is duplicated in the
  two rules (macro_rules can only branch by rule), which doubles the size of
  the carrier *definition*, not of anything expanded.
- `CARRIER_FORMAT_VERSION` is 2: the calling convention is part of the
  format. `__finish` (`carrier.rs::parse_finish_input`) accepts the trailing
  comma a v1 carrier leaves behind (so a lone v1 carrier gets the normal R10
  version error) and reports leftover source paths as a calling-convention
  mismatch naming the source and both versions, instead of a parse error.
- Measured (rustc 1.97, `scripts/scale_probe.sh`): depth =
  `1 + sources + conditional records + 1 + E`, where E is the deepest macro
  nesting inside the plugins' output (1 for stq-lite's
  `#[diagnostic::on_unimplemented]` obligation traits). Must be <=
  `recursion_limit`.
  - limit 128: 125 sources OK, 126 fail (was 62/63);
    1 conditional record per source: 62/63;
    2 conditional records per source: 41/42 (was 31/32).
  - limit 1024: 1021 OK, 1022 fail; 300 sources x 5 tables x 10 columns ~5 s.
- Build times: `cargo build -p demo-app` after touching `demo-app/src/lib.rs`
  ~0.45 s; from scratch (proc-macro2, quote, syn, the engine, the three
  plugins, the bundle, demo-upstream and demo-app) ~5.4 s. Unchanged in
  practice.

### 5b. The recursion limit

- No stable-Rust detection exists: when the chain is cut, `__finish` never
  runs. Documented the measured formula in README and SPEC section 6, and
  deviation 17 describes what rustc reports and where it points.
- New UI case `examples/demo-app/tests/ui/recursion_limit.rs`: 14 sources
  under `#![recursion_limit = "15"]`; the `.stderr` pins rustc's text
  (`recursion limit reached while expanding `demo_macros::__finish!``, help
  text naming the attribute); the fix is stated in the file's comment.
- Observation worth knowing: with direct chaining the error at the `__finish`
  step points at the `expand!` invocation (the callback path carries the
  user's `demo_macros` span); when a carrier trips the limit it names
  `crate::<source>::__pl_carrier!` and points at the previous source's
  `#[collect]` attribute. No trick found that improves on rustc's message.

### 5c. Hidden-name clash (deviation 11)

- Carrier hashes now cover the module's `file:line` (`diag::span_key`): the
  stable `proc_macro::Span::file()`/`line()` of the module's name inside the
  compiler. Outside the compiler (engine tests) proc-macro2's `span-locations`
  feature is used and the key is the line/column proc-macro2's source map
  gives a token parsed from a string; `quote!` tokens all sit at `1:0`, so
  the existing determinism tests still hold.
- Findings on the vendored proc-macro2 1.0.95: `Span::file()` exists only
  under the semver-exempt cfg (`--cfg procmacro2_semver_exempt`, not a
  feature); `Span::start()/end()` need the `span-locations` feature and
  return `0:0` for compiler spans on stable because the build probe needs the
  whole unstable `proc_macro_span` feature. Hence two code paths.
- Engine test `carrier_names_depend_on_where_the_module_is`.
- SPEC section 9 items 15 (v2 chaining), 16 (location in hidden names, G1
  reading), 17 (recursion limit).

### Tests after push 5

- `cargo test --workspace`: 54 passed (29 engine, 3 util, 5 stq-lite plugin,
  2 registry plugin, 14 rendered, 1 UI test running 18 cases).
- `cargo test -p demo-app --features extra`: 15 passed.
- UI `.stderr` changes, all intended: `carrier_version_mismatch` (now v1 vs
  v2; the legacy carrier in the case is a faithful v1 carrier), new
  `carrier_v1_in_chain` and `recursion_limit`. The "originates in the macro
  ... which comes from the expansion of ... `plenary::expand`" notes did
  not change, because rustc names the innermost and the outermost macro.
- `scripts/invalidation_demo.sh` still passes (G3).

### Gotchas hit

- `stringify!($name)` inside `#[stq::table(name = ..)]` is not a string
  literal to the plugin (macros see code as written): `error: expected string
  literal`. The UI case writes the modules out by hand.
- Regenerating a `.stderr` and then editing the case's comment shifts line
  numbers; regenerate last.

## Push 1: stages (done)

A collection is now also a stage: a later `expand!` can list it like a
source and gets (a) the facts its handlers exported and (b) the records found
in the items it emitted. SPEC section 10 (R13–R20, G8) is the contract;
deviations 18–23 record the choices.

### What changed

- `crates/plenary-plugin/src/facts.rs` (new): `ExportFact` (stable
  `NAME`, `to_tokens`, `from_tokens`), the `Encode`/`Decode` codecs for
  strings, integers, bools, `Vec`, `Option`, tuples, `syn::Path`, `Ident`,
  `LitStr`, `Span`, `TokenStream`, the `Fields` struct helper (unknown fields
  ignored, missing field is an error naming it), and `rebase_paths` (R17).
- `plugin.rs`: `Plugin::exportable::<F>()`, `ExportableDef` (type-erased
  decode / `Imported` construction), `Imported<F>` (a built-in fact, one per
  registered type, canonical order), bundle lookups by `(ns, NAME)`.
- `global.rs`: `GlobalCx::export(&fact)` (error naming plugin and type when
  unregistered; exports dropped with a failing handler), `exports()` for
  tests, handler ordering treats `Imported<F>` as built in and gives a clear
  message when a handler reads `Imported<F>` for an unregistered `F`.
- `carrier.rs`: header keywords `stage ;` and `fact { ns ".." type ".." ver ".." { .. } }`
  inside carrier format v2 (no bump); `parse_finish_input` reads the `export`
  flag `expand!(bundle; export; ..)` passes.
- `collect.rs`: `Collector::run_body` split out; `collect_round` runs it over
  the emitted items (module name `__pl_stage`, deviation 21).
- `finish.rs`: decodes stage blocks, checks fact versions, rebases and
  decodes facts into `Imported<F>`, runs handlers, runs the round, emits the
  processed items and the stage carrier (`carrier_macros`, same shape as a
  source carrier, `#[macro_export]` + `pub use` alias when exporting).
- `crates/plenary`: `expand!(bundle; export; ..)` form.
- `testing.rs`: `global_cx_with`, `imported`, `imported_from`,
  `SourceBlock`/`finish_blocks`/`finish_blocks_with`, `Finished::stage()`,
  `stage_of`.
- Plugins: `stq-lite`'s `Resolved` and `registry`'s `Index` are exportable;
  `link` and `index` take `&Imported<..>` and merge (imported first,
  duplicates cite the stage); the registry emits a generated
  `#[registry::handler("generated_echo")]` so every collection demonstrates
  the round (its body has no macro, see the depth note below).
- Demos: `demo-upstream::plan` is `expand!(demo_macros; export; ..)`;
  demo-app gets `staged.rs` + `plan2` (consumes `demo_upstream::plan` only)
  and `lookups.rs` + `plan3` (lists `crate::plan`: same-crate round, with
  `registry::lookup!("generated_echo")` resolving).
- UI support: `ui-plugins::unregistered_export` + `export-bundle`.

### Tests

- `cargo test --workspace`: 83 passed (7 unit incl. 4 facts codec, 29 engine,
  15 stage engine tests in `tests/stages.rs`, 6 stq-lite, 5 registry, 20
  rendered, 1 UI test running 21 cases). `--features extra`: 21 passed.
- New engine tests: fact round trip per codec, `Imported` ordering with
  rebased paths, fact major-version mismatch, unknown fact ignored,
  undecodable fact, stage carrier determinism (G1), one-round rule, export
  of an unregistered type, reading `Imported` of an unregistered type,
  markers in emitted items, failed stage / stage plugin major mismatch.
- New UI cases: `stage_export_unregistered`, `stage_fact_version_mismatch`
  (hand-written carrier, deviation 23), `stage_misspelled_column` (error at
  the user's literal through the extra hop). No existing `.stderr` changed.
- `scripts/invalidation_demo.sh` step 4 now also proves G8: a column added in
  demo-upstream changes `plan2`'s `AccountById` SQL although `plan2` lists no
  raw upstream source.

### Measured

- Depth thresholds unchanged (125/126 at the default limit): the stage
  carrier is a `macro_rules!` definition, not an expansion, and a listed stage
  costs one level like a source. One real finding: the first version of the
  generated handler used `format!`, which nests `format_args!` inside the
  collection's output and cost one more level (124/125). Macros inside
  emitted items count toward the depth; the body now has none.
- demo-app with three collections: incremental rebuild ~0.5 s.

### Gotchas hit

- Rebasing on raw tokens: a `crate` right after a single `:` (the
  `Fields` separator, `path : crate :: x`) must count as a path start; only a
  preceding `::` makes it a later segment. The first version treated any
  preceding `:` as a separator and rebased nothing.
- `#[derive(Default)]` on a struct holding a `proc_macro2::Span` fails
  (`Span: Default` is not satisfied); hand-written `Default`.
- The fact-level version check is unreachable with real carriers (the
  stage-level plugin check fires first); the UI case writes the carrier by
  hand and says so.

## Push 2: closed enums over open sets (done)

### What changed

- `crates/plenary-plugin/src/gen.rs` (new): `ClosedEnum` builder:
  `new(name)`, `entry(name, span)`, `payload(name, ty, values)`, `vis`,
  `derives`, `doc`, `variant_name(&str) -> Option<String>`, `build() ->
  Built { tokens, diagnostics, variants }`. Emits `#[repr(u16)]` dense
  discriminants in entry order, `ALL`, `COUNT`, `name`, `from_name` (a
  `match` on literals; perfect hash noted as future work), `id`,
  `TryFrom<u16>` (error = rejected id), one `const fn` per payload. Invalid or
  colliding names are errors citing the entries (note at the first) and are
  left out so the rest compiles. An empty set gets no `repr` (E0084).
- `registry` runtime: `HandlerFn` (was `Handler`, renamed to free the name),
  `Lookup { FN }` unchanged, new `Variant { type Handler; const HANDLER }`.
- `registry-plugin`: `emit` now returns a `HandlerEnum` fact (name → variant
  mapping) and emits `pub enum Handler` with payload `function` over the
  coercion-checked constants, `Handler::call`, `dispatch =
  Handler::from_name(name).map(|h| h.call(input))`, and keeps `HANDLERS`.
  New call kind `registry::variant!("name")` → `<marker as Variant>::HANDLER`;
  `lookups` implements both traits (no `Variant` impl for a name that made no
  variant: an empty enum has no placeholder value; the error about the name
  was already reported and the lookup gets its own error). The enum name and
  the generated handler carry the collection site's span, so rustc's notes
  (`Handler` defined here / first registered here) point at the `expand!`.
- demo-app: `consumers::describe` is an exhaustive `match` over
  `plan::Handler` (with a `#[cfg(feature = "extra")]` arm);
  `handlers::shout_handler` uses `variant!`.
- SPEC section 11; README "Closed sets".

### Tests

- `cargo test --workspace`: 94 passed (11 unit incl. 4 for `gen`, 29 engine,
  15 stages, 6 stq-lite, 6 registry, 26 rendered, 1 UI test running 24
  cases); `--features extra`: 27 passed.
- Rendered: `ALL.len() == HANDLERS.len()` with and without `extra`;
  `name`/`from_name`/`id`/`TryFrom` round trips; `call`; exhaustive match;
  `variant!`; the enum at `plan2` has `UpstreamEcho` and `GeneratedEcho`
  (imported through the stage), `plan3`'s has plan's handlers plus the
  generated one.
- UI: `registry_nonexhaustive_match` (rustc's `non-exhaustive patterns:
  `Handler::Shout` not covered`, with the note pointing at the handler's
  name literal), `registry_variant_collision` (`greet`/`Greet` collision
  citing both, `hello-world` invalid), `stage_generated_name_collision`
  (local `generated_echo` vs the one imported as a stage record; the note
  points at demo-upstream's `expand!`). All 21 earlier `.stderr` files are
  byte-identical to the push-1 checkpoint (checked by diffing the zips).

### Measured

- Depth thresholds unchanged: 125/126 plain sources, 41/42 with two
  conditional records each at the default limit. rustc now names
  `#[derive]` (the enum's) when the limit is hit: a derive is one expansion
  inside the output, the same level the stq obligations already used.
- `cargo doc --workspace --no-deps`: no warnings.

### Gotchas hit

- `#[repr(u16)]` on a zero-variant enum: `error[E0084]: unsupported
  representation for zero-variant enum`. Found by diffing all UI `.stderr`
  files against the previous checkpoint after regenerating (every case whose
  collection had no handler suddenly carried that error). The empty set now
  gets no `repr`.
- The `lookup!` and `variant!` markers are numbered in document order
  (`__pl_m0`, `__pl_m1`), so a hand-written `<self::__pl_m0 as Variant>`
  return type in the demo picked the wrong marker; a source can simply name
  the collection's type, `crate::plan::Handler` (R4 allows naming).
- `format!` in a generated item body costs one expansion level (`format!` →
  `format_args!`): the first generated handler lowered the threshold to 124.
  Generated bodies now use no macros.

## Rename: `macrostate` → `plenary`, `compose!` → `expand!` (done)

The working name is retired. The phrase for the idea is "plenary macro
expansion": a plenary session is one where every member is present; a
collection is a plenary expansion, where every record is present before
anything is decided. SPEC section 9, item 24 is the log entry.

### What changed

- The workspace lives in `plenary/` (a symlink `macrostate -> plenary` keeps
  old paths working). Crates: `crates/plenary` (was `crates/macrostate`) and
  `crates/plenary-plugin` (was `crates/macrostate-plugin`; lib
  `plenary_plugin`). The example crates keep their names.
- The collection macro is `plenary::expand!` (was `compose!`), with the same
  two forms: `expand!(bundle; sources..)` and `expand!(bundle; export;
  sources..)`. `#[collect]`, `__finish`, `Collection`, `Plugin`, `Record`,
  `State`, `Imported`, `ExportFact` and `ClosedEnum` keep their names. The
  facade-path accessor on `GlobalCx` follows the crate: `cx.plenary()`.
- The keyword at the front of the `__finish` input is `plenary = [ $crate ]`
  (`__finish` rejects any other word there: "expected `plenary = [..]`").
  Carrier format stays v2; item 24 says why, and what happens to a carrier a
  pre-rename build emitted (it is unreachable, rustc says "cannot find
  macro", nothing like it was ever released).
- The reserved hidden prefix is `__pl_` (was `__ms_`) everywhere it is
  generated or matched: `__pl_carrier_<hash>` / `__pl_carrier`,
  `__pl_rec_<seq>_<hash>` / `__pl_rec_<seq>`, markers `__pl_m<n>`, stq-lite's
  `__pl_ty_<Struct>_<field>` aliases, the `__pl_dollar` escape, the round
  module `__pl_stage`; the emitted constants are `__PlCoerce<n>` (were
  `__MsCoerce<n>`), and the const `bundle!` plants in a bundle crate is
  `__PL_BUNDLE` (was `__MS_BUNDLE`, which a case-sensitive search for `__ms_`
  / `__Ms` does not find).
- Prose: SPEC title, status line (the name is no longer "a working name"),
  the one-sentence gloss in section 1, item 24; README (same gloss, step 3
  says "expand"); doc comments; the scripts' comments; this file.

### Tests

- `cargo test --workspace`: 94 passed (same breakdown as push 2);
  `cargo test -p demo-app --features extra`: 27 passed; 0 build warnings from
  a clean `target/`; `cargo doc --workspace --no-deps`: no warnings.
- UI: regenerated with `UI_OVERWRITE=1`; 20 of the 23 `.stderr` files
  changed, 3 did not (`cfg_attr_plugin_attr`, `nested_source`,
  `unknown_kind`: they never mention the name). Every differing line was
  checked against the old file by a script, not by eye: it is one of the
  three substitutions (crate name, hidden prefix, `compose!` → `expand!`), or
  the mechanical consequence of `plenary::expand!` being four columns shorter
  than `macrostate::compose!`: on a line holding the call, a `-->` column
  number, a `^` underline or the `---- in this macro invocation` underline
  moves left or shrinks by exactly 4 (one `= note: first registered here
  (at ..:47:26)` became `:47:22` for the same reason). Nothing else differs.
- `scripts/invalidation_demo.sh`: steps 1–5 as before (G3 and G8 hold).
- `scripts/scale_probe.sh`: thresholds unchanged, 125/126 plain sources,
  62/63 and 41/42 with conditional records at the default limit, 1021/1022 at
  1024; 300 x 5 x 10 in ~5.7 s.

### Gotchas hit

- A blind `compose` → `expand` would have mangled English ("the sources the
  collection was composed from", "composing", "to compose") and the
  `Collection` trait's doc; those eight lines were reworded by hand. A blind
  `macrostate` → `plenary` would have left the SPEC status line calling the
  new name "a working name".
- The `.stderr` diff is not purely textual, because rustc underlines the
  whole `expand!(..)` call for "in this macro invocation" and reports columns
  of spans that sit after the macro name on the same line. Comparing the
  regenerated files line by line against the old ones, with those two shapes
  allowed only when the shift is exactly 4, is what makes "only the
  substitutions differ" checkable.
- The hidden prefix is not just cosmetic: `expand!` and every carrier call
  the next module's alias by its hidden name, so renaming the prefix cuts off
  carriers emitted under the old one. Recorded in item 24 rather than hidden
  behind a format bump, since no such carrier exists outside this repository's
  history.
