# ADR 0241: `match` guards lower, and a false guard falls through to the next arm

## Status

Accepted. Date: 2026-09-23.

## Problem

The checker accepts a guarded `match` arm, `pattern if condition => { … }`: it
types the guard with the arm's pattern bindings in scope, rejects a guard that
uses a mutating effect (`match_guard_mutation_diagnostic`), and leaves guarded
arms out of the exhaustiveness calculation. The checked-HIR-to-MIR lowerer did
not. Every guarded arm refused with `checked HIR match guard` (statement form)
or `checked HIR match expression guard` (expression form and a `match` in an
expression arm's value position), so a program `rss check` reported clean
failed `rss build` and `rss run`:

```rsscript
struct P {
    x: Int
}
fn main() -> Int {
    let p = P(x: 5)
    match p {
        P { x } if x > 3 => { return x }
        _ => { return 0 }
    }
}
```

That breaks the project rule that a program the checker accepts builds,
verifies, and runs.

## Decision and non-goals

Guards lower, with this evaluation order for each arm, in written order:

1. The arm's pattern is tested against the scrutinee.
2. If it matches, the pattern's bindings are written.
3. If the arm has a guard, the guard is evaluated with those bindings in scope.
   If it is `false`, control continues to the next arm's pattern test, exactly
   as if the arm's pattern had not matched. If it is `true`, the arm's body
   runs.

The first arm whose pattern matches and whose guard (if any) is `true` is the
arm that runs; no later arm is tested. This holds for every pattern form the
lowerer accepts (literal, `Option`/`Result`, declared sum variant in either
spelling, tuple, list, struct) and for statement `match`, expression `match`,
and a `match` in an expression arm's value position. The three share one arm
loop, `lowerer_calls.rs::lower_match_arms`, so they cannot accept different
guard shapes.

A guard is lowered as an ordinary expression followed by a MIR `Branch` whose
false edge is the next arm's test block. No MIR instruction, terminator,
verifier rule, or bytecode opcode is added.

Unchanged: the checker's rules. Guarded arms still do not count toward
exhaustiveness, and a guard still may not use a mutating effect. Because a
guard cannot mutate, evaluating it and then falling through leaves no state to
undo; the bindings the failed arm wrote are dead on that path.

Non-goal: this record does not change which patterns the checker accepts.

## Compatibility and migration

Language: no program changes meaning. Programs with guards that used to check
clean and then fail to build now build and run with the order above. No
program that built before builds differently. Artifact, bytecode, MIR, and
Provider formats are unchanged. Rollback is a revert, which returns guarded
arms to a build-time refusal.

## Verifier and security impact

None. The emitted MIR uses only existing instructions and terminators, and the
Artifact verifier checks it as it checks any `if`.

## Provider and backend impact

None for Providers. The guard is a compare and a `Branch`, both in the native
tier's subset, so a function with a guarded `match` compiles to native code
like one with an `if` in the arm; the `guarded-match-loop.rss` differential
case confirms the native tier and the interpreter agree and that the guarded
callees compile.

## Evidence

- `rsscript-sdk` (`--features execution`):
  `a_match_guard_reads_its_arm_binding` and `match_guards_lower_and_execute`
  build, verify, and run guarded matches over every lowerable pattern form in
  statement and expression position, including fall-through from a false guard
  to a later arm.
- `crates/rsscript-sdk/tests/fixtures/pass/match-guards.rss`, covered by
  `fixture_corpus` (checks clean) and `fixture_build_corpus` (builds and
  verifies).
- `rsscript-sdk` `native_jit_differential`: the `guarded-match-loop.rss` case
  in `native_engine_matches_the_verified_interpreter_corpus`.
