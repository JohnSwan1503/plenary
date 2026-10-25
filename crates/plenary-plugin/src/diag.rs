//! Diagnostics: errors and warnings that point at the user's tokens.
//!
//! Errors become `compile_error!` invocations spanned at the user's token.
//! Warnings use the deprecated-item technique (stable Rust has no custom
//! warnings): a `#[deprecated]` constant is defined and used at the user's
//! span, so the compiler reports `use of deprecated constant ...: <message>`
//! there. `#[allow(deprecated)]` hides them and `-D warnings` denies them.

use proc_macro2::{Span, TokenStream};
use quote::quote_spanned;
use std::fmt;

/// Severity of a [`Diagnostic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Reported as a compile error.
    Error,
    /// Reported as a `deprecated` warning at the user's code.
    Warning,
}

/// One message attached to a span.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    level: Level,
    span: Span,
    message: String,
    notes: Vec<(Option<Span>, String)>,
}

impl Diagnostic {
    /// An error at `span`.
    pub fn error(span: Span, message: impl fmt::Display) -> Self {
        Diagnostic { level: Level::Error, span, message: message.to_string(), notes: Vec::new() }
    }

    /// A warning at `span`.
    pub fn warning(span: Span, message: impl fmt::Display) -> Self {
        Diagnostic { level: Level::Warning, span, message: message.to_string(), notes: Vec::new() }
    }

    /// Attach a note. When the note has a span and the location of that span
    /// is known (inside a real proc-macro expansion), the rendered note names
    /// the file, line and column.
    pub fn note(mut self, span: Option<Span>, message: impl fmt::Display) -> Self {
        self.notes.push((span, message.to_string()));
        self
    }

    /// The severity.
    pub fn level(&self) -> Level {
        self.level
    }

    /// The span the diagnostic points at.
    pub fn span(&self) -> Span {
        self.span
    }

    /// The main message, without notes.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The attached notes.
    pub fn notes(&self) -> &[(Option<Span>, String)] {
        &self.notes
    }

    /// The message followed by its notes, as the user will read it.
    pub fn rendered_message(&self) -> String {
        let mut s = self.message.clone();
        for (span, note) in &self.notes {
            s.push_str("\n  = note: ");
            s.push_str(note);
            if let Some(loc) = span.and_then(span_location) {
                s.push_str(" (at ");
                s.push_str(&loc);
                s.push(')');
            }
        }
        s
    }

    /// The tokens that make the compiler report this diagnostic.
    pub fn to_tokens(&self) -> TokenStream {
        let span = self.span;
        let msg = self.rendered_message();
        match self.level {
            Level::Error => quote_spanned! {span=> ::core::compile_error! { #msg } },
            Level::Warning => quote_spanned! {span=>
                const _: () = {
                    #[deprecated(note = #msg)]
                    #[allow(non_upper_case_globals)]
                    const warning: () = ();
                    let _ = warning;
                };
            },
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        write!(f, "{level}: {}", self.rendered_message())
    }
}

/// `file:line:column` of a span, when the real compiler is behind it.
///
/// Returns `None` outside a proc-macro expansion (unit tests) so plugin logic
/// stays testable without a compiler.
pub fn span_location(span: Span) -> Option<String> {
    if !proc_macro::is_available() {
        return None;
    }
    let s = span.unwrap();
    let file = s.file();
    if file.is_empty() {
        return None;
    }
    Some(format!("{}:{}:{}", file, s.line(), s.column()))
}

/// Where a span is, as text that goes into hidden generated names.
///
/// Inside the compiler this is `file:line` from the stable
/// `proc_macro::Span::file`/`line` (the file as rustc displays it in
/// diagnostics; stable Rust 1.88+). Outside the compiler (the engine's own
/// tests, `testing::collect`) the compiler's span data does not exist, so
/// this falls back to the line and column proc-macro2's own source map gives
/// a token parsed from a string (`"...".parse::<TokenStream>()`); tokens made
/// with `quote!` all sit at `1:0`. Either way the result depends only on the
/// input, never on time, environment or iteration order (R8).
///
/// (The vendored proc-macro2 1.0.95 exposes `Span::file()` only under the
/// semver-exempt cfg, and its `Span::start()` returns `0:0` for compiler
/// spans on stable, so the two cases need two code paths.)
pub fn span_key(span: Span) -> String {
    if proc_macro::is_available() {
        let s = span.unwrap();
        return format!("{}:{}", s.file(), s.line());
    }
    let start = span.start();
    format!("<parsed>:{}:{}", start.line, start.column)
}

/// The error type of handlers. It carries any number of diagnostics so that
/// everything wrong with one expansion is reported together (R11).
#[derive(Clone, Debug)]
pub struct Error {
    diagnostics: Vec<Diagnostic>,
}

/// `Result` with [`Error`] as the error type.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// An error at `span`.
    pub fn new(span: Span, message: impl fmt::Display) -> Self {
        Error { diagnostics: vec![Diagnostic::error(span, message)] }
    }

    /// An error with no better span than the macro invocation.
    pub fn call_site(message: impl fmt::Display) -> Self {
        Self::new(Span::call_site(), message)
    }

    /// Attach a note to the last diagnostic.
    pub fn note(mut self, span: Option<Span>, message: impl fmt::Display) -> Self {
        if let Some(last) = self.diagnostics.pop() {
            self.diagnostics.push(last.note(span, message));
        }
        self
    }

    /// Add another error's diagnostics to this one.
    pub fn combine(&mut self, other: Error) {
        self.diagnostics.extend(other.diagnostics);
    }

    /// Add a diagnostic.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// The diagnostics, in the order they were added.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Take the diagnostics out.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// Tokens for every diagnostic, concatenated.
    pub fn to_compile_errors(&self) -> TokenStream {
        self.diagnostics.iter().map(Diagnostic::to_tokens).collect()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, d) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{}", d.rendered_message())?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

impl From<syn::Error> for Error {
    fn from(e: syn::Error) -> Self {
        Error { diagnostics: e.into_iter().map(|e| Diagnostic::error(e.span(), e)).collect() }
    }
}

impl From<Diagnostic> for Error {
    fn from(d: Diagnostic) -> Self {
        Error { diagnostics: vec![d] }
    }
}

/// Accumulates diagnostics from many places and renders them together.
#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn error(&mut self, span: Span, message: impl fmt::Display) {
        self.items.push(Diagnostic::error(span, message));
    }

    pub fn warning(&mut self, span: Span, message: impl fmt::Display) {
        self.items.push(Diagnostic::warning(span, message));
    }

    pub fn extend_from_error(&mut self, e: Error) {
        self.items.extend(e.into_diagnostics());
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.level == Level::Error)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }

    pub fn to_tokens(&self) -> TokenStream {
        self.items.iter().map(Diagnostic::to_tokens).collect()
    }
}
