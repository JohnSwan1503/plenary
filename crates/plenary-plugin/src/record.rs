//! Records, sources and the `State` fact.

use crate::util::path_string;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, ToTokens};
use std::collections::BTreeMap;
use std::fmt;

/// The two forms a record can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// `#[ns::kind(args)] item`
    Attribute,
    /// `ns::kind!(args)` anywhere in the source, including function bodies.
    Call,
}

impl fmt::Display for Form {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Form::Attribute => "attribute",
            Form::Call => "call",
        })
    }
}

/// A record kind: `ns::kind`. Both idents carry the spans of the user's
/// attribute or macro path.
#[derive(Clone, Debug)]
pub struct Kind {
    pub(crate) ns: Ident,
    pub(crate) kind: Ident,
}

impl Kind {
    pub fn new(ns: Ident, kind: Ident) -> Self {
        Kind { ns, kind }
    }

    /// The namespace, e.g. `stq`.
    pub fn namespace(&self) -> &Ident {
        &self.ns
    }

    /// The kind name, e.g. `table`.
    pub fn name(&self) -> &Ident {
        &self.kind
    }

    /// `true` when this is `ns::kind`.
    pub fn is(&self, ns: &str, kind: &str) -> bool {
        self.ns == ns && self.kind == kind
    }

    /// The span of the kind name in the user's code.
    pub fn span(&self) -> Span {
        self.kind.span()
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.ns, self.kind)
    }
}

/// One unit of introduced state (see SPEC section 2).
///
/// A record never changes after its local handler ran. Every token in it keeps
/// the span it had in the user's file, so diagnostics can point there from a
/// collection in another module or crate.
#[derive(Clone, Debug)]
pub struct Record {
    pub(crate) kind: Kind,
    pub(crate) form: Form,
    pub(crate) args: TokenStream,
    pub(crate) item: Option<TokenStream>,
    pub(crate) relative_path: Vec<Ident>,
    pub(crate) sequence: u32,
    pub(crate) condition: Option<TokenStream>,
    pub(crate) annotations: BTreeMap<String, TokenStream>,
    pub(crate) markers: Vec<Ident>,
    pub(crate) tainted: bool,
    /// Set by `__finish` once the record is placed in a collection.
    pub(crate) source: Option<SourceRef>,
}

/// Where a record came from, once it is part of a collection.
#[derive(Clone, Debug)]
pub(crate) struct SourceRef {
    pub(crate) index: usize,
    pub(crate) path: Vec<Ident>,
}

impl Record {
    pub(crate) fn new(
        kind: Kind,
        form: Form,
        args: TokenStream,
        item: Option<TokenStream>,
        relative_path: Vec<Ident>,
        sequence: u32,
        condition: Option<TokenStream>,
    ) -> Self {
        Record {
            kind,
            form,
            args,
            item,
            relative_path,
            sequence,
            condition,
            annotations: BTreeMap::new(),
            markers: Vec::new(),
            tainted: false,
            source: None,
        }
    }

    /// `ns::kind`.
    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    /// Attribute or call form.
    pub fn form(&self) -> Form {
        self.form
    }

    /// The attribute's arguments (the tokens inside `#[ns::kind(...)]`) or
    /// the call's arguments (the tokens inside `ns::kind!(...)`).
    pub fn args(&self) -> &TokenStream {
        &self.args
    }

    /// Parse the arguments as a syn type, e.g. `LitStr` or a custom `Parse`.
    pub fn parse_args<T: syn::parse::Parse>(&self) -> crate::Result<T> {
        Ok(syn::parse2(self.args.clone())?)
    }

    /// Parse the arguments with a custom parser (`syn::meta::parser`, ...).
    pub fn parse_args_with<F: syn::parse::Parser>(&self, parser: F) -> crate::Result<F::Output> {
        Ok(parser.parse2(self.args.clone())?)
    }

    /// Attribute form only: the full item, with every plugin attribute still
    /// on it (including those on fields, variants and parameters).
    pub fn item(&self) -> Option<&TokenStream> {
        self.item.as_ref()
    }

    /// Parse the item as a syn type, e.g. `syn::ItemStruct`.
    pub fn parse_item<T: syn::parse::Parse>(&self) -> crate::Result<T> {
        match &self.item {
            Some(item) => Ok(syn::parse2(item.clone())?),
            None => Err(crate::Error::new(self.kind.span(), "call-form records have no item")),
        }
    }

    /// The record's position inside its source: `Users`, `inner::Users`. For
    /// a call-form record this is the path of the enclosing item.
    pub fn relative_path(&self) -> &[Ident] {
        &self.relative_path
    }

    /// The last segment of the relative path (the item's own name), if any.
    pub fn ident(&self) -> Option<&Ident> {
        self.relative_path.last()
    }

    /// Document-order position inside the source.
    pub fn sequence(&self) -> u32 {
        self.sequence
    }

    /// The combined `#[cfg]` predicate of the record and its enclosing items,
    /// or `None` when unconditional. Informational: the compiler, not the
    /// framework, decides whether a conditional record is in the state (R7).
    pub fn condition(&self) -> Option<&TokenStream> {
        self.condition.as_ref()
    }

    /// An annotation attached by the local handler.
    pub fn annotation(&self, key: &str) -> Option<&TokenStream> {
        self.annotations.get(key)
    }

    /// An annotation parsed as a syn type.
    pub fn annotation_as<T: syn::parse::Parse>(&self, key: &str) -> Option<crate::Result<T>> {
        self.annotations.get(key).map(|t| Ok(syn::parse2(t.clone())?))
    }

    /// All annotations, sorted by key.
    pub fn annotations(&self) -> &BTreeMap<String, TokenStream> {
        &self.annotations
    }

    /// The markers the local handler created for this record, in creation
    /// order. Each lives at the source root.
    pub fn markers(&self) -> &[Ident] {
        &self.markers
    }

    /// `true` if the local handler reported an error. The record still
    /// travels so global handlers can skip it instead of piling on errors.
    pub fn is_tainted(&self) -> bool {
        self.tainted
    }

    /// Index of the record's source in the collection (list order).
    /// Only meaningful inside a global handler.
    pub fn source_index(&self) -> usize {
        self.source.as_ref().map(|s| s.index).unwrap_or(0)
    }

    /// The source's path as listed in `expand!`. Only meaningful inside a
    /// global handler.
    pub fn source_path(&self) -> &[Ident] {
        self.source.as_ref().map(|s| s.path.as_slice()).unwrap_or(&[])
    }

    /// The absolute path of the item (R3): the source's path as listed in
    /// `expand!` joined with the record's relative path. For a call-form
    /// record this is the path of the enclosing item.
    pub fn absolute_path(&self) -> syn::Path {
        let segs = self.source_path().iter().chain(self.relative_path.iter());
        syn::parse2(quote!(#(#segs)::*)).expect("a path")
    }

    /// The absolute path of one of this record's markers.
    pub fn marker_path(&self, marker: &Ident) -> syn::Path {
        let segs = self.source_path().iter();
        syn::parse2(quote!(#(#segs)::* :: #marker)).expect("a path")
    }

    /// Absolute paths of all markers, in creation order.
    pub fn marker_paths(&self) -> Vec<syn::Path> {
        self.markers.iter().map(|m| self.marker_path(m)).collect()
    }

    /// A readable location for diagnostics: `crate::users::users::Users`
    /// (attribute form) or `crate::users::users::body#3` (call form).
    pub fn display_path(&self) -> String {
        let mut s = path_string(self.source_path());
        if !self.relative_path.is_empty() {
            if !s.is_empty() {
                s.push_str("::");
            }
            s.push_str(&path_string(&self.relative_path));
        }
        if self.form == Form::Call {
            s.push_str(&format!("#{}", self.sequence));
        }
        s
    }
}

/// What a collection knows about one listed source.
#[derive(Clone, Debug)]
pub struct SourceInfo {
    pub(crate) index: usize,
    pub(crate) path: Vec<Ident>,
    pub(crate) export: bool,
    pub(crate) failed: bool,
    pub(crate) stage: bool,
    pub(crate) plugins: Vec<PluginVersion>,
}

/// The plugin that ran a source's local handlers for one namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginVersion {
    pub namespace: String,
    pub name: String,
    pub version: String,
}

impl SourceInfo {
    /// Position in the `expand!` list.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The path as listed in `expand!`.
    pub fn path(&self) -> &[Ident] {
        &self.path
    }

    /// The path as a syn path.
    pub fn syn_path(&self) -> syn::Path {
        let segs = self.path.iter();
        syn::parse2(quote!(#(#segs)::*)).expect("a path")
    }

    /// The source module's own name (last path segment).
    pub fn module_name(&self) -> &Ident {
        self.path.last().expect("non-empty path")
    }

    /// `true` when the listed path starts with `crate`, `self` or `super`:
    /// the source lives in the crate doing the collecting. Only local sources
    /// may have traits implemented for their markers (R4).
    pub fn is_local(&self) -> bool {
        is_local_path(&self.path)
    }

    /// The crate the source belongs to, as named at the collection site:
    /// `crate` for local sources, otherwise the first path segment.
    pub fn crate_name(&self) -> String {
        if self.is_local() {
            "crate".to_string()
        } else {
            self.path.first().map(|s| s.to_string()).unwrap_or_default()
        }
    }

    /// Whether the source was marked `collect(export)`.
    pub fn is_export(&self) -> bool {
        self.export
    }

    /// Whether the source's collector failed (R11). A failed source has no
    /// records.
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    /// Whether this entry is a stage: another collection's exported results
    /// (SPEC section 10). Its records are the ones found in the items that
    /// collection emitted; its facts arrive through `Imported<F>`.
    pub fn is_stage(&self) -> bool {
        self.stage
    }

    /// The plugins (one per namespace) that ran the source's local handlers.
    pub fn plugins(&self) -> &[PluginVersion] {
        &self.plugins
    }

    /// Spelled as listed: `crate::users::users`.
    pub fn display_path(&self) -> String {
        path_string(&self.path)
    }
}

pub(crate) fn is_local_path(path: &[Ident]) -> bool {
    matches!(path.first().map(|s| s.to_string()).as_deref(), Some("crate" | "self" | "super"))
}

/// What global handlers receive: every record of the collection in canonical
/// order (R5), plus one [`SourceInfo`] per listed source.
///
/// `State` is a built-in fact; a global handler reads it by taking a `&State`
/// parameter.
#[derive(Clone, Debug)]
pub struct State {
    pub(crate) records: Vec<Record>,
    pub(crate) sources: Vec<SourceInfo>,
    pub(crate) collection: Ident,
}

impl State {
    /// Build a state from sources, for tests and for `__finish`. Records are
    /// sorted into canonical order (source list position, then sequence).
    pub fn new(sources: Vec<SourceInfo>, mut records: Vec<Record>) -> Self {
        records.sort_by_key(|r| (r.source_index(), r.sequence));
        State { records, sources, collection: format_ident!("__Collection") }
    }

    /// All records, in canonical order.
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Records of one kind, in canonical order.
    pub fn records_of<'a>(&'a self, ns: &'a str, kind: &'a str) -> impl Iterator<Item = &'a Record> + 'a {
        self.records.iter().filter(move |r| r.kind.is(ns, kind))
    }

    /// Records of one namespace, in canonical order.
    pub fn records_in<'a>(&'a self, ns: &'a str) -> impl Iterator<Item = &'a Record> + 'a {
        self.records.iter().filter(move |r| r.kind.ns == ns)
    }

    /// The listed sources, in list order.
    pub fn sources(&self) -> &[SourceInfo] {
        &self.sources
    }

    /// The source a record came from.
    pub fn source_of(&self, record: &Record) -> &SourceInfo {
        &self.sources[record.source_index()]
    }

    /// The collection's marker type, `__Collection`, as an identifier.
    pub fn collection(&self) -> &Ident {
        &self.collection
    }
}

impl crate::Fact for State {}

impl ToTokens for Kind {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let (ns, kind) = (&self.ns, &self.kind);
        tokens.extend(quote!(#ns :: #kind));
    }
}
