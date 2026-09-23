# ADR 0242: A bare identifier `match` arm binds the whole scrutinee

## Status

Accepted. Date: 2026-09-23.

## Problem

`match n { 0 => { … } other => { return other } }` was rejected with RS0209
(`match pattern `other` cannot match scrutinee type `Int``), RS0026 (unknown
binding `other`), and RS0021 (not exhaustive). The AST has
`MatchPattern::Binding`, the HIR types it (`match_pattern_binding_types`),
exhaustiveness counts it as a catch-all, and MIR lowering binds it, but the
parser cannot tell a binding from a payload-free case when the arm is one bare
name: `North =>` and `other =>` are the same tokens. It emitted both as a case
pattern, so no top-level arm could ever be a binding, and `_` was the only
catch-all.

## Decision and non-goals

A `match` arm whose whole pattern is one unqualified name, with no payload, is
resolved during HIR construction (`Hir::resolve_bare_arm_pattern`), declared
case first, the same order `Hir::is_nullary_enum_variant` uses for a bare name
in an expression:

1. **Declared case.** If the name is a declared case, it stays a case pattern:
   the builtin `None`, `Some`, `Ok`, `Err`, or any `sum` case, with or without
   fields, lowercase or not. This holds whatever the scrutinee's type, so a
   case of the wrong family is still reported (RS0209) instead of binding.
2. **Binding.** Otherwise, if the name does not start with an uppercase letter,
   the arm binds the whole scrutinee. The binding has the scrutinee's type, is
   in scope in the guard and the body, is irrefutable, and makes the match
   exhaustive exactly as `_` does.
3. **Unknown case.** Otherwise (a capitalized name that is no declared case,
   such as a misspelled `Nroth` or a constant-looking `MAX`) it stays a case
   pattern and is reported as before (RS0209 against the scrutinee's type), so
   a typo cannot silently become a catch-all.

Rule 3 matches the rule the parser already applies inside a payload position,
where `Some(v)` binds and `Some(None)` tests a case by capitalization.

A binding arm may carry a guard (`x if x > 3 => …`, ADR 0241); a guarded binding
arm, like every guarded arm, does not count toward exhaustiveness.

Non-goals:

- Sub-patterns are unchanged. Inside a payload, field, tuple, or list position
  the parser still decides by capitalization alone, so a lowercase declared
  case there is a binding, as before.
- Arms after an unguarded binding arm are unreachable. There is no
  unreachable-arm diagnostic in the checker today, for this or for `_`, and
  this record does not add one; such arms are accepted and never run.
- The AST is unchanged. `name()` with an empty payload parses to the same node
  as `name` (and `rss fmt` already prints it as `name`), so it resolves the
  same way.
- AST-only tooling (the LSP symbol index, `rss review` facts) still sees the
  unresolved node; only the checker, HIR, and lowering see the binding.

## Compatibility and migration

Language: programs that were rejected now check, build, and run. No program
that built before changes meaning. An arm that resolves to a binding under
rule 2 names no declared case, so before this change it was an RS0209 error
when the scrutinee's type was known, and a build-time refusal (`unresolved
checked HIR variant match pattern`) when it was not. Every name that resolved
to a declared case still does (rule 1). Artifact, MIR, bytecode, and Provider formats are
unchanged. Rollback is a revert, which makes those programs errors again.

## Verifier and security impact

None. A binding arm lowers to an unconditional edge and a `WritePlace`, which
MIR and the Artifact verifier already handle for `_` and for nested bindings.

## Provider and backend impact

None. Lowering already supported a top-level `MatchPattern::Binding`; it was
never reached.

## Evidence

- `rsscript-semantics`: `hir::tests::a_bare_arm_name_resolves_declared_case_first_then_binding`.
- `rsscript-sdk` (`--features execution`):
  `bare_identifier_arms_bind_the_whole_scrutinee` (Int, String, Option, sum,
  struct, tuple, and List scrutinees; guarded binding arms; statement and
  expression `match`; declared nullary cases beside binding arms) and
  `a_binding_arm_returns_the_scrutinee`.
- Fixtures: `pass/match-binding-arm.rss` (checks, builds, verifies);
  `fail/match-bare-case-is-not-a-binding.rss`,
  `fail/match-bare-unknown-capitalized-name.rss`, and
  `fail/match-none-arm-on-string.rss` pin rules 1 and 3.
