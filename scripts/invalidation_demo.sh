#!/usr/bin/env bash
# G3 (and G8 for stages): the compiler sees the dependency from a source to
# the expand! site, even though the two live in different files (and, for
# the second half, different crates).
#
#   1. Baseline build passes.
#   2. Rename `users.email` -> `users.email_address` in users.rs.
#      orders.rs is untouched, but its `OrderOwnerEmail` query reads
#      `users.email`, so `cargo build` must now fail at the expand! site
#      with the error pointing into orders.rs.
#   3. Revert; build passes again.
#   4. Add a column to demo-upstream's `accounts` table. demo-app is
#      untouched, but its `AccountForOrder` is `SELECT * FROM accounts`, so
#      the rendered SQL const in demo-app must change. The same edit also
#      changes `plan2`'s `AccountById` (G8): `plan2` lists no raw upstream
#      source, only the stage `demo_upstream::plan`, so the table reached it
#      as an exported fact inside the stage carrier's tokens. The edit changes
#      those tokens, so `plan2` expands again.
#   5. Revert.
#
# Why this works: the carrier is an ordinary macro_rules! item in the crate
# that owns the source. Editing users.rs changes demo-app's source
# fingerprint; editing demo-upstream changes a dependency, so cargo rebuilds
# demo-app and expand!/__finish re-run with new tokens. There is no hidden
# state: everything __finish sees arrives as tokens through the normal
# expansion of the crate being compiled.
set -euo pipefail
cd "$(dirname "$0")/.."

USERS=examples/demo-app/src/users.rs
UP=examples/demo-upstream/src/lib.rs
restore() {
  sed -i "s/pub email_address: String,/pub email: String,/" "$USERS"
  sed -i "/pub nickname: String,/d" "$UP"
  rm -f examples/demo-app/examples/probe.rs; rmdir examples/demo-app/examples 2>/dev/null || true
}
trap restore EXIT

echo "== 1. baseline"
cargo build -p demo-app --quiet && echo "   ok"

echo "== 2. rename users.email -> users.email_address (orders.rs untouched)"
sed -i 's/pub email: String,/pub email_address: String,/' "$USERS"
if cargo build -p demo-app --quiet 2> target/inval.err; then
  echo "   UNEXPECTED: build passed"; exit 1
fi
grep -q 'column `email` does not exist on table `users`' target/inval.err && echo "   build failed as expected:"
grep -A1 "does not exist" target/inval.err | sed 's/^/   | /'

echo "== 3. revert"
sed -i 's/pub email_address: String,/pub email: String,/' "$USERS"
cargo build -p demo-app --quiet && echo "   ok"

echo "== 4. add column nickname to upstream accounts (demo-app untouched)"
sed -i 's/pub owner_email: String,/pub owner_email: String,\n        pub nickname: String,/' "$UP"
mkdir -p examples/demo-app/examples
cat > examples/demo-app/examples/probe.rs <<'RS'
fn main() {
    println!("{}", demo_app::sql::<demo_app::orders::orders::AccountForOrder>());
    println!("{}", demo_app::sql2::<demo_app::staged::staged::AccountById>());
}
RS
out=$(cargo run -p demo-app --example probe --quiet)
echo "   AccountForOrder SQL now (plan, raw upstream sources):   $(echo "$out" | sed -n 1p)"
echo "   AccountById SQL now     (plan2, through the stage, G8): $(echo "$out" | sed -n 2p)"
echo "$out" | sed -n 2p | grep -q nickname && echo "   G8 holds: the stage carrier's tokens changed, plan2 re-expanded"

echo "== 5. revert"
sed -i '/pub nickname: String,/d' "$UP"
out=$(cargo run -p demo-app --example probe --quiet)
echo "   AccountForOrder SQL back to: $(echo "$out" | sed -n 1p)"
echo "   AccountById SQL back to:     $(echo "$out" | sed -n 2p)"
echo "== done"
