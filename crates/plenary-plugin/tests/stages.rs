//! Stage tests (SPEC section 10), run without a compiler.

use plenary_plugin::facts::{self, Decode, Encode, Fields};
use plenary_plugin::testing::{self, SourceBlock};
use plenary_plugin::{ExportFact, Fact, GlobalCx, Imported, LocalCx, Plugin, Record, Result, State};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::parse::ParseStream;

// ───────────────────────────── a plugin with an exportable fact ─────────────────────────────

#[derive(Debug, PartialEq, Clone)]
struct Origin {
    path: syn::Path,
    stage: Option<String>,
}

impl Encode for Origin {
    fn encode(&self, out: &mut TokenStream) {
        Fields::new().with("path", &self.path).with("stage", &self.stage).encode_into(out)
    }
}

impl Decode for Origin {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let f = Fields::parse(input)?;
        Ok(Origin { path: f.get("path")?, stage: f.get("stage")? })
    }
}

/// The names of every `t::item` record a collection saw, plus where they came
/// from. Exported, so a later collection can build on it.
#[derive(Debug, PartialEq, Clone)]
struct Names {
    names: Vec<String>,
    origins: Vec<Origin>,
    rounds: u32,
}

impl Fact for Names {}

impl ExportFact for Names {
    const NAME: &'static str = "Names";
    fn to_tokens(&self) -> TokenStream {
        Fields::new().with("names", &self.names).with("origins", &self.origins).with("rounds", &self.rounds).encode()
    }
    fn from_tokens(tokens: TokenStream) -> Result<Self> {
        let f = Fields::decode(tokens)?;
        Ok(Names { names: f.get("names")?, origins: f.get("origins")?, rounds: f.get("rounds")? })
    }
}

fn item(record: &Record, cx: &mut LocalCx) -> Result<()> {
    cx.annotate("seen", record.ident().cloned().unwrap_or_else(|| Ident::new("anon", Span::call_site())));
    Ok(())
}

fn call(_record: &Record, cx: &mut LocalCx) -> Result<()> {
    let m = cx.marker();
    cx.set_replacement(quote!(<#m as ::some::Trait>::VALUE));
    Ok(())
}

/// Merge what the listed stages exported with this collection's own records,
/// export the result, and emit one item that is itself a record (so the next
/// collection sees it in the round).
fn names(state: &State, upstream: &Imported<Names>, cx: &mut GlobalCx) -> Result<Names> {
    let mut out = Names { names: Vec::new(), origins: Vec::new(), rounds: 0 };
    for (stage, n) in upstream.iter() {
        out.names.extend(n.names.iter().cloned());
        out.origins.extend(n.origins.iter().cloned().map(|mut o| {
            o.stage = Some(stage.display_path());
            o
        }));
        out.rounds = out.rounds.max(n.rounds + 1);
    }
    for r in state.records_of("t", "item") {
        let name = r.ident().map(|i| i.to_string()).unwrap_or_default();
        out.names.push(name);
        out.origins.push(Origin { path: r.absolute_path(), stage: None });
    }
    cx.export(&out);
    let lits = out.names.iter().map(|n| proc_macro2::Literal::string(n));
    let n = out.names.len();
    cx.emit(quote! {
        pub const NAMES: &[&str] = &[#(#lits),*];
        pub const COUNT: usize = #n;
        #[t::item] pub struct Generated;
    });
    Ok(out)
}

fn stage_plugin() -> Plugin {
    Plugin::new("t", "stage-test-plugin", "0.1.0")
        .attribute_kind("item", item)
        .call_kind("call", call)
        .exportable::<Names>()
        .global(names)
}

fn collect(module: TokenStream) -> Vec<Record> {
    let c = testing::collect(vec![stage_plugin()], module);
    assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
    c.records
}

// ───────────────────────────── codec round trips ─────────────────────────────

#[test]
fn an_export_fact_round_trips_through_its_tokens() {
    let value = Names {
        names: vec!["A".into(), "b c".into()],
        origins: vec![Origin { path: syn::parse_quote!(crate::m::A), stage: None }, Origin { path: syn::parse_quote!(up::x::B), stage: Some("up::plan".into()) }],
        rounds: 3,
    };
    let tokens = value.to_tokens();
    assert_eq!(
        tokens.to_string(),
        "{ names : [\"A\" , \"b c\" ,] , origins : [{ path : crate :: m :: A , stage : None , } , { path : up :: x :: B , stage : Some (\"up::plan\") , } ,] , rounds : 3 , }"
    );
    assert_eq!(Names::from_tokens(tokens).unwrap(), value);
}

#[test]
fn every_builtin_codec_round_trips() {
    let s: String = facts::decode(facts::encode("x")).unwrap();
    assert_eq!(s, "x");
    let lit = syn::LitStr::new("spanned", Span::call_site());
    let back: syn::LitStr = facts::decode(facts::encode(&lit)).unwrap();
    assert_eq!(back.value(), "spanned");
    assert_eq!(facts::decode::<bool>(facts::encode(&false)).unwrap(), false);
    assert_eq!(facts::decode::<i32>(facts::encode(&-7i32)).unwrap(), -7);
    assert_eq!(facts::decode::<u64>(facts::encode(&u64::MAX)).unwrap(), u64::MAX);
    assert_eq!(facts::decode::<usize>(facts::encode(&12usize)).unwrap(), 12);
    let p: syn::Path = syn::parse_quote!(crate::a::b::C);
    assert_eq!(facts::decode::<syn::Path>(facts::encode(&p)).unwrap(), p);
    let i = Ident::new("ident", Span::call_site());
    assert_eq!(facts::decode::<Ident>(facts::encode(&i)).unwrap(), i);
    let _: Span = facts::decode(facts::encode(&Span::call_site())).unwrap();
    let v = vec![Some(("k".to_string(), 1u8)), None];
    assert_eq!(facts::decode::<Vec<Option<(String, u8)>>>(facts::encode(&v)).unwrap(), v);
    let ts = quote!(any { tokens } $ here);
    assert_eq!(facts::decode::<TokenStream>(facts::encode(&ts)).unwrap().to_string(), ts.to_string());
}

// ───────────────────────────── exporting ─────────────────────────────

#[test]
fn a_collection_exports_facts_and_the_records_of_its_emitted_items() {
    let a = collect(quote! { mod a { #[t::item] pub struct A1; #[t::item] pub struct A2; } });
    let out = testing::finish(vec![stage_plugin()], vec![("crate::a", a)]);
    assert!(!out.has_errors(), "{:?}", out.errors);
    let text = out.output.to_string();

    // The handler's items are emitted with the plugin attribute stripped ...
    assert!(text.contains("pub const NAMES : & [& str] = & [\"A1\" , \"A2\"] ;"), "{text}");
    let items = &text[..text.find("macro_rules").unwrap()];
    assert!(items.contains("pub struct Generated ;") && !items.contains("# [t :: item]"), "{items}");
    // ... and a stage carrier follows, with the alias every caller uses.
    assert!(text.contains("macro_rules ! __pl_carrier_"), "{text}");
    assert!(text.contains("pub (crate) use __pl_carrier_") && text.contains("as __pl_carrier ;"), "{text}");

    let stage = out.stage();
    assert!(stage.info.is_stage());
    assert!(!stage.info.is_export());
    assert_eq!(stage.info.plugins().len(), 1);
    assert_eq!(stage.info.plugins()[0].name, "stage-test-plugin");
    // The exported fact ...
    assert_eq!(stage.facts.len(), 1);
    let f = &stage.facts[0];
    assert_eq!((f.namespace.as_str(), f.name.as_str(), f.version.as_str()), ("t", "Names", "0.1.0"));
    let names = Names::from_tokens(f.tokens.clone()).unwrap();
    assert_eq!(names.names, ["A1", "A2"]);
    assert_eq!(names.origins[0].path, syn::parse_quote!(crate::a::A1));
    // ... and the record found in the emitted items, with a relative path
    // under the collection site.
    assert_eq!(stage.records.len(), 1);
    assert_eq!(stage.records[0].relative_path().iter().map(|i| i.to_string()).collect::<Vec<_>>(), ["Generated"]);
    assert_eq!(stage.records[0].annotation("seen").unwrap().to_string(), "Generated");
}

#[test]
fn export_marks_the_stage_carrier_macro_export() {
    let out = testing::finish_blocks_with(vec![stage_plugin()], true, vec![SourceBlock::new("crate::a")]);
    assert!(!out.has_errors(), "{:?}", out.errors);
    let text = out.output.to_string();
    assert!(text.contains("# [macro_export] macro_rules ! __pl_carrier_"), "{text}");
    assert!(text.contains("pub use __pl_carrier_"), "{text}");
    assert!(out.stage().info.is_export());
}

#[test]
fn the_one_round_rule_an_emitted_record_is_not_in_its_own_state() {
    // `Generated` is emitted by the handler; the handler's own State does not
    // have it (COUNT counts the listed sources' records only) ...
    let out = testing::finish(vec![stage_plugin()], vec![("crate::a", collect(quote! { mod a { #[t::item] pub struct A1; } }))]);
    assert!(out.output.to_string().contains("pub const COUNT : usize = 1usize ;"), "{}", out.output);
    // ... but a second collection listing the first sees it as a record.
    let stage = out.stage();
    let second = testing::finish_blocks(
        vec![stage_plugin()],
        vec![SourceBlock::new("crate::plan").stage().records(stage.records).fact("t", "Names", "0.1.0", stage.facts[0].tokens.clone())],
    );
    assert!(!second.has_errors(), "{:?}", second.errors);
    let text = second.output.to_string();
    assert!(text.contains("pub const NAMES : & [& str] = & [\"A1\" , \"Generated\"] ;"), "{text}");
    let merged = Names::from_tokens(second.stage().facts[0].tokens.clone()).unwrap();
    assert_eq!(merged.rounds, 1);
    assert_eq!(merged.origins[0].stage.as_deref(), Some("crate::plan"));
    assert_eq!(merged.origins[0].path, syn::parse_quote!(crate::a::A1), "a local stage keeps `crate::` paths");
    assert_eq!(merged.origins[1].path, syn::parse_quote!(crate::plan::Generated), "the stage is the record's source");
}

// ───────────────────────────── importing ─────────────────────────────

fn names_fact(names: &[&str], origin: &str) -> TokenStream {
    let path: syn::Path = syn::parse_str(origin).unwrap();
    Names { names: names.iter().map(|s| s.to_string()).collect(), origins: vec![Origin { path, stage: None }], rounds: 0 }.to_tokens()
}

#[test]
fn imported_facts_arrive_in_canonical_order_with_their_stage() {
    let out = testing::finish_blocks(
        vec![stage_plugin()],
        vec![
            SourceBlock::new("second::plan").stage().fact("t", "Names", "0.1.0", names_fact(&["s1"], "crate::x::A")),
            SourceBlock::new("crate::a").records(collect(quote! { mod a { #[t::item] pub struct Local; } })),
            SourceBlock::new("first::plan")
                .stage()
                .fact("t", "Names", "0.1.0", names_fact(&["f1"], "crate::y::B"))
                .fact("t", "Names", "0.1.0", names_fact(&["f2"], "self::z::C")),
        ],
    );
    assert!(!out.has_errors(), "{:?}", out.errors);
    // Imported facts first (list order, then export order), then the local records.
    assert!(out.output.to_string().contains("& [\"s1\" , \"f1\" , \"f2\" , \"Local\"]"), "{}", out.output);
    let merged = Names::from_tokens(out.stage().facts[0].tokens.clone()).unwrap();
    let origins: Vec<(String, String)> = merged
        .origins
        .iter()
        .map(|o| (quote::ToTokens::to_token_stream(&o.path).to_string(), o.stage.clone().unwrap_or_default()))
        .collect();
    assert_eq!(
        origins,
        [
            ("second :: x :: A".to_string(), "second::plan".to_string()),
            ("first :: y :: B".to_string(), "first::plan".to_string()),
            ("first :: plan :: z :: C".to_string(), "first::plan".to_string()),
            ("crate :: a :: Local".to_string(), String::new()),
        ],
        "paths are rebased from each stage's listed path"
    );
}

#[test]
fn a_fact_from_a_plugin_of_another_major_version_is_an_error() {
    let out = testing::finish_blocks(
        vec![stage_plugin()],
        vec![SourceBlock::new("up::plan").stage().fact("t", "Names", "2.0.0", names_fact(&["x"], "crate::a"))],
    );
    assert_eq!(
        out.errors,
        vec!["stage `up::plan` exported `t::Names` with plugin `stage-test-plugin` v2.0.0, but this bundle has `stage-test-plugin` v0.1.0 (major versions differ)"]
    );
    // The fact is dropped, the collection goes on.
    assert!(out.output.to_string().contains("pub const COUNT : usize = 0usize ;"), "{}", out.output);
}

#[test]
fn unknown_fact_types_are_ignored() {
    let out = testing::finish_blocks(
        vec![stage_plugin()],
        vec![
            SourceBlock::new("up::plan")
                .stage()
                .fact("other", "Thing", "0.1.0", quote!({ anything : 1 , }))
                .fact("t", "Nope", "0.1.0", quote!({ anything : 1 , }))
                .fact("t", "Names", "0.1.0", names_fact(&["kept"], "crate::a")),
        ],
    );
    assert!(!out.has_errors(), "{:?}", out.errors);
    assert!(out.output.to_string().contains("& [\"kept\"]"), "{}", out.output);
}

#[test]
fn an_undecodable_fact_is_an_error_naming_the_stage() {
    let out = testing::finish_blocks(
        vec![stage_plugin()],
        vec![SourceBlock::new("up::plan").stage().fact("t", "Names", "0.1.0", quote!({ names : [ "x" ] , }))],
    );
    assert_eq!(out.errors, vec!["could not decode `t::Names` exported by stage `up::plan`: exported fact has no field `origins`"]);
}

#[test]
fn the_stage_carrier_is_deterministic() {
    let make = || {
        testing::finish_blocks(
            vec![stage_plugin()],
            vec![
                SourceBlock::new("up::plan").stage().fact("t", "Names", "0.1.0", names_fact(&["u"], "crate::a")),
                SourceBlock::new("crate::a").records(collect(quote! { mod a { #[t::item] pub struct A1; pub fn f() -> u8 { t::call!(1) } } })),
            ],
        )
    };
    let (a, b) = (make(), make());
    assert!(!a.has_errors(), "{:?}", a.errors);
    assert_eq!(a.output.to_string(), b.output.to_string());
}

#[test]
fn exporting_an_unregistered_type_is_an_error_naming_type_and_plugin() {
    struct Secret;
    impl Fact for Secret {}
    impl ExportFact for Secret {
        const NAME: &'static str = "Secret";
        fn to_tokens(&self) -> TokenStream {
            quote!({})
        }
        fn from_tokens(_: TokenStream) -> Result<Self> {
            Ok(Secret)
        }
    }
    fn leak(_s: &State, cx: &mut GlobalCx) -> Result<()> {
        cx.export(&Secret);
        Ok(())
    }
    let plugin = Plugin::new("u", "unregistered-plugin", "0.1.0").global(leak);
    let out = testing::finish(vec![plugin], vec![]);
    assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
    assert!(
        out.errors[0].starts_with("plugin `unregistered-plugin` exported a `stages::")
            && out.errors[0].ends_with("Secret`, but no plugin in this bundle registered that type with `.exportable::<Secret>()`; add the registration to the plugin that defines the type"),
        "{}",
        out.errors[0]
    );
    assert!(out.stage().facts.is_empty());
}

#[test]
fn reading_imported_of_an_unregistered_type_is_a_clear_error() {
    fn reads(_i: &Imported<Names>, _cx: &mut GlobalCx) -> Result<()> {
        Ok(())
    }
    // `Names` is not registered in this plugin.
    let plugin = Plugin::new("r", "reader-plugin", "0.1.0").global(reads);
    let out = testing::finish(vec![plugin], vec![]);
    assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
    assert!(
        out.errors[0].contains("::reads` (plugin `reader-plugin`) reads `Imported<stages::Names>`, but no plugin in this bundle registered that fact type with `.exportable::<..>()`"),
        "{}",
        out.errors[0]
    );
}

#[test]
fn imported_is_always_available_even_when_nothing_is_listed() {
    let out = testing::finish(vec![stage_plugin()], vec![]);
    assert!(!out.has_errors(), "{:?}", out.errors);
    assert!(out.output.to_string().contains("pub const COUNT : usize = 0usize ;"), "{}", out.output);
}

#[test]
fn markers_in_emitted_items_are_planted_at_the_collection_site() {
    fn emits_call(_s: &State, cx: &mut GlobalCx) -> Result<()> {
        cx.emit(quote! { pub fn f() -> u8 { t::call!(7) } });
        Ok(())
    }
    let plugin = stage_plugin().global(emits_call);
    let out = testing::finish(vec![plugin], vec![]);
    assert!(!out.has_errors(), "{:?}", out.errors);
    let text = out.output.to_string();
    assert!(text.contains("pub fn f () -> u8 { < self :: __pl_m0 as :: some :: Trait > :: VALUE }"), "{text}");
    assert!(text.contains("pub struct __pl_m0 ;"), "{text}");
    let stage = out.stage();
    let call = stage.records.iter().find(|r| r.kind().is("t", "call")).expect("call record");
    assert_eq!(call.markers().len(), 1);
}

#[test]
fn a_failed_stage_and_a_stage_of_another_plugin_major_are_handled_like_sources() {
    let out = testing::finish_blocks(vec![stage_plugin()], vec![SourceBlock::new("up::plan").stage().failed()]);
    assert!(!out.has_errors(), "{:?}", out.errors);
    let out = testing::finish_blocks(
        vec![stage_plugin()],
        vec![SourceBlock::new("up::plan").stage().plugins(vec![plenary_plugin::PluginVersion {
            namespace: "t".into(),
            name: "stage-test-plugin".into(),
            version: "3.0.0".into(),
        }])],
    );
    assert_eq!(
        out.errors,
        vec!["stage `up::plan` was collected by plugin `stage-test-plugin` v3.0.0, but this bundle has `stage-test-plugin` v0.1.0 (major versions differ)"]
    );
}
