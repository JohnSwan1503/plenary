#!/usr/bin/env python3
"""Generate a crate body with M sources x T tables (+ T queries) x C columns.

Usage: gen_scale.py M T C [conditional]
With a 4th argument, every query gets `#[cfg(all())]`, so each source also has
T conditional records (one extra expansion level each, see SPEC section 6).
"""
import sys

M, T, C = map(int, sys.argv[1:4])
conditional = len(sys.argv) > 4
out = []
for m in range(M):
    out.append(f"#[demo_macros::collect] pub mod m{m} {{")
    for t in range(T):
        cols = ", ".join(f"pub c{c}: i64" for c in range(C))
        out.append(f'  #[stq::table(name = "t{m}_{t}")] pub struct T{t} {{ {cols} }}')
        cfg = "#[cfg(all())] " if conditional else ""
        out.append(f'  {cfg}#[stq::query(sql = "SELECT * FROM t{m}_{t} WHERE c0 = {{c0}}")] pub struct Q{t} {{ pub c0: i64 }}')
    out.append("}")
paths = ", ".join(f"crate::m{m}" for m in range(M))
out.append(f"pub mod plan {{ plenary::expand!(demo_macros; {paths}); }}")
print("\n".join(out))
