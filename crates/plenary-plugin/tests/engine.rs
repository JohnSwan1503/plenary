//! Engine tests: everything here runs without a compiler, on `proc_macro2`
//! tokens, which is the point of writing the engine against `proc_macro2`.

use plenary_plugin::__private::{decode_source, encode_source_body, finish_impl, Bundle, SourceDesc};
use plenary_plugin::testing;
use plenary_plugin::{Fact, Form, GlobalCx, LocalCx, Plugin, Record, Result, State};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

// ───────────────────────────── test plugins ─────────────────────────────

/// A small plugin with one attribute kind, one call kind and no globals.
fn basic_plugin() -> Plugin {
    fn item(record: &Record, cx: &mut LocalCx) -> Result<()> {
        cx.annotate("seen", record.ident().cloned().unwrap_or_else(|| Ident::new("anon", Span::call_site())));
        Ok(())
    }
    fn call(record: &Record, cx: &mut LocalCx) -> Result<()> {
        let m = cx.marker();
        cx.annotate("args", record.args());
        cx.set_replacement(quote!(<#m as ::some::Trait>::VALUE));
        Ok(())
    }
    Plugin::new("t", "test-plugin", "0.1.0").attribute_kind("item", item).call_kind("call", call)
}

struct A(u32);
impl Fact for A {}
struct B(u32);
impl Fact for B {}

fn emit_marker(cx: &mut GlobalCx, name: &str) {
    let id = Ident::new(name, Span::call_site());
    cx.emit(quote!(const #id: () = ();));
}

/// Handlers registered consumer-first: `needs_a` before `makes_a`.
fn ordering_plugin() -> Plugin {
    fn needs_a(a: &A, cx: &mut GlobalCx) -> Result<B> {
        emit_marker(cx, "NEEDS_A");
        Ok(B(a.0 + 1))
    }
    fn makes_a(_state: &State, cx: &mut GlobalCx) -> Result<A> {
        emit_marker(cx, "MAKES_A");
        Ok(A(1))
    }
    fn needs_b(b: &B, cx: &mut GlobalCx) -> Result<()> {
        assert_eq!(b.0, 2);
        emit_marker(cx, "NEEDS_B");
        Ok(())
    }
    fn independent(cx: &mut GlobalCx) -> Result<()> {
        emit_marker(cx, "INDEPENDENT");
        Ok(())
    }
    Plugin::new("o", "ordering-plugin", "1.0.0")
        .global(needs_a)
        .global(independent)
        .global(makes_a)
        .global(needs_b)
}

fn sources_of(output: &str) -> Vec<&str> {
    ["MAKES_A", "NEEDS_A", "NEEDS_B", "INDEPENDENT"]
        .into_iter()
        .filter(|m| output.contains(m))
        .collect()
}

fn position(output: &str, marker: &str) -> usize {
    output.find(marker).unwrap_or_else(|| panic!("{marker} missing in {output}"))
}

// ───────────────────────────── carrier round trip ─────────────────────────────

#[test]
fn carrier_encode_decode_round_trip() {
    let collected = testing::collect(
        vec![basic_plugin()],
        quote! {
            mod src {
                #[cfg(feature = "x")]
                #[t::item]
                #[derive(Debug)]
                pub struct S { #[t::sub] pub f: i64 }

                pub mod inner {
                    #[t::item]
                    pub fn body() {
                        let v = t::call!(1 + 2);
                        macro_rules! local { ($x:expr) => { $x }; }
                    }
                }
            }
        },
    );
    assert!(collected.diagnostics.is_empty(), "{:?}", collected.diagnostics);
    let records = collected.records;
    assert_eq!(records.len(), 3);

    let desc = SourceDesc { export: true, ..SourceDesc::default() };
    let body = encode_source_body(&desc, &records);
    let path = vec![Ident::new("crate", Span::call_site()), Ident::new("src", Span::call_site())];
    let decoded = decode_source(3, path, body).expect("decodes");
    assert!(decoded.info.is_export());
    assert_eq!(decoded.info.index(), 3);
    assert_eq!(decoded.records.len(), 3);
    for (a, b) in records.iter().zip(&decoded.records) {
        assert_eq!(a.kind().to_string(), b.kind().to_string());
        assert_eq!(a.form(), b.form());
        assert_eq!(a.sequence(), b.sequence());
        assert_eq!(a.args().to_string(), b.args().to_string());
        assert_eq!(a.item().map(|t| t.to_string()), b.item().map(|t| t.to_string()));
        assert_eq!(a.relative_path(), b.relative_path());
        assert_eq!(a.condition().map(|t| t.to_string()), b.condition().map(|t| t.to_string()));
        assert_eq!(a.markers(), b.markers());
        assert_eq!(a.is_tainted(), b.is_tainted());
        let an: Vec<(String, String)> = a.annotations().iter().map(|(k, v)| (k.clone(), v.to_string())).collect();
        let bn: Vec<(String, String)> = b.annotations().iter().map(|(k, v)| (k.clone(), v.to_string())).collect();
        assert_eq!(an, bn);
    }
    // Details worth pinning down.
    let s = &decoded.records[0];
    assert_eq!(s.condition().unwrap().to_string(), "feature = \"x\"");
    assert_eq!(s.relative_path().iter().map(|i| i.to_string()).collect::<Vec<_>>(), ["S"]);
    assert!(s.item().unwrap().to_string().contains("# [t :: sub]"), "sub-attributes stay on the record's item");
    let body = &decoded.records[1];
    assert_eq!(body.relative_path().iter().map(|i| i.to_string()).collect::<Vec<_>>(), ["inner", "body"]);
    assert!(body.item().unwrap().to_string().contains("$ x"), "a `$` inside a record survives: {}", body.item().unwrap());
    let call = &decoded.records[2];
    assert_eq!(call.form(), Form::Call);
    assert_eq!(call.markers().len(), 1);
    assert_eq!(call.annotation("args").unwrap().to_string(), "1 + 2");
    assert_eq!(call.absolute_path().segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>(), ["crate", "src", "inner", "body"]);
    assert_eq!(call.display_path(), "crate::src::inner::body#2");
}

#[test]
fn collector_output_strips_plugin_attributes_and_replaces_calls() {
    let collected = testing::collect(
        vec![basic_plugin()],
        quote! {
            mod src {
                #[t::item]
                pub struct S { #[t::sub] pub f: i64 }
                pub mod inner {
                    pub fn body() -> u8 { t::call!(x) }
                }
            }
        },
    );
    let out = collected.output.to_string();
    // The emitted items lose the plugin attributes...
    assert!(out.starts_with("mod src { pub struct S { pub f : i64 } pub mod inner { pub fn body () -> u8 { < super :: __pl_m0 as :: some :: Trait > :: VALUE } }"), "{out}");
    // ... while the record's item inside the carrier keeps all of them.
    assert!(out.contains("item { # [t :: item] pub struct S { # [t :: sub] pub f : i64 } }"), "{out}");
    assert!(out.contains("pub struct __pl_m0 ;"), "{out}");
    assert!(out.contains("macro_rules ! __pl_carrier_"), "{out}");
    assert!(out.contains("pub (crate) use __pl_carrier_"), "{out}");
}

#[test]
fn export_sources_get_macro_export_carriers() {
    let collected = testing::collect_with(vec![basic_plugin()], quote!(export), quote! { mod src { #[t::item] pub struct S; } });
    let out = collected.output.to_string();
    assert!(out.contains("# [macro_export] macro_rules ! __pl_carrier_"), "{out}");
    assert!(out.contains("pub use __pl_carrier_"), "{out}");
}

#[test]
fn conditional_records_get_a_carrier_pair() {
    let collected = testing::collect(
        vec![basic_plugin()],
        quote! {
            mod src {
                #[t::item] pub struct Always;
                #[cfg(feature = "x")] #[t::item] pub struct Sometimes;
                #[cfg(unix)] pub mod nested { #[cfg(feature = "y")] #[t::item] pub struct Deep; }
            }
        },
    );
    let out = collected.output.to_string();
    assert!(out.contains("# [cfg (feature = \"x\")] # [doc (hidden)] # [allow (unused_macros)] macro_rules ! __pl_rec_1_"), "{out}");
    assert!(out.contains("# [cfg (not (feature = \"x\"))]"), "{out}");
    assert!(out.contains("# [cfg (all (unix , feature = \"y\"))]"), "{out}");
    assert!(out.contains("pub (crate) use __pl_rec_1_"), "{out}");
    assert_eq!(collected.records[2].condition().unwrap().to_string(), "all (unix , feature = \"y\")");
}

#[test]
fn carrier_names_depend_on_where_the_module_is() {
    // Same name, same body, different place: the hidden names differ, so two
    // exported sources with identical text in one crate don't clash at the
    // crate root (SPEC section 9, item 11). Outside the compiler the "place"
    // is the line/column proc-macro2 gives tokens parsed from a string; in a
    // real build it is the module's `file:line`.
    let text = "mod src { #[t::item] pub struct S; }";
    let at_line_1: TokenStream = text.parse().unwrap();
    let at_line_3: TokenStream = format!("\n\n{text}").parse().unwrap();
    let name = |out: &TokenStream| {
        let out = out.to_string();
        let start = out.find("__pl_carrier_").unwrap();
        out[start..start + "__pl_carrier_".len() + 16].to_string()
    };
    let a = testing::collect(vec![basic_plugin()], at_line_1.clone());
    let b = testing::collect(vec![basic_plugin()], at_line_3);
    let again = testing::collect(vec![basic_plugin()], at_line_1);
    assert_ne!(name(&a.output), name(&b.output), "{}", a.output);
    assert_eq!(name(&a.output), name(&again.output), "same place, same name (G1)");
    // The alias every caller uses does not change.
    assert!(a.output.to_string().contains("as __pl_carrier ;"));
}

// ───────────────────────────── canonical order (R5) ─────────────────────────────

#[test]
fn canonical_order_follows_list_then_sequence() {
    let a = testing::collect(vec![basic_plugin()], quote! { mod a { #[t::item] pub struct A1; #[t::item] pub struct A2; } });
    let b = testing::collect(vec![basic_plugin()], quote! { mod b { #[t::item] pub struct B1; } });
    let names = |state: &State| state.records().iter().map(|r| r.ident().unwrap().to_string()).collect::<Vec<_>>();

    let state = testing::state(vec![("crate::a", a.records.clone()), ("crate::b", b.records.clone())]);
    assert_eq!(names(&state), ["A1", "A2", "B1"]);

    let state = testing::state(vec![("crate::b", b.records), ("crate::a", a.records)]);
    assert_eq!(names(&state), ["B1", "A1", "A2"]);
    assert_eq!(state.records()[1].source_index(), 1);
    assert!(state.source_of(&state.records()[0]).is_local());
}

#[test]
fn state_records_out_of_order_are_sorted() {
    let a = testing::collect(vec![basic_plugin()], quote! { mod a { #[t::item] pub struct A1; #[t::item] pub struct A2; } });
    let mut recs = a.records;
    recs.reverse();
    let state = testing::state(vec![("crate::a", recs)]);
    let seqs: Vec<u32> = state.records().iter().map(|r| r.sequence()).collect();
    assert_eq!(seqs, [0, 1]);
}

// ───────────────────────────── handler ordering (R6) ─────────────────────────────

#[test]
fn handlers_run_in_dependency_order_with_registration_tie_break() {
    let out = testing::finish(vec![ordering_plugin()], vec![]);
    assert!(!out.has_errors(), "{:?}", out.errors);
    let text = out.output.to_string();
    assert_eq!(sources_of(&text).len(), 4);
    assert!(position(&text, "MAKES_A") < position(&text, "NEEDS_A"));
    assert!(position(&text, "NEEDS_A") < position(&text, "NEEDS_B"));
    // `independent` was registered second and nothing holds it back.
    assert!(position(&text, "INDEPENDENT") < position(&text, "MAKES_A"));
}

#[test]
fn a_fact_cycle_is_an_error_naming_the_handlers() {
    fn a(_b: &B, _cx: &mut GlobalCx) -> Result<A> {
        unreachable!()
    }
    fn b(_a: &A, _cx: &mut GlobalCx) -> Result<B> {
        unreachable!()
    }
    let plugin = Plugin::new("c", "cycle-plugin", "0.1.0").global(a).global(b);
    let out = testing::finish(vec![plugin], vec![]);
    assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
    let e = &out.errors[0];
    assert!(e.starts_with("global handlers depend on each other in a cycle: "), "{e}");
    assert!(e.contains("::a` (plugin `cycle-plugin`)") && e.contains("::b` (plugin `cycle-plugin`)"), "{e}");
}

#[test]
fn a_missing_producer_is_an_error_naming_the_handler_and_fact() {
    fn needs(_a: &A, _cx: &mut GlobalCx) -> Result<()> {
        unreachable!()
    }
    let plugin = Plugin::new("m", "missing-plugin", "0.1.0").global(needs);
    let out = testing::finish(vec![plugin], vec![]);
    assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
    let e = &out.errors[0];
    assert!(e.contains("::needs` (plugin `missing-plugin`) reads fact `engine::A`, but no handler in this bundle produces it"), "{e}");
}

#[test]
fn a_duplicate_producer_is_an_error_naming_both() {
    fn one(_s: &State, _cx: &mut GlobalCx) -> Result<A> {
        Ok(A(1))
    }
    fn two(_s: &State, _cx: &mut GlobalCx) -> Result<A> {
        Ok(A(2))
    }
    let plugin = Plugin::new("d", "dup-plugin", "0.1.0").global(one).global(two);
    let out = testing::finish(vec![plugin], vec![]);
    assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
    let e = &out.errors[0];
    assert!(e.contains("fact `engine::A` has two producers:"), "{e}");
    assert!(e.contains("::one` (plugin `dup-plugin`)") && e.contains("::two` (plugin `dup-plugin`)"), "{e}");
}

#[test]
fn dependents_of_a_failed_handler_are_skipped_silently() {
    fn fails(_s: &State, _cx: &mut GlobalCx) -> Result<A> {
        Err(plenary_plugin::Error::call_site("link failed"))
    }
    fn needs(_a: &A, cx: &mut GlobalCx) -> Result<()> {
        emit_marker(cx, "SHOULD_NOT_RUN");
        Ok(())
    }
    let plugin = Plugin::new("f", "fail-plugin", "0.1.0").global(fails).global(needs);
    let out = testing::finish(vec![plugin], vec![]);
    assert_eq!(out.errors, vec!["link failed"]);
    assert!(!out.output.to_string().contains("SHOULD_NOT_RUN"));
}

// ───────────────────────────── determinism (G1) ─────────────────────────────

#[test]
fn collect_and_finish_are_deterministic() {
    let module = quote! {
        mod src {
            #[t::item] pub struct S { pub f: i64 }
            #[cfg(feature = "x")] #[t::item] pub struct C;
            pub fn body() { let _ = t::call!(a); let _ = t::call!(b); }
        }
    };
    let first = testing::collect(vec![basic_plugin()], module.clone());
    let second = testing::collect(vec![basic_plugin()], module);
    assert_eq!(first.output.to_string(), second.output.to_string());

    let bundle = || Bundle::new("test_bundle", vec![basic_plugin(), ordering_plugin()]).unwrap();
    let input = testing::finish_input(&bundle(), vec![("crate::src", first.records.clone())]);
    let a = finish_impl(&bundle(), input.clone());
    let b = finish_impl(&bundle(), input);
    assert_eq!(a.to_string(), b.to_string());
}

// ───────────────────────────── panic capture (R11) ─────────────────────────────

#[test]
fn a_panicking_global_handler_becomes_an_error() {
    fn explode(_s: &State, _cx: &mut GlobalCx) -> Result<()> {
        panic!("kaboom {}", 42)
    }
    let plugin = Plugin::new("p", "panicky-plugin", "0.1.0").global(explode);
    let out = testing::finish(vec![plugin], vec![]);
    assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
    let e = &out.errors[0];
    assert!(e.starts_with("plugin `panicky-plugin` panicked in global handler `engine::"), "{e}");
    assert!(e.ends_with("::explode`: kaboom 42"), "{e}");
}

#[test]
fn a_panicking_local_handler_taints_the_record() {
    fn explode(_r: &Record, _cx: &mut LocalCx) -> Result<()> {
        panic!("local boom")
    }
    let plugin = Plugin::new("p", "panicky-plugin", "0.1.0").attribute_kind("thing", explode);
    let collected = testing::collect(vec![plugin], quote! { mod m { #[p::thing] pub struct S; #[p::thing] pub struct T; } });
    assert_eq!(collected.records.len(), 2);
    assert!(collected.records.iter().all(|r| r.is_tainted()));
    assert_eq!(collected.diagnostics[0], "error: plugin `panicky-plugin` panicked in the local handler for `p::thing`: local boom");
    assert!(collected.output.to_string().contains("pub struct S ;"), "the item is still emitted");
}

// ───────────────────────────── collector diagnostics ─────────────────────────────

#[test]
fn unknown_kind_in_a_known_namespace_suggests_the_right_one() {
    let collected = testing::collect(vec![basic_plugin()], quote! { mod m { #[t::itme] pub struct S; } });
    assert_eq!(collected.diagnostics, vec!["error: unknown kind `itme` in namespace `t`; did you mean `item`? (kinds of `t`: item, call)"]);
    assert!(collected.records.is_empty());
    assert!(!collected.output.to_string().contains("t :: itme"), "the attribute is still stripped");
}

#[test]
fn unknown_namespaces_are_not_touched() {
    let collected = testing::collect(vec![basic_plugin()], quote! { mod m { #[serde::rename = "x"] #[derive(Debug)] pub struct S; } });
    assert!(collected.diagnostics.is_empty());
    assert!(collected.output.to_string().contains("# [serde :: rename = \"x\"]"));
}

#[test]
fn call_kind_used_as_attribute_is_explained() {
    let collected = testing::collect(vec![basic_plugin()], quote! { mod m { #[t::call] pub struct S; } });
    assert_eq!(collected.diagnostics, vec!["error: `t::call` is an call kind; write it as a call: `t::call!(...)`"]);
}

#[test]
fn nested_sources_are_rejected_and_the_source_fails() {
    let collected = testing::collect(
        vec![basic_plugin()],
        quote! { mod outer { #[t::item] pub struct S; #[test_bundle::collect] pub mod inner { #[t::item] pub struct I; } } },
    );
    assert_eq!(collected.diagnostics, vec!["error: a source can't contain another source (R12): remove this `collect` from `inner` or move the module out of `outer`"]);
    let out = collected.output.to_string();
    assert!(out.contains("ver 2 ; failed ;"), "the carrier is marked failed: {out}");
    assert!(!out.contains("test_bundle :: collect"), "the inner attribute is removed: {out}");
}

#[test]
fn cfg_attr_wrapping_cfg_or_plugin_attributes_is_rejected() {
    let collected = testing::collect(
        vec![basic_plugin()],
        quote! { mod m {
            #[cfg_attr(feature = "x", t::item)] pub struct S;
            #[cfg_attr(feature = "y", cfg(unix))] #[t::item] pub struct U;
            #[cfg_attr(feature = "z", derive(Debug))] #[t::item] pub struct Fine;
        } },
    );
    assert_eq!(
        collected.diagnostics,
        vec![
            "error: `cfg_attr` wrapping `t::item` isn't supported in v0 (R7); write the attribute directly with its own `#[cfg(..)]`",
            "error: `cfg_attr` wrapping `cfg` isn't supported in v0 (R7); write the attribute directly with its own `#[cfg(..)]`",
        ]
    );
}

#[test]
fn call_without_replacement_in_expression_position_is_an_error() {
    fn silent(_r: &Record, _cx: &mut LocalCx) -> Result<()> {
        Ok(())
    }
    let plugin = Plugin::new("s", "silent-plugin", "0.1.0").call_kind("thing", silent);
    let collected = testing::collect(vec![plugin], quote! { mod m { pub fn f() -> u8 { s::thing!() } s::thing!{} } });
    let out = collected.output.to_string();
    assert!(out.contains("compile_error ! { \"`s::thing!` is used where a value is needed, but its local handler set no replacement\" }"), "{out}");
    assert_eq!(collected.records.len(), 2, "item-position calls simply vanish");
}

#[test]
fn marker_paths_depend_on_depth() {
    let collected = testing::collect(
        vec![basic_plugin()],
        quote! { mod m {
            pub fn top() -> u8 { t::call!(0) }
            pub mod a { pub mod b { pub fn deep() -> u8 { t::call!(1) } } }
        } },
    );
    let out = collected.output.to_string();
    assert!(out.contains("< self :: __pl_m0 as"), "{out}");
    assert!(out.contains("< super :: super :: __pl_m1 as"), "{out}");
    assert!(out.contains("pub struct __pl_m0 ;") && out.contains("pub struct __pl_m1 ;"), "{out}");
}

#[test]
fn out_of_line_modules_are_rejected() {
    let collected = testing::collect(vec![basic_plugin()], quote! { pub mod m; });
    assert_eq!(collected.diagnostics.len(), 1);
    assert!(collected.diagnostics[0].contains("must be placed on an inline module"));
}

// ───────────────────────────── finish checks (R9, R10) ─────────────────────────────

#[test]
fn carrier_version_mismatch_names_source_and_versions() {
    let path = vec![Ident::new("crate", Span::call_site()), Ident::new("legacy", Span::call_site())];
    let err = decode_source(0, path, quote!(ver 1 ;)).unwrap_err();
    assert_eq!(
        err.to_string(),
        "carrier version mismatch: source `crate::legacy` was collected with carrier format v1, but this bundle expects v2. Rebuild that crate against the same plenary version."
    );
}

#[test]
fn records_of_unknown_namespaces_and_kinds_are_ignored_at_finish() {
    let other = Plugin::new("other", "other-plugin", "0.1.0").attribute_kind("thing", plenary_plugin::keep);
    let collected = testing::collect(vec![basic_plugin(), other], quote! { mod m { #[other::thing] pub struct O; #[t::item] pub struct S; } });
    assert_eq!(collected.records.len(), 2);

    fn count(state: &State, cx: &mut GlobalCx) -> Result<()> {
        let n = state.records().len();
        cx.emit(quote!(const COUNT: usize = #n;));
        Ok(())
    }
    // A bundle that only knows `t` sees one record.
    let plugin = basic_plugin().global(count);
    let out = testing::finish(vec![plugin], vec![("crate::m", collected.records)]);
    assert!(!out.has_errors(), "{:?}", out.errors);
    assert!(out.output.to_string().contains("const COUNT : usize = 1usize ;"), "{}", out.output);
}

#[test]
fn plugin_major_version_mismatch_is_an_error() {
    let bundle = Bundle::new("test_bundle", vec![basic_plugin()]).unwrap();
    let input: TokenStream = quote! {
        plenary = [ ::plenary ] {
            source [ crate :: old ] { ver 2 ; plugin "t" "test-plugin" "2.3.0" ; }
        }
    };
    let out = finish_impl(&bundle, input).to_string();
    assert!(
        out.contains("source `crate::old` was collected by plugin `test-plugin` v2.3.0, but this bundle has `test-plugin` v0.1.0 (major versions differ)"),
        "{out}"
    );
}

#[test]
fn duplicate_sources_in_one_list_are_an_error() {
    let bundle = Bundle::new("test_bundle", vec![basic_plugin()]).unwrap();
    let input: TokenStream = quote! {
        plenary = [ ::plenary ] {
            source [ crate :: a ] { ver 2 ; }
            source [ crate :: a ] { ver 2 ; }
        }
    };
    let out = finish_impl(&bundle, input).to_string();
    assert!(out.contains("source `crate::a` is listed twice in this `expand!`"), "{out}");
}

#[test]
fn failed_sources_are_skipped_without_errors() {
    let bundle = Bundle::new("test_bundle", vec![basic_plugin()]).unwrap();
    let input: TokenStream = quote! {
        plenary = [ ::plenary ] { source [ crate :: broken ] { ver 2 ; failed ; } }
    };
    let out = finish_impl(&bundle, input).to_string();
    assert!(!out.contains("compile_error"), "{out}");
    assert!(out.contains("const SOURCES : & 'static [& 'static str] = [\"crate::broken\"]") || out.contains("\"crate::broken\""), "{out}");
}

#[test]
fn the_collection_marker_implements_collection() {
    let out = testing::finish(vec![basic_plugin()], vec![("crate::a", vec![]), ("crate::b", vec![])]);
    let text = out.output.to_string();
    assert!(text.contains("pub struct __Collection ;"), "{text}");
    assert!(text.contains("impl :: plenary :: Collection for __Collection"), "{text}");
    assert!(text.contains("[\"crate::a\" , \"crate::b\"]"), "{text}");
}
