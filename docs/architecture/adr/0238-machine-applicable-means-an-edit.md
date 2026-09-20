# ADR 0238: A machine-applicable fix carries an edit

- Status: Accepted
- Date: 2026-09-20

## Problem

`machine-applicable` is a promise to the consumer — an editor, a repair loop, a
model — that a fix can be applied without a person reading it. ADR 0236
measured what that promise is worth: a fix carrying a concrete replacement gets
taken and cleared, and one that only describes the failure persists.

The eval report found `RS0306` making that promise and carrying no edit:
`applicability` was `machine-applicable` and `replacement` was null. A consumer
either drops such a fix or, worse, invents the edit itself. Nothing in the
repository asserted the invariant, so the gap was found by reading output.

## Decision and non-goals

1. `RS0306`'s fix carries the edit. The binding's primary span *is* the `local`
   keyword — verified across `local x`, `local mut x`, nested blocks and
   irregular spacing — so `let` replaces exactly that span and the rest of the
   line is untouched. A span whose length is not the keyword's is not rewritten
   sight-unseen; that case degrades to `manual`.
2. `RS0306`'s static explanation says the instance fix carries that edit.
3. Two other fixes that made the same promise are downgraded to `manual`
   because their spans cannot carry a derived edit:
   - `RS0706`'s span is the producer's *callee name*, not the producer
     expression, so there is no position in it at which `?` can be inserted
     without guessing where the call ends;
   - `RS0202`'s span names the receiver alone, so an inserted effect keyword is
     correct only when the source spells no effect there — and `read` is
     spellable, which the fact cannot distinguish from the implicit default.
4. `machine_applicable_fixes_carry_an_edit` walks every `tests/fixtures/fail`
   fixture through the same checker `rss check --json` runs and asserts that
   every machine-applicable fix carries an edit that changes something.

Non-goals. No diagnostic code is added or retired, no program's accept/reject
outcome changes, and the descriptive titles of the two downgraded fixes are
unchanged — a consumer reading the title sees what it always saw. Deriving real
edits for `RS0706` and `RS0202` needs spans those diagnostics do not currently
carry; widening them is separate work.

## Compatibility and migration

No ABI, MIR, bytecode, Artifact, Provider or persisted-data contract changes.
The `Diagnostic`/`Fix`/`FixEdit` JSON shape is unchanged: `Fix` already carried
an optional `edit`, and `RS0306` instances now populate the field the schema
always had.

Two `applicability` values change from `machine-applicable` to `manual`. That is
the direction that removes a false promise: a consumer applying only
machine-applicable fixes now applies fewer, and every one it applies is real.
`rss fix --write` already applied only fixes carrying an edit, so its behavior
on `RS0706` and `RS0202` is unchanged in practice.

`docs/generated/diagnostic-catalog.{md,json}` are regenerated from the
explanation registry.

Rollback is a revert.

## Verifier and security impact

None. A fix edit is data returned to a caller that chooses whether to apply it,
and no validation is relaxed.

## Provider and backend impact

None. These are semantic-frontend diagnostics emitted before any lowering.

## Evidence

- `rsscript-sdk`: `fixture_corpus::machine_applicable_fixes_carry_an_edit`,
  which is the standing assertion, not a single-case regression test.
- `rsscript-xtask`: `language-card --check`, over the regenerated catalog.
