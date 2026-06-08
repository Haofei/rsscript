#!/usr/bin/env bash
# End-to-end demo of RSScript's capability-aware review pipeline.
#
#   rss pkg review            — list a package's powers, ranked by risk
#   rss pkg diff              — what powers did a change introduce?
#   rss pkg lock              — a provider swap changes the review hash
#   rss check                 — effect-annotated closures parse; bad category flagged
#   reir report-pr            — reconcile required vs granted with a policy, emit SARIF
#   rss pkg review --markdown — PR-facing review render
#   rss native audit          — native adapter risk facts
#   rss pkg metadata          — fail closed: invalid source yields no evidence
#   rss check --explain --json— machine-readable diagnostic for agents
#
# Run from anywhere; it locates the repo root and builds `rss`/`reir` if needed.
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

if [[ -x "$root/target/debug/reir" ]]; then
  REIR="$root/target/debug/reir"
elif [[ -x "$root/target/release/reir" ]]; then
  REIR="$root/target/release/reir"
else
  echo "building reir ..." >&2
  (cd "$root" && cargo build --quiet --bin reir -p reir)
  REIR="$root/target/debug/reir"
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

rule "GATE — reconcile required vs granted (policy + SARIF)"
work="$(mktemp -d)"
"$RSS" pkg review --json "$after" > "$work/after-review.json" 2>/dev/null || true
"$REIR" collect --producer rsscript --package-review "$work/after-review.json" \
  --out "$work/required.reir.json" >/dev/null
# The deployment grants the package's powers EXCEPT the new outbound network the
# PR introduced (infra wasn't updated) — derived from the real required facts.
python3 - "$work/required.reir.json" "$work/granted.reir.json" <<'PY'
import json, sys
req = json.load(open(sys.argv[1]))
facts = []
for fact in req.get("facts", []):
    cap = fact.get("capability") or {}
    if cap.get("category") == "network.client":
        continue  # infra did not grant the new power
    if fact.get("role") == "required":
        fact = dict(fact, role="granted")
    facts.append(fact)
json.dump({**req, "facts": facts}, open(sys.argv[2], "w"))
PY
echo "\$ reir report-pr --policy examples/rss-policy.toml --target prod --sarif"
set +e
"$REIR" report-pr --required "$work/required.reir.json" --granted "$work/granted.reir.json" \
  --policy "$root/examples/rss-policy.toml" --target prod --sarif 2>/dev/null \
  | python3 -c "import sys,json; d=json.load(sys.stdin); [print('  ', r['level'], r['ruleId'], '-', r['message']['text']) for r in d['runs'][0]['results']]"
"$REIR" report-pr --required "$work/required.reir.json" --granted "$work/granted.reir.json" \
  --policy "$root/examples/rss-policy.toml" --target prod >/dev/null 2>&1
echo "  gate exit code: $?  (non-zero = blocked)"
set -e

rule "RENDER — markdown review for a PR"
echo "\$ rss pkg review --markdown after"
"$RSS" pkg review --markdown "$after" | sed -n '1,14p'

rule "NATIVE AUDIT — adapter risk facts"
echo "\$ rss native audit after"
"$RSS" native audit "$after" 2>/dev/null || true

rule "FAIL CLOSED — invalid source yields no authoritative evidence"
broken="$(mktemp -d)/broken"
cp -r "$after" "$broken"
printf '\nnative fn Broken.x(a: read String -> String\n' >> "$broken/interface/lib.rssi"
echo "\$ rss pkg metadata <package with a parse error>"
set +e
"$RSS" pkg metadata "$broken" >/dev/null 2>&1
echo "  metadata exit code: $?  (non-zero = refused)"
set -e
test -f "$broken/review/reir/rsscript.json" \
  && echo "  REIR written (unexpected)" \
  || echo "  REIR bundle NOT written — evidence withheld for invalid source"

rule "AGENT — machine-readable diagnostic explanation"
echo "\$ rss check --explain RS0015 --json"
"$RSS" check --explain RS0015 --json 2>/dev/null || true

printf '\nDone.\n'
