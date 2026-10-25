//! The carrier token format: how records travel from a source to a collection.
//!
//! A carrier is a `macro_rules!` whose body holds the source's records as
//! tokens. Identifiers and literals are the user's original tokens, so their
//! spans survive every `macro_rules!` hop. The layout, version 2 (v1 had the
//! same block layout but a different calling convention: `expand!` called
//! every carrier itself instead of carriers chaining to each other):
//!
//! ```text
//! source [ crate :: users :: users ] {
//!     ver 2 ;
//!     export ;                             // only for `collect(export)` / `expand!(b; export; ..)`
//!     failed ;                             // only when the collector failed (R11)
//!     stage ;                              // only for a stage (an exporting collection, SPEC section 10)
//!     plugin "stq" "stq-lite-plugin" "0.1.0" ;   // one per namespace that processed the source
//!     fact { ns "stq" type "Resolved" ver "0.1.0" { .. } }   // stages only: an exported fact
//!     rec {
//!         kind [ stq :: table ]
//!         form attr                        // or `form call`
//!         seq 3
//!         path [ inner Users ]
//!         cond [ feature = "extra" ]       // only for conditional records
//!         args { name = "users" }
//!         item { pub struct Users { .. } } // attribute form only
//!         ann { "key" { .. } .. }
//!         markers [ __pl_m0 ]
//!         flags [ tainted ]
//!     }
//! }
//! ```
//!
//! Bare `$` tokens inside the user's code are escaped (see
//! [`crate::util::escape_dollars`]) because the carrier body is a macro
//! transcriber.

use crate::record::{Form, Kind, PluginVersion, Record, SourceInfo, SourceRef};
use crate::util::{escape_dollars, path_string, unescape_dollars};
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::{quote, ToTokens};
use syn::ext::IdentExt;
use syn::parse::{ParseStream, Parser};
use syn::{braced, bracketed, LitInt, LitStr, Token};

/// Version of the carrier token format. Must match
/// `plenary::CARRIER_FORMAT_VERSION`.
pub const CARRIER_FORMAT_VERSION: u32 = 2;

/// A source as the collector describes it, before it becomes a `SourceInfo`.
#[derive(Clone, Debug, Default)]
pub struct SourceDesc {
    pub export: bool,
    pub failed: bool,
    /// An exporting collection rather than a source module.
    pub stage: bool,
    pub plugins: Vec<PluginVersion>,
    /// Stages only: the facts the collection's handlers exported, in export
    /// order.
    pub facts: Vec<EncodedFact>,
}

/// One exported fact as it travels: which plugin's type it is, the version of
/// that plugin, and the tokens `ExportFact::to_tokens` produced.
#[derive(Clone, Debug)]
pub struct EncodedFact {
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub tokens: TokenStream,
    /// Where the `fact` block is (for errors about it); the span of the
    /// `ns` keyword after decoding, the export call site before.
    pub span: Span,
}

/// `ver N ; export ; failed ; stage ; plugin .. ; fact { .. }` — the start of
/// every source block.
pub fn encode_header(desc: &SourceDesc) -> TokenStream {
    let ver = Literal::u32_unsuffixed(CARRIER_FORMAT_VERSION);
    let export = desc.export.then(|| quote!(export ;));
    let failed = desc.failed.then(|| quote!(failed ;));
    let stage = desc.stage.then(|| quote!(stage ;));
    let plugins = desc.plugins.iter().map(|p| {
        let (ns, name, version) = (
            Literal::string(&p.namespace),
            Literal::string(&p.name),
            Literal::string(&p.version),
        );
        quote!(plugin #ns #name #version ;)
    });
    let facts = desc.facts.iter().map(|f| {
        let (ns, name, version) = (Literal::string(&f.namespace), Literal::string(&f.name), Literal::string(&f.version));
        let tokens = escape_dollars(f.tokens.clone());
        quote!(fact { ns #ns type #name ver #version { #tokens } })
    });
    quote!(ver #ver ; #export #failed #stage #(#plugins)* #(#facts)*)
}

/// `rec { .. }` for one record.
pub fn encode_record(record: &Record) -> TokenStream {
    let Kind { ns, kind } = &record.kind;
    let form = match record.form {
        Form::Attribute => quote!(attr),
        Form::Call => quote!(call),
    };
    let seq = Literal::u32_unsuffixed(record.sequence);
    let path = &record.relative_path;
    let cond = record.condition.as_ref().map(|c| {
        let c = escape_dollars(c.clone());
        quote!(cond [ #c ])
    });
    let args = escape_dollars(record.args.clone());
    let item = record.item.as_ref().map(|i| {
        let i = escape_dollars(i.clone());
        quote!(item { #i })
    });
    let ann = record.annotations.iter().map(|(k, v)| {
        let k = Literal::string(k);
        let v = escape_dollars(v.clone());
        quote!(#k { #v })
    });
    let markers = &record.markers;
    let flags = record.tainted.then(|| quote!(tainted));
    quote! {
        rec {
            kind [ #ns :: #kind ]
            form #form
            seq #seq
            path [ #(#path)* ]
            #cond
            args { #args }
            #item
            ann { #(#ann)* }
            markers [ #(#markers)* ]
            flags [ #flags ]
        }
    }
}

/// One decoded source block.
#[derive(Debug)]
pub struct DecodedSource {
    pub info: SourceInfo,
    pub records: Vec<Record>,
    /// Stages only: the exported facts, in export order, with `$` unescaped
    /// and paths as the stage wrote them (not yet rebased).
    pub facts: Vec<EncodedFact>,
    /// The format version the carrier declared.
    pub version: u32,
}

mod kw {
    syn::custom_keyword!(ver);
    syn::custom_keyword!(export);
    syn::custom_keyword!(failed);
    syn::custom_keyword!(plugin);
    syn::custom_keyword!(rec);
    syn::custom_keyword!(kind);
    syn::custom_keyword!(form);
    syn::custom_keyword!(attr);
    syn::custom_keyword!(call);
    syn::custom_keyword!(seq);
    syn::custom_keyword!(path);
    syn::custom_keyword!(cond);
    syn::custom_keyword!(args);
    syn::custom_keyword!(item);
    syn::custom_keyword!(ann);
    syn::custom_keyword!(markers);
    syn::custom_keyword!(flags);
    syn::custom_keyword!(tainted);
    syn::custom_keyword!(stage);
    syn::custom_keyword!(fact);
    syn::custom_keyword!(ns);
}

/// Decode the body of `source [ path ] { body }` for the source at `index`.
///
/// The version is checked first (R10): a mismatch is an error that names the
/// source and both versions, and nothing else is parsed.
pub fn decode_source(index: usize, path: Vec<Ident>, body: TokenStream) -> crate::Result<DecodedSource> {
    let path_text = path_string(&path);
    let parser = move |input: ParseStream| -> syn::Result<DecodedSource> {
        let ver_kw: kw::ver = input.parse().map_err(|_| {
            syn::Error::new(
                input.span(),
                format!(
                    "carrier for source `{path_text}` does not start with a version tag; \
                     it was not produced by a compatible `collect` attribute"
                ),
            )
        })?;
        let ver: LitInt = input.parse()?;
        input.parse::<Token![;]>()?;
        let version: u32 = ver.base10_parse()?;
        if version != CARRIER_FORMAT_VERSION {
            return Err(syn::Error::new(
                ver_kw.span,
                format!(
                    "carrier version mismatch: source `{path_text}` was collected with carrier format \
                     v{version}, but this bundle expects v{CARRIER_FORMAT_VERSION}. \
                     Rebuild that crate against the same plenary version."
                ),
            ));
        }
        let mut info = SourceInfo { index, path: path.clone(), export: false, failed: false, stage: false, plugins: Vec::new() };
        let mut records = Vec::new();
        let mut facts = Vec::new();
        while !input.is_empty() {
            let look = input.lookahead1();
            if look.peek(kw::export) {
                input.parse::<kw::export>()?;
                input.parse::<Token![;]>()?;
                info.export = true;
            } else if look.peek(kw::failed) {
                input.parse::<kw::failed>()?;
                input.parse::<Token![;]>()?;
                info.failed = true;
            } else if look.peek(kw::stage) {
                input.parse::<kw::stage>()?;
                input.parse::<Token![;]>()?;
                info.stage = true;
            } else if look.peek(kw::fact) {
                input.parse::<kw::fact>()?;
                let content;
                braced!(content in input);
                let ns_kw: kw::ns = content.parse()?;
                let ns: LitStr = content.parse()?;
                content.parse::<Token![type]>()?;
                let name: LitStr = content.parse()?;
                content.parse::<kw::ver>()?;
                let version: LitStr = content.parse()?;
                let body;
                braced!(body in content);
                let tokens = unescape_dollars(body.parse::<TokenStream>()?);
                facts.push(EncodedFact { namespace: ns.value(), name: name.value(), version: version.value(), tokens, span: ns_kw.span });
            } else if look.peek(kw::plugin) {
                input.parse::<kw::plugin>()?;
                let ns: LitStr = input.parse()?;
                let name: LitStr = input.parse()?;
                let version: LitStr = input.parse()?;
                input.parse::<Token![;]>()?;
                info.plugins.push(PluginVersion { namespace: ns.value(), name: name.value(), version: version.value() });
            } else if look.peek(kw::rec) {
                input.parse::<kw::rec>()?;
                let content;
                braced!(content in input);
                let mut record = parse_record(&content)?;
                record.source = Some(SourceRef { index, path: path.clone() });
                records.push(record);
            } else {
                return Err(look.error());
            }
        }
        Ok(DecodedSource { info, records, facts, version })
    };
    Ok(parser.parse2(body)?)
}

fn parse_record(input: ParseStream) -> syn::Result<Record> {
    input.parse::<kw::kind>()?;
    let k;
    bracketed!(k in input);
    let ns = k.call(Ident::parse_any)?;
    k.parse::<Token![::]>()?;
    let kind = k.call(Ident::parse_any)?;

    input.parse::<kw::form>()?;
    let form = if input.peek(kw::attr) {
        input.parse::<kw::attr>()?;
        Form::Attribute
    } else {
        input.parse::<kw::call>()?;
        Form::Call
    };

    input.parse::<kw::seq>()?;
    let seq: LitInt = input.parse()?;
    let sequence: u32 = seq.base10_parse()?;

    input.parse::<kw::path>()?;
    let p;
    bracketed!(p in input);
    let mut relative_path = Vec::new();
    while !p.is_empty() {
        relative_path.push(p.call(Ident::parse_any)?);
    }

    let mut condition = None;
    if input.peek(kw::cond) {
        input.parse::<kw::cond>()?;
        let c;
        bracketed!(c in input);
        condition = Some(unescape_dollars(c.parse::<TokenStream>()?));
    }

    input.parse::<kw::args>()?;
    let a;
    braced!(a in input);
    let args = unescape_dollars(a.parse::<TokenStream>()?);

    let mut item = None;
    if input.peek(kw::item) {
        input.parse::<kw::item>()?;
        let i;
        braced!(i in input);
        item = Some(unescape_dollars(i.parse::<TokenStream>()?));
    }

    input.parse::<kw::ann>()?;
    let anns;
    braced!(anns in input);
    let mut record = Record::new(Kind::new(ns, kind), form, args, item, relative_path, sequence, condition);
    while !anns.is_empty() {
        let key: LitStr = anns.parse()?;
        let v;
        braced!(v in anns);
        record.annotations.insert(key.value(), unescape_dollars(v.parse::<TokenStream>()?));
    }

    input.parse::<kw::markers>()?;
    let m;
    bracketed!(m in input);
    while !m.is_empty() {
        record.markers.push(m.call(Ident::parse_any)?);
    }

    input.parse::<kw::flags>()?;
    let f;
    bracketed!(f in input);
    while !f.is_empty() {
        if f.peek(kw::tainted) {
            f.parse::<kw::tainted>()?;
            record.tainted = true;
        } else {
            return Err(f.error("unknown record flag"));
        }
    }
    Ok(record)
}

/// Split the input of `__finish` into the facade path and the source blocks.
///
/// ```text
/// plenary = [ $crate ] bundle = [ demo_macros ] export { source [ path ] { body } ... }
/// ```
///
/// (`export` is present for `expand!(bundle; export; ..)`.)
pub struct FinishInput {
    pub plenary: TokenStream,
    /// Span of the collection site (the bundle path the user wrote in
    /// `expand!`), or the call site when `expand!` did not pass it.
    pub site: Span,
    /// `expand!(bundle; export; ..)`: the stage carrier is `#[macro_export]`.
    pub export: bool,
    pub sources: Vec<(Vec<Ident>, TokenStream, Span)>,
}

pub fn parse_finish_input(input: TokenStream) -> crate::Result<FinishInput> {
    let parser = |input: ParseStream| -> syn::Result<FinishInput> {
        let kw: Ident = input.call(Ident::parse_any)?;
        if kw != "plenary" {
            return Err(syn::Error::new(kw.span(), "expected `plenary = [..]`"));
        }
        input.parse::<Token![=]>()?;
        let m;
        bracketed!(m in input);
        let plenary: TokenStream = m.parse()?;
        let mut site = Span::call_site();
        if input.peek(Ident::peek_any) {
            let kw: Ident = input.call(Ident::parse_any)?;
            if kw != "bundle" {
                return Err(syn::Error::new(kw.span(), "expected `bundle = [..]`"));
            }
            input.parse::<Token![=]>()?;
            let b;
            bracketed!(b in input);
            let first: Ident = b.call(Ident::parse_any)?;
            site = first.span();
            let _rest: TokenStream = b.parse()?;
        }
        let mut export = false;
        if input.peek(kw::export) {
            input.parse::<kw::export>()?;
            export = true;
        }
        let body;
        braced!(body in input);
        let mut sources: Vec<(Vec<Ident>, TokenStream, Span)> = Vec::new();
        while !body.is_empty() {
            let s: Ident = body.call(Ident::parse_any)?;
            if s != "source" {
                return Err(syn::Error::new(s.span(), "expected `source [path] { .. }`"));
            }
            let p;
            bracketed!(p in body);
            let mut path = Vec::new();
            while !p.is_empty() {
                path.push(p.call(Ident::parse_any)?);
                if !p.is_empty() {
                    p.parse::<Token![::]>()?;
                }
            }
            let span = body.span();
            let b;
            braced!(b in body);
            sources.push((path, b.parse::<TokenStream>()?, span));
        }
        // `expand!` ends the list with a comma; a carrier that finished the
        // chain leaves nothing, a v1 carrier (which handed the rest of the list
        // back to its callback instead of chaining) leaves that comma, or the
        // comma and the sources it never called.
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        if !input.is_empty() {
            let culprit = sources.last().map(|(p, _, _)| path_string(p)).unwrap_or_default();
            let span = sources.last().and_then(|(p, _, _)| p.first().map(|i| i.span())).unwrap_or_else(|| input.span());
            let handed_back = input.cursor().token_stream().to_string();
            let handed_back = handed_back.split(',').next().unwrap_or("").replace(' ', "");
            return Err(syn::Error::new(
                span,
                format!(
                    "carrier calling convention mismatch: source `{culprit}` was collected with carrier format v1 \
                     (its carrier handed `{handed_back}` back to the collection instead of calling that source's \
                     carrier), but this bundle expects v{CARRIER_FORMAT_VERSION}. Rebuild that crate against the \
                     same plenary version."
                ),
            ));
        }
        Ok(FinishInput { plenary, site, export, sources })
    };
    Ok(parser.parse2(input)?)
}

/// Helper for tests: the full body of a source (header + every record).
pub fn encode_source_body(desc: &SourceDesc, records: &[Record]) -> TokenStream {
    let header = encode_header(desc);
    let recs = records.iter().map(encode_record);
    quote!(#header #(#recs)*)
}

impl ToTokens for SourceDesc {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.extend(encode_header(self));
    }
}
