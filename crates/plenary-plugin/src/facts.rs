//! Encoding facts as tokens, for stages (SPEC section 10).
//!
//! A stage carrier holds exported facts as tokens, so a fact type that wants
//! to travel implements [`ExportFact`]: a stable name, `to_tokens` and
//! `from_tokens`. This module has the codecs for the common cases so those
//! two methods are a few lines: strings, integers, booleans, lists, options,
//! paths, identifiers, spans, tuples and nested structs ([`Fields`]).
//!
//! The format is plain tokens, readable in a macro expansion:
//!
//! ```text
//! { tables : [ { name : "users" , path : crate :: users :: Users , used : true , } , ] , errors : 0 , }
//! ```
//!
//! Spans survive: a string encoded from a `LitStr` keeps the literal's span,
//! and a `Span` is encoded as a `_` token carrying it, so a diagnostic about
//! an imported fact can still point at the upstream user's code.
//!
//! Paths need no care from the plugin: when a collection imports a fact, the
//! engine rebases every path that starts with `crate`, `self` or `super` from
//! the exporting collection's site to the importing one (see
//! [`rebase_paths`]), because a path in a fact meant something where the
//! stage was written (R3).

use proc_macro2::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};
use quote::{quote, ToTokens};
use std::collections::BTreeMap;
use syn::parse::{Parse, ParseStream, Parser};
use syn::{braced, bracketed, parenthesized, LitBool, LitInt, LitStr, Token};

/// A fact that a stage can export and a later collection can import.
///
/// ```ignore
/// impl plenary_plugin::ExportFact for Index {
///     const NAME: &'static str = "Index";
///     fn to_tokens(&self) -> TokenStream {
///         facts::Fields::new().with("handlers", &self.handlers).encode()
///     }
///     fn from_tokens(tokens: TokenStream) -> Result<Self> {
///         let f = facts::Fields::decode(tokens)?;
///         Ok(Index { handlers: f.get("handlers")? })
///     }
/// }
/// ```
///
/// Register it on the plugin that defines it (`.exportable::<Index>()`), then
/// a global handler exports a value with `cx.export(&index)` and a handler in
/// a collection that lists the stage reads `&Imported<Index>`.
pub trait ExportFact: crate::Fact + Sized {
    /// The type's name in carriers (`fact { ns ".." type "<NAME>" .. }`).
    /// Keep it stable: it is how a later collection recognises the fact.
    const NAME: &'static str;

    /// Encode the fact. Prefer the codecs in [`facts`](crate::facts).
    fn to_tokens(&self) -> TokenStream;

    /// Decode a fact encoded by `to_tokens` (paths already rebased to the
    /// importing collection's site).
    fn from_tokens(tokens: TokenStream) -> crate::Result<Self>;
}

/// Values the codecs can write.
pub trait Encode {
    fn encode(&self, out: &mut TokenStream);
}

/// Values the codecs can read.
pub trait Decode: Sized {
    fn decode(input: ParseStream) -> syn::Result<Self>;
}

/// Encode one value as a token stream.
pub fn encode<T: Encode + ?Sized>(value: &T) -> TokenStream {
    let mut out = TokenStream::new();
    value.encode(&mut out);
    out
}

/// Decode one value from a token stream (the whole stream must be consumed).
pub fn decode<T: Decode>(tokens: TokenStream) -> crate::Result<T> {
    Ok((|input: ParseStream| T::decode(input)).parse2(tokens)?)
}

// ───────────────────────────── scalars ─────────────────────────────

impl Encode for str {
    fn encode(&self, out: &mut TokenStream) {
        out.extend(std::iter::once(TokenTree::Literal(Literal::string(self))));
    }
}

impl Encode for String {
    fn encode(&self, out: &mut TokenStream) {
        self.as_str().encode(out)
    }
}

impl Decode for String {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        Ok(input.parse::<LitStr>()?.value())
    }
}

/// A string literal keeps its span, so `lit.span()` after decoding points at
/// the upstream user's token.
impl Encode for LitStr {
    fn encode(&self, out: &mut TokenStream) {
        self.to_tokens(out)
    }
}

impl Decode for LitStr {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        input.parse()
    }
}

impl Encode for bool {
    fn encode(&self, out: &mut TokenStream) {
        LitBool::new(*self, Span::call_site()).to_tokens(out)
    }
}

impl Decode for bool {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        Ok(input.parse::<LitBool>()?.value)
    }
}

macro_rules! int_codec {
    ($($t:ty => $ctor:ident),* $(,)?) => {$(
        impl Encode for $t {
            fn encode(&self, out: &mut TokenStream) {
                #[allow(unused_comparisons)]
                if *self < 0 {
                    out.extend(std::iter::once(TokenTree::Punct(Punct::new('-', Spacing::Alone))));
                    let mag = (*self as i128).unsigned_abs();
                    out.extend(std::iter::once(TokenTree::Literal(Literal::u128_unsuffixed(mag))));
                } else {
                    out.extend(std::iter::once(TokenTree::Literal(Literal::$ctor(*self))));
                }
            }
        }
        impl Decode for $t {
            fn decode(input: ParseStream) -> syn::Result<Self> {
                let negative = input.peek(Token![-]);
                if negative {
                    input.parse::<Token![-]>()?;
                }
                let lit: LitInt = input.parse()?;
                let text = if negative { format!("-{}", lit.base10_digits()) } else { lit.base10_digits().to_string() };
                text.parse::<$t>().map_err(|_| syn::Error::new(lit.span(), format!("integer out of range for `{}`", stringify!($t))))
            }
        }
    )*};
}

int_codec! {
    u8 => u8_unsuffixed, u16 => u16_unsuffixed, u32 => u32_unsuffixed, u64 => u64_unsuffixed, usize => usize_unsuffixed,
    i8 => i8_unsuffixed, i16 => i16_unsuffixed, i32 => i32_unsuffixed, i64 => i64_unsuffixed, isize => isize_unsuffixed,
}

impl Encode for Ident {
    fn encode(&self, out: &mut TokenStream) {
        self.to_tokens(out)
    }
}

impl Decode for Ident {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        input.call(syn::ext::IdentExt::parse_any)
    }
}

/// A path is written as is (`crate :: users :: Users`). On import the engine
/// rebases its leading `crate`/`self`/`super` to the importing site.
impl Encode for syn::Path {
    fn encode(&self, out: &mut TokenStream) {
        self.to_tokens(out)
    }
}

impl Decode for syn::Path {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        input.call(syn::Path::parse_mod_style)
    }
}

/// A span travels as a `_` token carrying it.
impl Encode for Span {
    fn encode(&self, out: &mut TokenStream) {
        out.extend(std::iter::once(TokenTree::Ident(Ident::new("_", *self))));
    }
}

impl Decode for Span {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        Ok(input.parse::<Token![_]>()?.span)
    }
}

/// Arbitrary tokens, in braces.
impl Encode for TokenStream {
    fn encode(&self, out: &mut TokenStream) {
        out.extend(std::iter::once(TokenTree::Group(Group::new(Delimiter::Brace, self.clone()))));
    }
}

impl Decode for TokenStream {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let content;
        braced!(content in input);
        content.parse()
    }
}

// ───────────────────────────── compounds ─────────────────────────────

/// `[ a , b , ]`
impl<T: Encode> Encode for Vec<T> {
    fn encode(&self, out: &mut TokenStream) {
        self.as_slice().encode(out)
    }
}

impl<T: Encode> Encode for [T] {
    fn encode(&self, out: &mut TokenStream) {
        let mut inner = TokenStream::new();
        for item in self {
            item.encode(&mut inner);
            inner.extend(std::iter::once(TokenTree::Punct(Punct::new(',', Spacing::Alone))));
        }
        out.extend(std::iter::once(TokenTree::Group(Group::new(Delimiter::Bracket, inner))));
    }
}

impl<T: Decode> Decode for Vec<T> {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let content;
        bracketed!(content in input);
        let mut out = Vec::new();
        while !content.is_empty() {
            out.push(T::decode(&content)?);
            if !content.is_empty() {
                content.parse::<Token![,]>()?;
            }
        }
        Ok(out)
    }
}

/// `None` or `Some ( v )`
impl<T: Encode> Encode for Option<T> {
    fn encode(&self, out: &mut TokenStream) {
        match self {
            None => out.extend(quote!(None)),
            Some(v) => {
                out.extend(quote!(Some));
                let mut inner = TokenStream::new();
                v.encode(&mut inner);
                out.extend(std::iter::once(TokenTree::Group(Group::new(Delimiter::Parenthesis, inner))));
            }
        }
    }
}

impl<T: Decode> Decode for Option<T> {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let tag: Ident = input.parse()?;
        if tag == "None" {
            Ok(None)
        } else if tag == "Some" {
            let content;
            parenthesized!(content in input);
            Ok(Some(T::decode(&content)?))
        } else {
            Err(syn::Error::new(tag.span(), "expected `None` or `Some(..)`"))
        }
    }
}

/// `( a , b )`
impl<A: Encode, B: Encode> Encode for (A, B) {
    fn encode(&self, out: &mut TokenStream) {
        let mut inner = TokenStream::new();
        self.0.encode(&mut inner);
        inner.extend(std::iter::once(TokenTree::Punct(Punct::new(',', Spacing::Alone))));
        self.1.encode(&mut inner);
        out.extend(std::iter::once(TokenTree::Group(Group::new(Delimiter::Parenthesis, inner))));
    }
}

impl<A: Decode, B: Decode> Decode for (A, B) {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        let content;
        parenthesized!(content in input);
        let a = A::decode(&content)?;
        content.parse::<Token![,]>()?;
        let b = B::decode(&content)?;
        Ok((a, b))
    }
}

impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self, out: &mut TokenStream) {
        (**self).encode(out)
    }
}

impl<T: Encode + ?Sized> Encode for Box<T> {
    fn encode(&self, out: &mut TokenStream) {
        (**self).encode(out)
    }
}

impl<T: Decode> Decode for Box<T> {
    fn decode(input: ParseStream) -> syn::Result<Self> {
        T::decode(input).map(Box::new)
    }
}

// ───────────────────────────── structs ─────────────────────────────

/// Named fields: `{ name : value , name : value , }`.
///
/// Encode with [`Fields::new`] and [`Fields::with`]; decode with
/// [`Fields::decode`] and [`Fields::get`]. Unknown fields are ignored on
/// decode and a missing field is an error naming it, so adding a field is a
/// compatible change within a plugin's major version (R10).
///
/// ```ignore
/// impl facts::Encode for Table {
///     fn encode(&self, out: &mut TokenStream) {
///         facts::Fields::new().with("name", &self.name).with("path", &self.path).encode_into(out)
///     }
/// }
/// impl facts::Decode for Table {
///     fn decode(input: ParseStream) -> syn::Result<Self> {
///         let f = facts::Fields::parse(input)?;
///         Ok(Table { name: f.get("name")?, path: f.get("path")? })
///     }
/// }
/// ```
pub struct Fields {
    encoded: Vec<(String, TokenStream)>,
    decoded: BTreeMap<String, TokenStream>,
    span: Span,
}

impl Fields {
    /// Start encoding a struct.
    pub fn new() -> Self {
        Fields { encoded: Vec::new(), decoded: BTreeMap::new(), span: Span::call_site() }
    }

    /// Add a field.
    pub fn with<T: Encode + ?Sized>(mut self, name: &str, value: &T) -> Self {
        self.encoded.push((name.to_string(), encode(value)));
        self
    }

    /// The `{ .. }` group holding every field added so far.
    pub fn encode(&self) -> TokenStream {
        let mut out = TokenStream::new();
        self.encode_into(&mut out);
        out
    }

    /// Append the `{ .. }` group to `out`.
    pub fn encode_into(&self, out: &mut TokenStream) {
        let mut inner = TokenStream::new();
        for (name, value) in &self.encoded {
            let name = Ident::new(name, Span::call_site());
            inner.extend(quote!(#name :));
            inner.extend(value.clone());
            inner.extend(quote!(,));
        }
        out.extend(std::iter::once(TokenTree::Group(Group::new(Delimiter::Brace, inner))));
    }

    /// Parse a `{ .. }` group from a parse stream (for nested structs).
    pub fn parse(input: ParseStream) -> syn::Result<Self> {
        let content;
        let brace = braced!(content in input);
        let mut decoded = BTreeMap::new();
        while !content.is_empty() {
            let name: Ident = content.call(syn::ext::IdentExt::parse_any)?;
            content.parse::<Token![:]>()?;
            // A field's value runs up to the next top-level comma.
            let mut value = TokenStream::new();
            while !content.is_empty() && !content.peek(Token![,]) {
                let tt: TokenTree = content.parse()?;
                value.extend(std::iter::once(tt));
            }
            if !content.is_empty() {
                content.parse::<Token![,]>()?;
            }
            decoded.insert(name.to_string(), value);
        }
        Ok(Fields { encoded: Vec::new(), decoded, span: brace.span.join() })
    }

    /// Parse a whole token stream as one `{ .. }` group (for the top level
    /// of an `ExportFact::from_tokens`).
    pub fn decode(tokens: TokenStream) -> crate::Result<Self> {
        Ok((|input: ParseStream| Fields::parse(input)).parse2(tokens)?)
    }

    /// Read a field. A missing field is an error naming it.
    pub fn get<T: Decode>(&self, name: &str) -> syn::Result<T> {
        match self.decoded.get(name) {
            Some(tokens) => (|input: ParseStream| T::decode(input)).parse2(tokens.clone()),
            None => Err(syn::Error::new(self.span, format!("exported fact has no field `{name}`"))),
        }
    }

    /// Read a field that may be absent (for fields added after a stage was
    /// built with an older minor version of the plugin).
    pub fn get_or<T: Decode>(&self, name: &str, default: T) -> syn::Result<T> {
        match self.decoded.get(name) {
            Some(tokens) => (|input: ParseStream| T::decode(input)).parse2(tokens.clone()),
            None => Ok(default),
        }
    }

    /// `true` when the field is present.
    pub fn has(&self, name: &str) -> bool {
        self.decoded.contains_key(name)
    }
}

impl Default for Fields {
    fn default() -> Self {
        Self::new()
    }
}

impl Parse for Fields {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Fields::parse(input)
    }
}

// ───────────────────────────── rebasing ─────────────────────────────

/// Rewrite every path in `tokens` that starts with `crate`, `self` or
/// `super` so that it means the same thing at a collection that lists the
/// stage as `stage` (the path as written in that `expand!`):
///
/// - `crate::x` stays when `stage` is local (`crate::..`, `self::..`,
///   `super::..`), otherwise its `crate` becomes the stage's crate name (the
///   first segment of `stage`);
/// - `self::x` (relative to the stage's module) becomes `<stage>::x`;
/// - `super::super::x` drops as many trailing segments from `stage` as there
///   are `super`s, when `stage` has them; otherwise it is left alone.
///
/// Only a leading keyword counts: an ident preceded by `::` is a later
/// segment, and one preceded by the `$` escape (`__pl_dollar`) is `$crate`.
/// Groups are rewritten recursively.
pub fn rebase_paths(tokens: TokenStream, stage: &[Ident]) -> TokenStream {
    let tts: Vec<TokenTree> = tokens.into_iter().collect();
    let mut out = TokenStream::new();
    let mut i = 0;
    let is_path_sep = |tt: Option<&TokenTree>| matches!(tt, Some(TokenTree::Punct(p)) if p.as_char() == ':');
    while i < tts.len() {
        match &tts[i] {
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), rebase_paths(g.stream(), stage));
                ng.set_span(g.span());
                out.extend(std::iter::once(TokenTree::Group(ng)));
                i += 1;
            }
            TokenTree::Ident(id)
                if (id == "crate" || id == "self" || id == "super")
                    // Preceded by `::`: a later segment, not a path start. (A
                    // single `:` before it is a field separator, `name : crate :: x`.)
                    && !(i >= 2 && is_path_sep(tts.get(i - 1)) && is_path_sep(tts.get(i - 2))) =>
            {
                let escaped = matches!(i.checked_sub(1).and_then(|j| tts.get(j)), Some(TokenTree::Ident(p)) if p == crate::util::DOLLAR_ESCAPE);
                let followed_by_sep = is_path_sep(tts.get(i + 1)) && is_path_sep(tts.get(i + 2));
                if escaped || !followed_by_sep {
                    out.extend(std::iter::once(tts[i].clone()));
                    i += 1;
                    continue;
                }
                // The run of leading keywords: one `crate`/`self`, or
                // `super :: super :: ..`; `last` is the index of its last ident.
                let span = id.span();
                let kind = id.to_string();
                let mut supers = 0;
                let mut last = i;
                if kind == "super" {
                    supers = 1;
                    while is_path_sep(tts.get(last + 1))
                        && is_path_sep(tts.get(last + 2))
                        && matches!(tts.get(last + 3), Some(TokenTree::Ident(n)) if n == "super")
                    {
                        last += 3;
                        supers += 1;
                    }
                }
                let replacement: Option<Vec<Ident>> = match kind.as_str() {
                    "crate" => {
                        if crate::record::is_local_path(stage) {
                            None
                        } else {
                            stage.first().map(|c| vec![Ident::new(&c.to_string(), span)])
                        }
                    }
                    "self" => Some(stage.iter().map(|s| Ident::new(&s.to_string(), span)).collect()),
                    _ => {
                        if stage.len() > supers {
                            Some(stage[..stage.len() - supers].iter().map(|s| Ident::new(&s.to_string(), span)).collect())
                        } else {
                            None
                        }
                    }
                };
                match replacement {
                    Some(segs) if !segs.is_empty() => {
                        for (n, seg) in segs.iter().enumerate() {
                            if n > 0 {
                                out.extend(quote!(::));
                            }
                            out.extend(std::iter::once(TokenTree::Ident(seg.clone())));
                        }
                        i = last + 1;
                    }
                    _ => {
                        out.extend(std::iter::once(tts[i].clone()));
                        i += 1;
                    }
                }
            }
            other => {
                out.extend(std::iter::once(other.clone()));
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(s: &str) -> Vec<Ident> {
        s.split("::").map(|s| Ident::new(s, Span::call_site())).collect()
    }

    #[test]
    fn scalars_round_trip() {
        assert_eq!(decode::<String>(encode("a \"b\"")).unwrap(), "a \"b\"");
        assert_eq!(decode::<bool>(encode(&true)).unwrap(), true);
        assert_eq!(decode::<u32>(encode(&7u32)).unwrap(), 7);
        assert_eq!(decode::<i64>(encode(&-42i64)).unwrap(), -42);
        assert_eq!(decode::<usize>(encode(&0usize)).unwrap(), 0);
        assert!(decode::<u8>(encode(&300u32)).is_err());
        let p: syn::Path = syn::parse_quote!(crate::a::B);
        assert_eq!(decode::<syn::Path>(encode(&p)).unwrap(), p);
        let id = Ident::new("x", Span::call_site());
        assert_eq!(decode::<Ident>(encode(&id)).unwrap(), id);
        let _: Span = decode(encode(&Span::call_site())).unwrap();
        let ts: TokenStream = quote!(a + $ b);
        assert_eq!(decode::<TokenStream>(encode(&ts)).unwrap().to_string(), ts.to_string());
    }

    #[test]
    fn compounds_round_trip() {
        let v = vec![Some(1u32), None, Some(3)];
        assert_eq!(decode::<Vec<Option<u32>>>(encode(&v)).unwrap(), v);
        let empty: Vec<String> = Vec::new();
        assert_eq!(decode::<Vec<String>>(encode(&empty)).unwrap(), empty);
        let t = ("x".to_string(), vec![true]);
        assert_eq!(decode::<(String, Vec<bool>)>(encode(&t)).unwrap(), t);
    }

    #[test]
    fn fields_round_trip_and_tolerate_unknown_and_missing() {
        let tokens = Fields::new().with("name", "users").with("n", &2u8).with("path", &syn::parse_quote!(crate::x) as &syn::Path).encode();
        assert_eq!(tokens.to_string(), "{ name : \"users\" , n : 2 , path : crate :: x , }");
        let f = Fields::decode(tokens).unwrap();
        assert_eq!(f.get::<String>("name").unwrap(), "users");
        assert_eq!(f.get::<u8>("n").unwrap(), 2);
        assert!(f.has("path") && !f.has("nope"));
        assert_eq!(f.get::<bool>("nope").unwrap_err().to_string(), "exported fact has no field `nope`");
        assert_eq!(f.get_or::<bool>("nope", true).unwrap(), true);
    }

    #[test]
    fn rebasing_follows_the_listed_stage_path() {
        let t = |s: &str| -> TokenStream { s.parse().unwrap() };
        let r = |tokens: &str, st: &str| rebase_paths(t(tokens), &stage(st)).to_string();
        // From another crate: `crate` becomes the crate name.
        assert_eq!(r("crate :: a :: B", "up::plan"), "up :: a :: B");
        assert_eq!(r("[ crate :: a , crate :: b ]", "up::plan"), "[up :: a , up :: b]");
        // Local stage: `crate` stays.
        assert_eq!(r("crate :: a :: B", "crate::plan"), "crate :: a :: B");
        // `self` is the stage module, `super` its parent.
        assert_eq!(r("self :: a :: B", "up::plan"), "up :: plan :: a :: B");
        assert_eq!(r("super :: a :: B", "up::plan"), "up :: a :: B");
        assert_eq!(r("super :: super :: a", "up::x::plan"), "up :: a");
        assert_eq!(r("super :: super :: a", "plan"), "super :: super :: a", "not enough segments: left alone");
        // Later segments and `$crate` are not touched; other roots neither.
        assert_eq!(r("a :: crate :: b", "up::plan"), "a :: crate :: b");
        assert_eq!(r("path : crate :: b ,", "up::plan"), "path : up :: b ,", "a field separator does not hide a path start");
        assert_eq!(r("__pl_dollar crate :: b", "up::plan"), "__pl_dollar crate :: b");
        assert_eq!(r("other :: crate_thing", "up::plan"), "other :: crate_thing");
        assert_eq!(r("self", "up::plan"), "self");
    }
}
