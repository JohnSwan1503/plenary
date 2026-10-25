#!/usr/bin/env bash
# Measures how many sources one expand! can take before hitting
# `recursion_limit`, and how long big inputs take. Usage:
#   scripts/scale_probe.sh                # default sweep
#   scripts/scale_probe.sh 200 5 10       # sources, tables/source, cols/table
#   scripts/scale_probe.sh 200 5 10 1024  # ... with #![recursion_limit = "1024"]
#   scripts/scale_probe.sh 40 2 5 "" cond # ... with one conditional record per table
#
# Expansion depth of a collection = 1 (expand!) + 1 per source (its carrier
# chains straight to the next one) + 1 per conditional record (its #[cfg]
# carrier pair) + 1 (__finish) + the deepest macro nesting inside what the
# plugins emit (1 here: stq-lite's type obligations wrap a
# #[diagnostic::on_unimplemented] trait, and the registry's closed enum has a
# #[derive]; rustc names whichever it meets first). It must not exceed
# recursion_limit.
# Results observed (rustc 1.97, debug, carrier format v2):
#   limit 128:  125 sources ok, 126 fail (1 + 125 + 1 + 1 = 128)
#               1 conditional record per source: 62 ok, 63 fail
#               2 conditional records per source: 41 ok, 42 fail
#   limit 1024: 1021 sources ok, 1022 fail; 300 sources x 5 tables x 10 cols ok, ~5s
# Before v2 (expand! called every carrier itself, two levels per source):
#   limit 128:  62 ok, 63 fail; 2 conditional records per source: 31 ok, 32 fail
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/target/scale-probe"; mkdir -p "$DIR/src"
cat > "$DIR/Cargo.toml" <<TOML
[package]
name = "scale-probe"
version = "0.0.0"
edition = "2021"
[dependencies]
plenary = { path = "$ROOT/crates/plenary" }
demo-macros = { path = "$ROOT/examples/demo-macros" }
stq-lite = { path = "$ROOT/examples/stq-lite" }
registry = { path = "$ROOT/examples/registry" }
[workspace]
TOML
run() {
  python3 "$ROOT/scripts/gen_scale.py" "$1" "$2" "$3" ${5:-} > "$DIR/src/lib.rs"
  [ -n "${4:-}" ] && sed -i "1i #![recursion_limit = \"$4\"]" "$DIR/src/lib.rs"
  printf "%4s sources x %3s tables x %3s cols %-5s limit=%-6s " "$1" "$2" "$3" "${5:-}" "${4:-128}"
  s=$(date +%s.%N)
  if out=$(cd "$DIR" && CARGO_TARGET_DIR="$ROOT/target/scale-target" cargo build --quiet 2>&1); then
    printf "ok    %.1fs\n" "$(echo "$(date +%s.%N)-$s" | bc)"
  else
    echo "FAIL: $(echo "$out" | grep -m1 -E '^error' | cut -c1-110)"
  fi
}
if [ $# -ge 3 ]; then run "$1" "$2" "$3" "${4:-}" "${5:-}"; exit; fi
run 10 2 5
run 125 1 2
run 126 1 2
run 62 1 2 "" cond
run 63 1 2 "" cond
run 41 2 2 "" cond
run 42 2 2 "" cond
run 300 5 10 1024
run 1021 1 2 1024
run 1022 1 2 1024
