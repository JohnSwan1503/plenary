//! The context a local handler gets.

use crate::diag::{Diagnostic, Diagnostics};
use crate::record::Form;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, ToTokens};
use std::collections::BTreeMap;
use std::fmt;

/// What a local handler knows about its source: nothing about other records
/// (R2), only the module itself.
#[derive(Clone, Debug)]
pub struct LocalSourceInfo {
    pub(crate) module_name: Ident,
    pub(crate) export: bool,
}

impl LocalSourceInfo {
    /// The source module's name.
    pub fn module_name(&self) -> &Ident {
        &self.module_name
    }

    /// Whether the source is `collect(export)`.
    pub fn is_export(&self) -> bool {
        self.export
    }
}

/// A marker type created by [`LocalCx::marker`].
#[derive(Clone, Debug)]
pub struct Marker {
    name: Ident,
    path: TokenStream,
}

impl Marker {
    /// The marker's name at the source root (`__pl_m3`).
    pub fn name(&self) -> &Ident {
        &self.name
    }

    /// A path to the marker that is valid where the record is: `self::__pl_m3`
    /// at the source root, `super::__pl_m3` one module down, and so on.
    pub fn path(&self) -> &TokenStream {
        &self.path
    }
}

impl ToTokens for Marker {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.extend(self.path.clone());
    }
}

/// Everything a local handler may do, besides reading its record.
pub struct LocalCx<'a> {
    pub(crate) source: &'a LocalSourceInfo,
    pub(crate) form: Form,
    pub(crate) depth: usize,
    pub(crate) next_marker: &'a mut usize,
    pub(crate) replacement_item: Option<TokenStream>,
    pub(crate) added_items: Vec<TokenStream>,
    pub(crate) call_replacement: Option<TokenStream>,
    pub(crate) keep_spans: bool,
    pub(crate) markers: Vec<Ident>,
    pub(crate) annotations: BTreeMap<String, TokenStream>,
    pub(crate) diagnostics: Diagnostics,
}

impl<'a> LocalCx<'a> {
    pub(crate) fn new(source: &'a LocalSourceInfo, form: Form, depth: usize, next_marker: &'a mut usize) -> Self {
        LocalCx {
            source,
            form,
            depth,
            next_marker,
            replacement_item: None,
            added_items: Vec::new(),
            call_replacement: None,
            keep_spans: false,
            markers: Vec::new(),
            annotations: BTreeMap::new(),
            diagnostics: Diagnostics::new(),
        }
    }

    /// The source this record is in.
    pub fn source(&self) -> &LocalSourceInfo {
        self.source
    }

    /// How many inline modules deep the record is below the source root.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Attribute form only: emit these tokens instead of the item. By
    /// default the item stays, with plugin attributes removed.
    pub fn replace_item(&mut self, items: impl ToTokens) {
        self.replacement_item = Some(items.to_token_stream());
    }

    /// Emit these items right after the record's item (or, for a call-form
    /// record, after the module-level item that contains the call). Tokens
    /// here resolve in the source's module, which is how a local handler
    /// creates named handles for a collection to use (R3).
    pub fn add_item(&mut self, items: impl ToTokens) {
        self.added_items.push(items.to_token_stream());
    }

    /// Call form only: the tokens the `ns::kind!(...)` call expands to. They
    /// are given the span of the call, so compiler errors about them point at
    /// the user's call; use [`set_replacement_spanned`](Self::set_replacement_spanned)
    /// to keep the spans you set yourself.
    pub fn set_replacement(&mut self, tokens: impl ToTokens) {
        self.call_replacement = Some(tokens.to_token_stream());
        self.keep_spans = false;
    }

    /// Like [`set_replacement`](Self::set_replacement), keeping the tokens'
    /// own spans.
    pub fn set_replacement_spanned(&mut self, tokens: impl ToTokens) {
        self.call_replacement = Some(tokens.to_token_stream());
        self.keep_spans = true;
    }

    /// Create a marker type at the source root (R4) and return a path to it
    /// that is valid at the record. The marker's name is recorded on the
    /// record, so global handlers can reach it through
    /// [`Record::marker_paths`](crate::Record::marker_paths).
    pub fn marker(&mut self) -> Marker {
        let name = format_ident!("__pl_m{}", *self.next_marker);
        *self.next_marker += 1;
        self.markers.push(name.clone());
        let path = if self.depth == 0 {
            quote!(self::#name)
        } else {
            let supers = (0..self.depth).map(|_| quote!(super));
            quote!(#(#supers)::* :: #name)
        };
        Marker { name, path }
    }

    /// Attach data to the record. It travels with the record to every
    /// collection; global handlers read it with
    /// [`Record::annotation`](crate::Record::annotation).
    pub fn annotate(&mut self, key: impl Into<String>, value: impl ToTokens) {
        self.annotations.insert(key.into(), value.to_token_stream());
    }

    /// Report an error at `span`. The record is marked tainted; the handler
    /// may keep going and report more.
    pub fn error(&mut self, span: Span, message: impl fmt::Display) {
        self.diagnostics.error(span, message);
    }

    /// Report a warning at `span`.
    pub fn warn(&mut self, span: Span, message: impl fmt::Display) {
        self.diagnostics.warning(span, message);
    }

    /// Report a prepared diagnostic.
    pub fn report(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// The form of the record being handled.
    pub fn form(&self) -> Form {
        self.form
    }
}
