#!/usr/bin/env bash
# End-to-end demo of RSScript's capability-aware review pipeline.
#
#   Phase 1  rss pkg review   — list a package's powers, ranked by risk
#   Phase 3  rss pkg diff     — what powers did a change introduce?
#   Phase 2  rss pkg lock      — a provider swap changes the review hash
#   Phase 0  rss check         — effect-annotated closures parse; bad category flagged
#
# Run from anywhere; it locates the repo root and builds `rss` if needed.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
before="$here/before"
after="$here/after"

# Prefer a prebuilt binary; otherwise build it once.
if [[ -x "$root/target/debug/rss" ]]; then
  RSS="$root/target/debug/rss"
elif [[ -x "$root/target/release/rss" ]]; then
  RSS="$root/target/release/rss"
else
  echo "building rss ..." >&2
  (cd "$root" && cargo build --quiet --bin rss)
  RSS="$root/target/debug/rss"
fi

rule() { printf '\n=== %s ===\n\n' "$1"; }

rule "PHASE 1 — capability-aware review (powers ranked by risk)"
echo "\$ rss pkg review before        # the data package today"
"$RSS" pkg review "$before" | sed -n '/capabilities (by risk)/,/^exports:/p' | grep -v '^exports:' || true
echo
echo "\$ rss pkg review after         # after a PR adds an outbound HTTP call"
"$RSS" pkg review "$after" | sed -n '/capabilities (by risk)/,/^exports:/p' | grep -v '^exports:' || true

rule "PHASE 3 — AI review: what powers did this change introduce?"
echo "\$ rss pkg diff before after"
"$RSS" pkg diff "$before" "$after" | sed -n '/^package diff/,/^interface /p' | grep -vE '^interface ' || true

rule "PHASE 2 — provider pinning (reproducible review)"
echo "Only change: Net.fetch provider  reqwest -> evil-corp  (identical code)"
evil="$(mktemp -d)/after_evil"
cp -r "$after" "$evil"
sed -i.bak 's/provider = "reqwest"/provider = "evil-corp"/' "$evil/rsspkg.toml" && rm -f "$evil/rsspkg.toml.bak"
h_ok="$("$RSS" pkg lock "$after" | grep review_hash | head -1)"
h_evil="$("$RSS" pkg lock "$evil" | grep review_hash | head -1)"
echo "reqwest  : $h_ok"
echo "evil-corp: $h_evil"
if [[ "$h_ok" != "$h_evil" ]]; then
  echo "=> review_hash CHANGED — the lock catches the provider swap."
else
  echo "=> hashes equal (this would be a miss)"
fi

rule "PHASE 0 — effect-annotated closures parse; unknown category flagged"
closure="$(mktemp --suffix=.rss)"
cat > "$closure" <<'RSS'
fn apply(f: noescape Fn() -> Unit) -> Unit
fn run() -> Unit {
    apply(f: read || {
        Log.write(message: read "hello from an inline closure")
    })
    return Unit
}
RSS
echo "\$ rss check <file with 'read || { ... }'>   # was RS0015 'unsupported syntax'"
"$RSS" check "$closure"
echo
bogus="$(mktemp -d)/bogus"
cp -r "$before" "$bogus"
sed -i.bak 's/category = "database.read"/category = "databse.raed"/' "$bogus/rsspkg.toml" && rm -f "$bogus/rsspkg.toml.bak"
echo "\$ rss pkg review <package with a typo'd category 'databse.raed'>"
"$RSS" pkg review "$bogus" | sed -n '/capabilities (by risk)/,/^exports:/p' | grep -v '^exports:' || true

printf '\nDone.\n'
