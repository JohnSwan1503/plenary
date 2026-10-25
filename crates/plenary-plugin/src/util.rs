//! Small deterministic helpers shared by the engine.

use proc_macro2::{Group, Ident, Punct, Spacing, Span, TokenStream, TokenTree};

/// FNV-1a, 64 bit. Deterministic across processes (unlike `DefaultHasher`),
/// which is what hidden generated names need (G1).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Short hex digest of a string, for hidden names.
pub fn digest(s: &str) -> String {
    format!("{:016x}", fnv1a(s.as_bytes()))
}

/// Edit distance, for "did you mean" suggestions.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The closest candidate to `name`, if it is close enough to be a likely typo.
pub fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        let d = levenshtein(name, c);
        let limit = (name.len().max(c.len()) / 3).max(2);
        if d <= limit && d < name.len().max(c.len()) && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

/// Give every token in `tokens` the span `span`, except `$crate` which must
/// keep the hygiene of its own span.
pub fn respan(tokens: TokenStream, span: Span) -> TokenStream {
    tokens
        .into_iter()
        .map(|tt| match tt {
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), respan(g.stream(), span));
                ng.set_span(span);
                TokenTree::Group(ng)
            }
            TokenTree::Ident(i) => {
                if i == "$crate" {
                    TokenTree::Ident(i)
                } else {
                    let mut i = i;
                    i.set_span(span);
                    TokenTree::Ident(i)
                }
            }
            TokenTree::Punct(p) => {
                let mut p = Punct::new(p.as_char(), p.spacing());
                p.set_span(span);
                TokenTree::Punct(p)
            }
            TokenTree::Literal(mut l) => {
                l.set_span(span);
                TokenTree::Literal(l)
            }
        })
        .collect()
}

/// Name used for an escaped `$` token inside carrier bodies. Reserved.
pub const DOLLAR_ESCAPE: &str = "__pl_dollar";

/// Replace every bare `$` punct with the ident [`DOLLAR_ESCAPE`], keeping its
/// span. A carrier is a `macro_rules!` whose transcriber contains the user's
/// tokens verbatim, and a bare `$` there would be read as a metavariable.
pub fn escape_dollars(tokens: TokenStream) -> TokenStream {
    tokens
        .into_iter()
        .map(|tt| match tt {
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), escape_dollars(g.stream()));
                ng.set_span(g.span());
                TokenTree::Group(ng)
            }
            TokenTree::Punct(p) if p.as_char() == '$' => {
                TokenTree::Ident(Ident::new(DOLLAR_ESCAPE, p.span()))
            }
            other => other,
        })
        .collect()
}

/// Inverse of [`escape_dollars`].
pub fn unescape_dollars(tokens: TokenStream) -> TokenStream {
    tokens
        .into_iter()
        .map(|tt| match tt {
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), unescape_dollars(g.stream()));
                ng.set_span(g.span());
                TokenTree::Group(ng)
            }
            TokenTree::Ident(i) if i == DOLLAR_ESCAPE => {
                let mut p = Punct::new('$', Spacing::Alone);
                p.set_span(i.span());
                TokenTree::Punct(p)
            }
            other => other,
        })
        .collect()
}

/// Render a path as the user would write it (`crate::users::users`).
pub fn path_string(segments: &[Ident]) -> String {
    segments.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
}

/// Render a `syn::Path` as the user would write it (`crate::users::Users`),
/// without the spaces `quote!` inserts. For diagnostics.
pub fn path_text(path: &syn::Path) -> String {
    let mut s = String::new();
    if path.leading_colon.is_some() {
        s.push_str("::");
    }
    for (i, seg) in path.segments.iter().enumerate() {
        if i > 0 {
            s.push_str("::");
        }
        s.push_str(&seg.ident.to_string());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn digest_is_stable() {
        assert_eq!(digest("abc"), digest("abc"));
        assert_ne!(digest("abc"), digest("abd"));
    }

    #[test]
    fn suggestions() {
        assert_eq!(suggest("tabel", ["table", "query"]), Some("table"));
        assert_eq!(suggest("zzzzzz", ["table", "query"]), None);
        assert_eq!(suggest("greet", ["greet", "shout"]), Some("greet"));
    }

    #[test]
    fn dollars_round_trip() {
        let src = quote! { macro_rules! m { ($x:expr) => { $x + $crate::y } } };
        let escaped = escape_dollars(src.clone());
        assert!(!escaped.to_string().contains('$'));
        assert_eq!(unescape_dollars(escaped).to_string(), src.to_string());
    }
}
