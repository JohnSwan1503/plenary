# plenary: specification

Status: draft 0.1. The name is "plenary" (section 1); until v0.2 the working name was "macrostate", and section 9, item 24 records what the rename changed.

## 1. What this is

Rust macros are pure functions from code to code. Each expansion sees only its own input, and the compiler doesn't promise any order between expansions. So "stateful" expansion, where one expansion's output depends on what other expansions saw, can't be done safely with hidden variables inside a macro crate. Three things go wrong:

- the expansion order isn't fixed;
- every crate starts with an empty state;
- the compiler can't see that one expansion depends on another, so cached results go stale.

plenary makes stateful expansion safe by keeping all state where the compiler can see it. There is one standard way to introduce state and one standard way to consume it. The name says how it works: a plenary session is one where every member is present; a collection is a plenary expansion, where every record is present before anything is decided.

- **Introduce.** Inside a *source* (a module marked for collection), an item or macro call tagged with a plugin's namespace becomes a *record*. Records are copied into a *carrier*, an ordinary `macro_rules!` macro generated next to the source.
- **Consume.** A *collection* (`plenary::expand!`) lists sources explicitly. It pulls their records through the carriers and runs the plugins' *global handlers* once, with every record in view. Their output is emitted where the collection is written.

Everything a global handler sees arrives as code passed into it, so the compiler's normal rules for when to expand again are always correct.

## 2. Terms

**Plugin.** An ordinary library crate. It registers:
- a namespace, for example `stq`;
- the record kinds it owns, for example `stq::table` and `stq::query`;
- local handlers;
- global handlers.

A plugin never defines proc macros itself. It usually has a companion runtime crate that holds the traits and types its generated code refers to.

**Bundle.** A proc-macro crate, one per project. Its entire source is one line:

```rust
plenary_plugin::bundle!(stq_plugin::plugin, registry_plugin::plugin);
```

It generates the project's two proc macros: the `collect` attribute, and `__finish`, which only `expand!` calls. All plugins in a bundle run inside the same expansion, so they can share results.

**Source.** An inline module marked with the bundle's attribute: `#[my_macros::collect] pub mod users { ... }`. Records can only be introduced inside a source. Optional argument: `export`, which makes the carrier usable from other crates.

**Record.** One unit of introduced state. It comes in exactly two forms.

- *Attribute form:* an item with an attribute whose path is `ns::kind`, where `ns` is a registered namespace. Example: `#[stq::table(name = "users")] pub struct Users { ... }`.
- *Call form:* a macro call whose path is `ns::kind!`, anywhere inside the source, including inside function bodies. Example: `let sql = stq::sql!("SELECT id FROM {Users}");`.

A record holds:

| Field | Meaning |
|---|---|
| kind | `ns::kind` |
| args | the attribute's or the call's arguments, as tokens |
| item | attribute form only: the full item, with all plugin attributes still on it |
| relative path | where the record is inside the source (`Users`, `inner::Users`). For the call form, the enclosing item plus a sequence number. |
| sequence | the record's position in the source, in document order |
| condition | the `#[cfg]` predicates on the record and its enclosing items, combined. Empty if unconditional. |
| annotations | data its local handler attached, carried along with it |
| tainted | true if its local handler reported an error |

Records don't change after their local handler runs. Spans inside the tokens are kept, so diagnostics can point at the user's code from anywhere.

**Carrier.** A `macro_rules!` macro emitted next to a source. It passes the source's records to whatever macro calls it, and it records a format version. Without `export`, a carrier is visible inside its own crate. With `export`, it's also visible from other crates.

**Collection.** An invocation of `plenary::expand!(bundle; path::to::source_a, path::to::source_b, ...)` in item position. It gathers the records of the listed sources and runs the global handlers. A project can have any number of collections, and they share nothing. A source can appear in several collections.

**State.** What global handlers receive. It contains:
- the collected records, in canonical order (R5);
- for each source: its path as listed, its crate, whether it is local to this crate, its export flag, and the plugin versions that processed it.

**Local handler.** A function with the signature `fn(&Record, &mut LocalCx) -> Result<()>`, registered for one kind. It runs inside `collect`, once per record, and sees only that record and its source. It can:
- replace the item (by default, the item stays, with plugin attributes removed);
- add items next to the item;
- set the replacement for a call-form record;
- create a marker at the source root (R4);
- attach annotations to the record;
- report diagnostics.

**Global handler.** A function whose parameters are references to facts, followed by `&mut GlobalCx`, and whose return type is `Result<T>`:

```rust
fn link(state: &State, cx: &mut GlobalCx) -> Result<Resolved>;
fn render(resolved: &Resolved, cx: &mut GlobalCx) -> Result<()>;
```

It runs inside `__finish`, once per collection. It can:
- emit items at the collection site;
- emit obligations (section 4);
- report diagnostics;
- return a fact. Returning `()` produces no fact.

**Fact.** A plain Rust value (`'static`) that one global handler produces and others read during one collection run. Facts are looked up by type, and `State` is a built-in fact. A plugin can read another plugin's facts by depending on that plugin's library crate. Facts never outlive the run.

## 3. Rules

**R1. How state enters.** State enters only as records inside sources, in the two forms above. Nothing else is a valid input to a handler: no files, no environment variables, no static variables.

**R2. Locality.** A source's output depends only on that source. Local handlers can't see other records. Results that depend on more than one source come only from global handlers, and appear only at collection sites.

**R3. Names resolve where they're emitted.** Tokens copied out of a record mean whatever they mean at the place they're emitted. To refer to something in a source from a collection, use one of these:
- the record's *absolute path*: the source's path as listed in `expand!`, joined with the record's relative path;
- a named handle that the local handler created inside the source.

Items that a collection refers to must be visible from the collection site.

**R4. Forward references.** Local output can *name* things a collection will define, but it can't depend on their contents. The standard pattern works like this:
- `LocalCx::marker()` creates a marker type at the source root and returns a path to it that is valid at the call site.
- The local handler replaces the call with an expression on that marker, for example `<super::__pl_m3 as ::stq::Query>::SQL`.
- A global handler implements that trait for the marker's absolute path.

This is the only way information flows from a collection back into a source's code. Markers belong to the crate that defines the source, so only collections in that crate may implement traits for them (`SourceInfo::is_local()`). A source that uses markers must be listed in exactly one collection in its crate. If it's in none, the trait is missing. If it's in two, the impls conflict.

**R5. Canonical order.** Global handlers see records ordered first by the source's position in the `expand!` list, then by sequence within the source. This order never depends on the order in which the compiler expands macros.

**R6. Handler order.** A global handler runs after the handlers that produce the facts it reads. Each fact type has at most one producer in a bundle. These are errors at the `expand!` site, and the message names the handlers involved:
- a fact with no producer;
- a fact with two producers;
- a cycle.

Handlers with no dependency between them run in registration order: first in the bundle's list order, then in the order each plugin registered them.

**R7. Configuration.** A record is in the state if and only if its condition holds in the current build. The compiler decides this, not the framework: each conditional record travels through its own pair of carriers. One sits under `#[cfg(condition)]` and passes the record. The other sits under `#[cfg(not(condition))]` and passes nothing. In v0, a `cfg_attr` that wraps a `cfg` or a plugin attribute is rejected. Also in v0, a source module that is itself compiled out must be left out of `expand!` lists.

**R8. Determinism.** Each expansion's output depends only on its input tokens. Plugins must not:
- read files, environment variables or clocks;
- keep state between expansions;
- let hash-map iteration order reach their output.

The framework uses ordered collections internally.

**R9. Unknown namespaces.** A collection uses only the namespaces its bundle knows. Records from namespaces it doesn't know are ignored, for example records from an upstream crate that uses an extra plugin. An unknown kind inside a known namespace is an error at the record, with a suggestion when a close match exists.

**R10. Compatibility.** A collection rejects a carrier with a different format version, and the error names the source and both versions. Each record carries the version of the plugin that ran its local handler. A plugin whose major version differs is an error.

**R11. Failure.**
- All errors from one expansion are reported together.
- A tainted record still travels, flagged, so global handlers can skip it instead of adding more errors on top.
- A handler that panics produces an error naming the plugin, the handler and the panic message.
- A source whose collector fails still emits a carrier marked as failed. Collections skip it instead of adding "cannot find macro" errors.

**R12. No nesting.** A source can't contain another source.

## 4. Outputs

**Items.** Global handlers emit items at the collection site. They can add new items and trait impls, including impls for types in sources, but they can't change existing items. To attach a decision to a user's type, implement a trait for it at the collection site. Each collection also defines a marker type, `__Collection`. Plugins can use it as a type parameter on their traits so that a crate can implement them for types from other crates.

**Diagnostics.** Errors point at the user's tokens. Warnings use the deprecated-item technique: they appear as `deprecated` warnings at the user's code. That means `#[allow(deprecated)]` hides them and `-D warnings` turns them into errors.

**Obligations.** Some facts only the compiler knows: resolved types, trait impls, constant values. These are checked by emitting obligations, which the compiler proves or rejects at the user's code.

| Obligation | Emitted as |
|---|---|
| `require_same_type(a, b, span, message)` | a helper trait for that one obligation, carrying `#[diagnostic::on_unimplemented(message = ...)]`, with a blanket impl `impl<T> Helper<T> for T`, and a bound check |
| `require_impl(ty, trait, span, message)` | a bound check `ty: trait`, with the best message rustc allows |
| `require_coerces(expr, ty, span)` | `const _: ty = expr;` |
| `require_const(expr, span, message)` | `const _: () = assert!(expr, message);` |

## 5. Guarantees

- **G1. Determinism:** identical input tokens produce identical output.
- **G2. Order independence:** a collection's result doesn't depend on the order in which the compiler expands macros.
- **G3. Correct rebuilds:** any change to a listed source changes the collection's input, so the collection expands again. No result can be stale.
- **G4. Locality:** see R2. This is enforced by what each handler receives.
- **G5. Configuration correctness:** see R7.
- **G6. Compatibility checks:** see R10.
- **G7. Diagnostics at the user's code:** errors and warnings point at the user's tokens, however many carrier steps they passed through.

## 6. Known limits (stable Rust 1.97)

- Sources must be inline modules, because file modules in proc-macro input are unstable (rust-lang/rust#54727). Put `#[collect] pub mod users { ... }` inside `users.rs`.
- Macros see code as written. Types, `use` renames and constants aren't resolved, so use obligations for those. Call-form records are matched by spelling, so a renamed import of `ns::kind!` isn't seen.
- Other macros' output isn't visible, because an outer attribute expands before the items inside it.
- Each source costs about two levels of expansion depth, plus one for each conditional record. Large collections need `#![recursion_limit = "..."]`.
- Depth formula, measured with carrier format v2 (rustc 1.97; see §9 item 15 for why the previous bullet says two): a collection's expansion depth is `1 + S + C + 1 + E`, where `S` is the number of listed sources (one carrier each, chained directly), `C` the number of conditional records across them (one `#[cfg]` carrier pair each), the two `1`s are `expand!` and `__finish`, and `E` the deepest macro nesting inside what the plugins emit (0 for plain items; 1 when an obligation wraps a `#[diagnostic::on_unimplemented]` trait, as stq-lite's do). The build fails when that depth exceeds `recursion_limit`. With the default limit of 128 and `E = 1`: 125 unconditional sources build, 126 don't; 62 build with one conditional record each; 41 with two. With `#![recursion_limit = "1024"]`: 1021 sources build. When the limit is hit, `__finish` never runs, so the framework cannot report it; rustc does (`recursion limit reached while expanding ...`, with its own `help:` naming the attribute to add).
- An error inside a string literal points at the whole literal.
- Stable Rust has no custom warnings, so warnings use the technique in section 4.

## 7. Not in v0

- Stages: a collection exporting derived facts as a carrier for later collections or crates.
- Per-source local handlers that see all the records of one source.
- Lint levels and allow/deny configuration. These belong in a plugin built on top.

## 8. How stq maps onto this

| stq concept | plenary concept |
|---|---|
| `#[table]`, `#[query]` | record kinds `stq::table`, `stq::query` |
| template parsing and placeholder checks | local handlers |
| matching queries to tables across files | global handler producing the fact `Resolved` |
| rendered SQL constants | global handler emitting `impl Query<__Collection>` |
| column type agreement | obligations |
| inline `stq::sql!` in a function body | call-form record plus marker (R4) |
| project rules | a separate plugin that reads `Resolved` |

## 9. Deviations (v0 implementation)

The normative text above is unchanged. This section lists where the v0
implementation in this repository differs from it or pins down something it
leaves open.

1. **R11, syntax errors.** A source whose *body cannot be parsed* can't get a
   working carrier: the collector re-emits the module unchanged (so the
   compiler reports the syntax error) and appends a carrier marked `failed`
   inside the body, but that carrier is only reachable when the compiler can
   parse what syn could not. For every other collector failure (nested source,
   R12) the failed carrier works as specified and the collection skips the
   source silently.

2. **`require_coerces` emits a named constant.** Instead of
   `const _: ty = expr;` it emits
   `#[doc(hidden)] const __PlCoerceN: ty = expr;` and returns the name, so a
   plugin can build emitted items from the checked value without coercing a
   second time (which would report the same mismatch twice). Everything else
   about the obligation is as specified.

3. **Attribute-form records are module-level items.** An item with a plugin
   attribute inside a function body (or any other body) is an error, not a
   record. Call-form records are found anywhere, including bodies, except
   inside a `mod` declared inside a body.

4. **Relative path of a call-form record.** The record stores the path of
   the enclosing item (`inner::body`) and the sequence number separately;
   `Record::display_path()` renders them together as `inner::body#3`.

5. **"Its crate" in `State`.** A source's crate name is derived from the path
   listed in `expand!` (`crate` for local sources, otherwise the first
   segment), because a collector cannot learn its own crate name without
   reading the environment (R8).

6. **Plugin versions travel per source.** The carrier header lists one
   `plugin "ns" "name" "version" ;` line per plugin in the bundle that ran the
   collector; records get their plugin version through their source rather
   than carrying a copy each. The R10 check is the same.

7. **Unknown kinds at the collection site are ignored.** At collect time an
   unknown kind in a known namespace is an error with a suggestion (R9). At
   finish time, records whose kind the bundle does not know (for example from
   an upstream crate collected with a newer plugin of the same major version)
   are ignored like unknown namespaces, not reported.

8. **Conditions come from `#[cfg]` on items.** The combined condition covers
   the record and its enclosing items, impl/trait items, `let` statements and
   match arms. A `#[cfg]` on a field or variant inside a record is left to the
   compiler and the plugin: the record's item tokens contain it as written.

9. **A failing global handler drops its output.** When a global handler
   returns `Err` or panics, the diagnostics it reported are kept, the items
   and obligations it emitted are dropped, its fact is not produced, and the
   handlers that read that fact are skipped without further errors.

10. **Two plugin attributes on one item** produce two records; if both local
    handlers try to replace the item, that is an error.

11. **Hidden names.** Carrier macros are named `__pl_carrier_<hash>` (alias
    `__pl_carrier`) and per-record carriers `__pl_rec_<seq>_<hash>` (alias
    `__pl_rec_<seq>`), where the hash covers the module's name and tokens.
    Two *exported* sources in one crate with the same name and identical
    contents would clash at the crate root. Identifiers starting with `__pl_`
    are reserved; a bare `$` inside a record is escaped as `__pl_dollar` in
    the carrier and restored on decode.

12. **`expand!` also passes the bundle path to `__finish`**
    (`bundle = [ demo_macros ]`), so errors that belong to the whole
    collection (handler ordering, panics) point at the `expand!` invocation.

13. **Call replacements are respanned.** The tokens a local handler sets with
    `set_replacement` get the span of the call's kind ident, so compiler
    errors about the expansion point at the user's call;
    `set_replacement_spanned` keeps the handler's own spans.

14. **The facade holds no obligation helpers.** Each obligation generates its
    own helper trait inside a `const _` block, because the
    `#[diagnostic::on_unimplemented]` message is per obligation. The facade
    holds `expand!`, the `Collection` trait (implemented by every
    `__Collection`, listing the sources) and `CARRIER_FORMAT_VERSION`.

15. **Carriers chain directly (carrier format v2).** Section 6 says a source
    costs "about two levels" of expansion depth; that described format v1,
    where `expand!` called each carrier and each carrier called `expand!`
    back. In v2 `expand!` only starts the chain: it calls the first source's
    carrier and hands it the rest of the list, every carrier (after its own
    conditional-record carriers) calls the *next* listed source's carrier
    itself, and the last one calls the bundle's `__finish`. One level per
    source; the measured formula is in section 6. The block layout inside
    `source [..] { .. }` is unchanged, but the calling convention is part of
    the format, so `CARRIER_FORMAT_VERSION` is 2. A v1 carrier that ends up
    last in a list reaches `__finish` and gets the R10 version error; a v1
    carrier in the middle hands the remaining list back to `__finish`
    instead of chaining, and `__finish` reports that as a calling-convention
    mismatch naming the source and both versions (also R10), rather than a
    parse error.

16. **Hidden names depend on the module's location.** The hash in
    `__pl_carrier_<hash>` and `__pl_rec_<seq>_<hash>` (item 11) now covers
    the module's `file:line` (from the stable `proc_macro::Span::file` and
    `line` of the module's name) as well as its name and tokens, so the clash
    described in item 11 (two exported sources with the same name and
    identical contents in one crate) no longer happens. G1 is to be read as
    "identical input at the same location produces identical output": moving
    a module changes its hidden names, which nothing refers to except through
    the `__pl_carrier` / `__pl_rec_<seq>` aliases next to it. Outside the
    compiler (the engine's own tests) there is no compiler span data, so the
    "location" is the line and column proc-macro2's own source map assigns a
    token parsed from a string (`"...".parse()`); `quote!`-made tokens all
    sit at `1:0`. The vendored proc-macro2 1.0.95 exposes `Span::file()` only
    under its semver-exempt cfg, and its `Span::start()` returns `0:0` for
    compiler spans on stable, hence the two code paths (`span_key`).

17. **The recursion limit is not detectable.** When a collection is deeper
    than `recursion_limit` (formula in section 6), the chain is cut before
    `__finish` runs, so no framework diagnostic is possible on stable Rust.
    rustc's own error names the macro that tripped the limit: `__finish!`
    when the list is one too long (the error then points at the `expand!`
    invocation), or `crate::<source>::__pl_carrier!` when it is longer (the
    error then points at the previous source's `#[collect]` attribute, whose
    span the chaining call inside that carrier carries, with `expand!` as
    the "in this macro invocation" note). Its `help:` names the attribute to
    add. The UI case `recursion_limit.rs` pins the text.

18. **Section 7, item 1 is implemented.** Stages exist as of v0.2; section 10
    is the contract. Items 2 and 3 of section 7 stand. Section 2's "a project
    can have any number of collections, and they share nothing" now reads:
    they share nothing unless one lists another as a stage.

19. **Every collection emits a stage carrier,** not only exporting ones. The
    carrier is a `macro_rules!` definition next to the collection's items; it
    holds the exported facts' tokens and the round's records, duplicated in
    its two rules (chain on / finish) like a source carrier's block, so a
    collection that exports a large fact pays for it twice in source size,
    once per rule. Its hidden name is `__pl_carrier_<hash>` where the hash
    covers the collection site's `file:line` and the whole `__finish` input;
    callers only ever use the `__pl_carrier` alias.

20. **Rebasing is done on tokens, by the engine.** `ExportFact::from_tokens`
    receives tokens with R17 already applied, so a plugin's codec never deals
    with paths. The engine decides what is a path start syntactically: an
    ident `crate`, `self` or `super` that is not preceded by `::` (a single
    `:`, the field separator of `facts::Fields`, does not count) and is
    followed by `::`; `$crate`, escaped as `__pl_dollar crate` inside a
    carrier, is left alone.

21. **The round's module name is `__pl_stage`.** `__finish` does not know the
    name of the module its `expand!` sits in, so local handlers running in a
    round see `cx.source().module_name()` as `__pl_stage` and
    `cx.source().is_export()` as the collection's `export` flag. Relative
    paths of round records never include it. When the emitted items do not
    parse as items, they are emitted unchanged, the stage carries facts only,
    and an error at the collection site says no records could be looked for.

22. **What the example plugins export.** `stq-lite`'s `Resolved` travels
    without its type obligations (the exporting collection proved them; the
    importing one re-emits only the `Query<__Collection>` impls) and marks
    imported tables so the "never used" warning does not fire for them;
    imported inline queries are never local. `registry`'s `Index` travels
    whole. Both plugins put imported entries before local ones (R5 order
    among stages, then list order among sources) and report a name defined
    both upstream and locally as a duplicate citing the stage. The registry
    also emits a generated handler, `generated_echo`, tagged
    `#[registry::handler]`, as the standing example of R16.

23. **The fact-level version check (R18) is reachable only with a
    hand-written carrier.** A real stage's `plugin` lines and its facts' `ver`
    come from the same bundle, so the stage-level R10 check reports first.
    The UI case `stage_fact_version_mismatch.rs` exercises the fact-level
    message by writing the carrier by hand.

24. **The name.** The framework was developed under the working name
    `macrostate`; it is now `plenary`, and the collection macro is `expand!`
    (it was `compose!`). Three things carry the name and were renamed with it,
    all still inside carrier format v2, so `CARRIER_FORMAT_VERSION` stays 2:
    the crates (`plenary`, `plenary-plugin`); the keyword `expand!` puts at
    the front of the `__finish` input, which in v2 is `plenary = [ $crate ]`
    (`__finish` rejects any other word there); and the reserved hidden prefix,
    which is `__pl_` (`__pl_carrier`, `__pl_rec_<seq>`, `__pl_m<n>`,
    `__pl_ty_*`, `__pl_dollar`, `__pl_stage`) with `__Pl` for the emitted
    constants (`__PlCoerce<n>`), item 2. The format is not bumped because the
    block layout and the calling convention are what they were, and a carrier
    never contains the keyword: it passes the prefix it is handed back to the
    callback unchanged, so the keyword is a contract between `expand!` and
    `__finish` only, which always come from the same version of this
    workspace. What the rename does break is reaching a carrier that a
    pre-rename build emitted: `expand!` and every carrier call the next
    module's alias by its hidden name (`__pl_carrier`), and such a carrier
    only has the alias `__ms_carrier`, so listing it fails with rustc's
    "cannot find macro" at the `expand!` site rather than with an R10 message.
    Nothing was released under the old name, so no carrier like that exists
    outside this repository's history. Items 2, 11, 16, 17, 19, 20 and 21
    above are read with the new spellings; their numbering and history are
    unchanged.

## 10. Stages (v0.2)

A *stage* is a collection seen as a source by later collections. This section
implements item 1 of section 7 ("Not in v0"); the terms of section 2 keep
their meaning.

**Stage.** Every collection is also a stage. Next to its items, `__finish`
emits a carrier with the calling convention of a source carrier (alias
`__pl_carrier` in the collection's module) whose block is marked `stage`.
Listing the collection's module in a later `expand!`, in the same crate or,
for an exporting collection (R20), in a crate that depends on it, gives that
later collection what the stage provides (R13). The stage block travels
inside carrier format v2: its header has the keyword `stage ;` and one
`fact { ns ".." type ".." ver ".." { .. } }` entry per exported fact, before
its `rec` blocks.

**R13. What a stage provides.** Two things: (a) the *exported facts*: the
values its global handlers passed to `GlobalCx::export`, in the order they
were exported, each tagged with the namespace and version of the plugin that
registered the fact type and with the type's name; (b) the *records* found in
the items the collection emitted (R16). Nothing else crosses: a stage does not
re-export the records of the sources it listed.

**R14. Exportable facts.** A fact type can be exported only if it implements
`ExportFact` (a stable `NAME`, `to_tokens`, `from_tokens`) and a plugin
registered it with `Plugin::exportable::<F>()`. `cx.export(&f)` of an
unregistered type is an error at the collection site naming the plugin and
the type, and exports nothing. The exports of a handler that fails are
dropped with its items (section 9, item 9). Register a type in the plugin that
defines it: the carrier records that plugin's namespace and version, and
collections match facts by namespace and `NAME`.

**R15. Imported facts.** For every registered exportable type `F`, every
collection has a built-in fact `Imported<F>`: the decoded values of type `F`
exported by the listed stages, each with the stage's `SourceInfo`, ordered by
the stage's position in the `expand!` list and then by export order inside
the stage. It exists before any handler runs, possibly empty, and needs no
producer. A handler reads it by taking `&Imported<F>`. A bare `&F` keeps its
meaning: produced by a handler in this bundle (R6). A handler cannot produce
`Imported<F>`, and reading `&Imported<F>` for an unregistered `F` is an error
at the collection site naming the handler and the type.

**R16. One round per collection.** After a collection's global handlers ran,
the items they emitted are processed once, as the body of a source at the
collection site: records are found, their local handlers run, plugin
attributes are stripped, call-form records are replaced, markers are planted
in the collection's module. The processed items are what the collection
emits; the records go into the stage carrier with paths relative to the
collection's module. These records are *not* visible to the same
collection's global handlers, which have already run. A collection that lists
this one sees them as ordinary records, whose source is the stage. There is
no fixpoint: what a round creates is visible exactly one collection later.

**R17. Paths in facts.** A fact's tokens mean what they meant at the
exporting collection's site (R3). When a collection imports a fact, every
path in the fact's tokens that starts with `crate`, `self` or `super` is
rebased to the importing site using the stage's path as listed: `crate::x`
stays when the stage is local (listed from `crate`, `self` or `super`) and
otherwise gets the stage's crate name (the first segment of the listed path)
in place of `crate`; `self::x` becomes `<listed path>::x`; each leading
`super` drops one trailing segment of the listed path. A path whose first
segment is anything else is left alone, as is a `super` chain that would
consume the whole listed path. A leading keyword is one not preceded by `::`. Record tokens are
not rebased, as for sources.

**R18. Unknown and incompatible facts.** A fact whose namespace or type name
the bundle does not know is ignored (R9). A stage's `plugin` lines are
checked like a source's (R10) and, when they pass, a fact whose plugin major
version differs from the bundle's is an error naming the stage, the fact, the
plugin and both versions; that fact is dropped and the collection goes on. A
fact that fails to decode is an error naming the stage and the fact.

**R19. Listing stages.** A stage may be listed by any number of collections.
R4 applies to a stage as to a source: when its emitted items contain
call-form records (markers planted in the collection's module), the stage
must be listed in exactly one collection of its own crate. A stage costs the
same expansion depth as a source: one level, plus one for each conditional
record among its emitted items.

**R20. Export marking.** `expand!(bundle; export; sources..)` makes the
stage carrier `#[macro_export]` with a `pub use` alias, so crates that depend
on this one can list the collection; without `export` the alias is
`pub(crate)` and only this crate's collections can. One `expand!` per
module: the alias name is fixed.

**G8. Stage invalidation.** The stage carrier's tokens are a function of the
collection's input (G1), so changing any record of any source listed by a
stage changes the stage carrier, and every collection that lists the stage
expands again; through any number of stages. `scripts/invalidation_demo.sh`
step 4 shows it: a column added to `demo-upstream`'s `accounts` table changes
the rendered SQL of a query in `demo-app`'s `plan2`, which lists only the
stage `demo_upstream::plan`, never the raw upstream source.

## 11. Closed sets (v0.2)

A collection sees every record at once, so a global handler can turn an open
set of declarations into a *closed* type: an enum with one variant per
record. `plenary_plugin::gen::ClosedEnum` is the standard way to do it; the
`registry` example's `plan::Handler` is built with it. The contract:

**Input.** An enum name, an ordered list of entries (a name and the span of
the record that declared it), and optional *payloads*: per-variant constants
(`pub const fn <name>(self) -> <type>`), one value per entry.

**Names to variants.** An entry's variant is its name split on `_`, each part
capitalised, joined: `greet` → `Greet`, `count_users_sql` → `CountUsersSql`.
A name whose parts contain anything but ASCII letters and digits, that would
start with a digit, or that becomes a keyword, makes no variant; that is an
error at the entry's span. Two entries whose names make the same variant
(`greet` and `Greet`) are an error at the second entry's span with a note at
the first. Entries with errors are left out, so the enum and the rest of the
collection still compile and the other errors of the build are reported.

**Output.** `#[repr(u16)] #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum <Name>` with discriminants `0, 1, 2, ..` in entry order; `pub const
ALL: &[Self]` and `pub const COUNT: usize`; `pub const fn name(self) ->
&'static str`; `pub fn from_name(&str) -> Option<Self>` (a `match` on string
literals; a perfect hash is future work); `pub const fn id(self) -> u16`;
`impl TryFrom<u16>` whose error is the rejected id; one `const fn` per
payload. An empty set is an empty enum without the `repr` (which rustc
rejects on a zero-variant enum, E0084). The builder also returns the final
`(name, variant)` mapping, so a plugin can refer to variants elsewhere (the
registry's `variant!` markers implement `registry::Variant` with it).

**Determinism.** Discriminants follow entry order; give the entries canonical
order (R5) and the type is the same for the same records (G1), and changes
only when the set changes.

**Depth.** The `#[derive]` on the enum is one macro expansion inside the
collection's output, the same `E = 1` level of section 6 that stq-lite's
obligation attributes already occupy; the generated code nests nothing deeper,
so the measured thresholds of section 6 are unchanged (125 sources at the
default limit, with rustc now naming `#[derive]` when the limit is hit).
