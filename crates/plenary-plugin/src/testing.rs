//! Helpers for testing plugins as plain functions, with no compiler involved.
//!
//! ```ignore
//! use plenary_plugin::testing;
//! use quote::quote;
//!
//! let collected = testing::collect(vec![stq_lite_plugin::plugin()], quote! {
//!     mod users {
//!         #[stq::table(name = "users")] pub struct Users { pub id: i64 }
//!         #[stq::query(sql = "SELECT * FROM users")] pub struct All {}
//!     }
//! });
//! let state = testing::state(vec![("crate::users", collected.records)]);
//! let mut cx = testing::global_cx();
//! let resolved = stq_lite_plugin::link(&state, &mut cx).unwrap();
//! ```

use crate::carrier::{decode_source, encode_source_body, DecodedSource, EncodedFact, SourceDesc};
use crate::collect::collect_full;
use crate::diag::Diagnostic;
use crate::finish::finish_impl;
use crate::global::GlobalCx;
use crate::plugin::{Bundle, Plugin};
use crate::record::{PluginVersion, Record, SourceInfo, SourceRef, State};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

/// What [`collect`] returns.
pub struct Collected {
    /// The expanded module, exactly as the `collect` attribute would emit it.
    pub output: TokenStream,
    /// The records the collector found, with their annotations, markers and
    /// taint flags, as a collection would see them.
    pub records: Vec<Record>,
    /// The diagnostics the collector reported (as text).
    pub diagnostics: Vec<String>,
}

/// Run the collector over an inline module with these plugins.
pub fn collect(plugins: Vec<Plugin>, module: TokenStream) -> Collected {
    collect_with(plugins, TokenStream::new(), module)
}

/// Like [`collect`], with `collect(args)`.
pub fn collect_with(plugins: Vec<Plugin>, args: TokenStream, module: TokenStream) -> Collected {
    let bundle = Bundle::new("test_bundle", plugins).expect("distinct namespaces");
    let collected = collect_full(&bundle, args, module);
    let diagnostics = collected.diagnostics.iter().map(|d| d.to_string()).collect();
    Collected { output: collected.output, records: collected.records, diagnostics }
}

/// Decode the records back out of collector output, through the carrier
/// token format (so this exercises encode and decode).
pub fn records_in_output(output: &TokenStream) -> Vec<Record> {
    let mut records = records_from_output(output);
    records.sort_by_key(|r| r.sequence());
    records
}

/// Find `source` descriptions in collector output: the record tokens inside
/// the carrier bodies. Every `rec { .. }` block is decoded; the header is
/// reconstructed from the main carrier.
fn records_from_output(output: &TokenStream) -> Vec<Record> {
    let text_recs = find_rec_blocks(output.clone());
    let ver = proc_macro2::Literal::u32_unsuffixed(crate::carrier::CARRIER_FORMAT_VERSION);
    let header = quote!(ver #ver ;);
    let body: TokenStream = std::iter::once(header).chain(text_recs).collect();
    let fake_path = vec![Ident::new("crate", Span::call_site()), Ident::new("test", Span::call_site())];
    match decode_source(0, fake_path, body) {
        Ok(d) => d.records.into_iter().map(|mut r| {
            r.source = None;
            r
        }).collect(),
        Err(e) => panic!("could not decode carrier records: {e}"),
    }
}

fn find_rec_blocks(tokens: TokenStream) -> Vec<TokenStream> {
    use proc_macro2::TokenTree;
    let mut out = Vec::new();
    let tts: Vec<TokenTree> = tokens.into_iter().collect();
    let mut i = 0;
    while i < tts.len() {
        match &tts[i] {
            TokenTree::Ident(id) if id == "rec" => {
                if let Some(TokenTree::Group(g)) = tts.get(i + 1) {
                    if g.delimiter() == proc_macro2::Delimiter::Brace {
                        let inner = g.stream();
                        out.push(quote!(rec { #inner }));
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
            TokenTree::Group(g) => {
                out.extend(find_rec_blocks(g.stream()));
                i += 1;
            }
            _ => i += 1,
        }
    }
    // A conditional record's carrier pair has the record only in the live
    // copy, so nothing is duplicated.
    out
}

fn diagnostics_from_output(output: &TokenStream) -> Vec<String> {
    use proc_macro2::TokenTree;
    let mut out = Vec::new();
    let tts: Vec<TokenTree> = output.clone().into_iter().collect();
    for (i, tt) in tts.iter().enumerate() {
        if let TokenTree::Ident(id) = tt {
            if id == "compile_error" {
                if let Some(TokenTree::Group(g)) = tts.get(i + 2) {
                    out.push(g.stream().to_string().trim_matches('"').replace("\\n", "\n"));
                }
            }
        }
        if let TokenTree::Group(g) = tt {
            out.extend(diagnostics_from_output(&g.stream()));
        }
    }
    out
}

/// Build a `State` from `(listed path, records)` pairs, in list order.
/// Records are given their source and sorted into canonical order.
pub fn state(sources: Vec<(&str, Vec<Record>)>) -> State {
    let mut infos = Vec::new();
    let mut records = Vec::new();
    for (index, (path, recs)) in sources.into_iter().enumerate() {
        let path: Vec<Ident> = path.split("::").map(|s| Ident::new(s, Span::call_site())).collect();
        infos.push(SourceInfo { index, path: path.clone(), export: false, failed: false, stage: false, plugins: Vec::new() });
        for mut r in recs {
            r.source = Some(SourceRef { index, path: path.clone() });
            records.push(r);
        }
    }
    State::new(infos, records)
}

/// A fresh global context whose `plenary` path is `::plenary`, with no
/// exportable types registered (`cx.export` reports an error).
pub fn global_cx() -> GlobalCx {
    GlobalCx::new(quote!(::plenary), Span::call_site(), Vec::new())
}

/// An empty `Imported<F>`, for calling a handler that reads one directly.
pub fn imported<F: crate::ExportFact>() -> crate::Imported<F> {
    crate::Imported::empty()
}

/// An `Imported<F>` holding these `(listed stage path, fact)` pairs, in order.
pub fn imported_from<F: crate::ExportFact>(facts: Vec<(&str, F)>) -> crate::Imported<F> {
    let mut out = crate::Imported::empty();
    for (index, (path, fact)) in facts.into_iter().enumerate() {
        let path: Vec<Ident> = path.split("::").map(|s| Ident::new(s, Span::call_site())).collect();
        out.push(SourceInfo { index, path, export: true, failed: false, stage: true, plugins: Vec::new() }, fact);
    }
    out
}

/// Like [`global_cx`], knowing the exportable types these plugins register,
/// so `cx.export` works; see [`GlobalCx::exports`](crate::GlobalCx::exports).
pub fn global_cx_with(plugins: Vec<Plugin>) -> GlobalCx {
    let bundle = Bundle::new("test_bundle", plugins).expect("distinct namespaces");
    GlobalCx::new(quote!(::plenary), Span::call_site(), crate::global::ExportableInfo::of_bundle(&bundle))
}

/// What [`finish`] returns.
pub struct Finished {
    /// Everything the collection emits, as tokens: the marker, the processed
    /// items, the obligations, the stage carrier and the diagnostics.
    pub output: TokenStream,
    /// Error messages found in the output.
    pub errors: Vec<String>,
}

impl Finished {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// The collection's stage block, decoded: its exported facts (not
    /// rebased) and the records found in the emitted items (only the
    /// unconditional ones: conditional records sit in their own carriers).
    pub fn stage(&self) -> DecodedSource {
        stage_of(&self.output).expect("the output holds a stage carrier")
    }
}

/// One listed entry for [`finish_blocks`]: a source or a stage.
pub struct SourceBlock {
    path: String,
    desc: SourceDesc,
    records: Vec<Record>,
    /// Override the `plugin` lines (default: the bundle's plugins).
    plugins: Option<Vec<PluginVersion>>,
}

impl SourceBlock {
    /// A source listed as `path`.
    pub fn new(path: &str) -> Self {
        SourceBlock { path: path.to_string(), desc: SourceDesc::default(), records: Vec::new(), plugins: None }
    }

    pub fn records(mut self, records: Vec<Record>) -> Self {
        self.records = records;
        self
    }

    /// Mark the block as a stage (an exporting collection).
    pub fn stage(mut self) -> Self {
        self.desc.stage = true;
        self
    }

    pub fn export(mut self) -> Self {
        self.desc.export = true;
        self
    }

    pub fn failed(mut self) -> Self {
        self.desc.failed = true;
        self
    }

    /// Add an exported fact as `fact { ns #ns type #name ver #version { tokens } }`.
    pub fn fact(mut self, ns: &str, name: &str, version: &str, tokens: TokenStream) -> Self {
        self.desc.facts.push(EncodedFact {
            namespace: ns.into(),
            name: name.into(),
            version: version.into(),
            tokens,
            span: Span::call_site(),
        });
        self
    }

    /// Replace the `plugin ".." ".." ".." ;` lines.
    pub fn plugins(mut self, plugins: Vec<PluginVersion>) -> Self {
        self.plugins = Some(plugins);
        self
    }
}

/// Run the whole `__finish` pipeline over sources described as
/// `(listed path, records)` pairs, the way `expand!` would feed it.
pub fn finish(plugins: Vec<Plugin>, sources: Vec<(&str, Vec<Record>)>) -> Finished {
    finish_blocks(plugins, sources.into_iter().map(|(p, r)| SourceBlock::new(p).records(r)).collect())
}

/// Like [`finish`], with full control over each block (stages, facts).
pub fn finish_blocks(plugins: Vec<Plugin>, blocks: Vec<SourceBlock>) -> Finished {
    finish_blocks_with(plugins, false, blocks)
}

/// Like [`finish_blocks`], for `expand!(bundle; export; ..)` when `export`.
pub fn finish_blocks_with(plugins: Vec<Plugin>, export: bool, blocks: Vec<SourceBlock>) -> Finished {
    let bundle = Bundle::new("test_bundle", plugins).expect("distinct namespaces");
    let input = finish_input_blocks(&bundle, export, blocks);
    let output = finish_impl(&bundle, input);
    let errors = diagnostics_from_output(&output);
    Finished { output, errors }
}

/// The exact tokens `expand!` would hand to `__finish` for these sources.
pub fn finish_input(bundle: &Bundle, sources: Vec<(&str, Vec<Record>)>) -> TokenStream {
    finish_input_blocks(bundle, false, sources.into_iter().map(|(p, r)| SourceBlock::new(p).records(r)).collect())
}

/// The exact tokens `expand!` would hand to `__finish` for these blocks.
pub fn finish_input_blocks(bundle: &Bundle, export: bool, blocks: Vec<SourceBlock>) -> TokenStream {
    let bundle_plugins: Vec<PluginVersion> = bundle
        .plugins()
        .iter()
        .map(|p| PluginVersion { namespace: p.namespace().into(), name: p.name().into(), version: p.version().into() })
        .collect();
    let blocks = blocks.into_iter().map(|b| {
        let path: Vec<Ident> = b.path.split("::").map(|s| Ident::new(s, Span::call_site())).collect();
        let mut desc = b.desc;
        desc.plugins = b.plugins.unwrap_or_else(|| bundle_plugins.clone());
        let body = encode_source_body(&desc, &b.records);
        quote!(source [ #(#path)::* ] { #body })
    });
    let export = export.then(|| quote!(export));
    quote!(plenary = [ ::plenary ] bundle = [ test_bundle ] #export { #(#blocks)* })
}

/// Find and decode the stage block in collection output: the first
/// `source [ .. ] { .. }` inside the output's carrier.
pub fn stage_of(output: &TokenStream) -> Option<DecodedSource> {
    use proc_macro2::TokenTree;
    fn find(tokens: TokenStream) -> Option<(Vec<Ident>, TokenStream)> {
        let tts: Vec<TokenTree> = tokens.into_iter().collect();
        for (i, tt) in tts.iter().enumerate() {
            if let TokenTree::Ident(id) = tt {
                if id == "source" {
                    if let (Some(TokenTree::Group(p)), Some(TokenTree::Group(b))) = (tts.get(i + 1), tts.get(i + 2)) {
                        if p.delimiter() == proc_macro2::Delimiter::Bracket && b.delimiter() == proc_macro2::Delimiter::Brace {
                            // The path is `$ ( $ seg ) *` in a carrier: use a
                            // placeholder, the test knows what it listed.
                            let path = vec![Ident::new("crate", Span::call_site()), Ident::new("stage", Span::call_site())];
                            return Some((path, b.stream()));
                        }
                    }
                }
            }
            if let TokenTree::Group(g) = tt {
                if let Some(found) = find(g.stream()) {
                    return Some(found);
                }
            }
        }
        None
    }
    let (path, body) = find(output.clone())?;
    Some(decode_source(0, path, body).expect("the stage block decodes"))
}

/// Render diagnostics as `level: message` lines.
pub fn render(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics.iter().map(|d| d.to_string()).collect()
}
