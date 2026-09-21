# ADR 0237: A closure's result type is proved, not defaulted

- Status: Accepted
- Date: 2026-09-20

## Problem

Writing reference solutions for the eval corpus turned up two check/build
splits — programs `rss check` reports nothing about and that then die in the
backend, the worst failure shape for a language whose programs are written by
models and validated by the checker.

**The closure's result was invented.** A core combinator's callback contract is
`noescape Fn(T) -> U`. Argument-position inference matched an inline closure's
body against the contract's return position, but nothing bound the closure's
*parameters* to the `T` the receiver argument had already proved. `|x| { return
x * 2 }` therefore had no inferable body — and the `return` arm defaulted an
un-inferable returned expression to `Unit`. That default was published as a
proof, and the caller unified its own result parameter against it:

- `List.map` over a `List<Int>` produced `List<Unit>`, so the *next* line was
  rejected for a mismatch it did not cause;
- `Option.and_then` and `Result.and_then`, whose `U` sits one level down inside
  `Option<U>`/`Result<U, E>`, matched nothing and left `U` unresolved, so
  `Option.unwrap_or(value: r, default: 0)` was reported as `Int` against `U`.

This is the closure-result instance of the rule 1657c8f7 already established
for closure *parameters*: a type spelling nothing proved must not be published
as evidence.

**Half the combinators had no lowering.** `List.map`, `List.filter`,
`List.fold`, `List.sort_by`, `List.sort_with` and `Set.for_each` are declared
`lowering = "special"` in `crates/rsscript-compiler/intrinsics.toml`, which
hands them to the checked-HIR-to-MIR lowerer — and the lowerer had a case for
none of them. Every program using one reached "unsupported checked HIR builtin
call" after checking clean. Their register-VM opcodes and v1 bytecode operand
tables already existed; only the MIR step was missing.

## Decision and non-goals

Inference, in `rsscript-semantics`:

1. `infer_closure_return_type`'s `return` arm answers `Unit` only for `return`
   with no value. `return <expr>` is exactly as typed as `<expr>` is, and
   "not inferable" is reported as such.
2. `closure_body_value_types` binds each closure parameter to the contract's
   corresponding parameter type with the substitutions proved so far applied,
   so the body's inference can see `x: Int`. A position still naming one of the
   callee's own type parameters, or the reserved unresolved spelling, is left
   *unbound* rather than bound to a placeholder.
3. An inline closure in a `noescape Fn(...)` argument position is matched
   against the contract's return position and nothing else, and never falls
   through to structural matching — doing so would unify the callee's result
   parameter against the closure's own unresolved marker.
4. `infer_arg_expr_type` still publishes the structural `Fn` contract when the
   body's result is unproved, with `WireType::UNRESOLVED` in the result
   position. Arity, parameter modes and `noescape` are real facts that MIR
   lowering needs; only the result records the absence, through the spelling
   `rsscript-codegen-vm` already projects to `TypedFactTypeV1::Unknown`.

Lowering: six MIR instructions — `ListMap`, `ListFilter`, `ListFold`,
`ListSortBy`, `ListSortWith`, `SetForEach` — with lowerer cases,
`rsscript-codegen-vm` emitters onto the existing register-VM opcodes, MIR
verifier definition/use/liveness rules, and a declining arm in the conformance
oracle, which does not execute callback bodies for the same reason it declines
`CallClosure`.

Non-goals. No change to the intrinsic catalog, so no `BuiltinId` moves and no
builtin-registry digest changes. `Pipeline.map` and `Pipeline.filter` remain
unlowered: unlike the six above they have no register-VM opcode, so closing
them means adding catalog entries, which would renumber every later `BuiltinId`
— a separate change with a real compatibility cost. No diagnostic code is added
or retired.

> **Superseded in part by ADR 0239.** The `BuiltinId` cost priced here was a
> cost of *where* a catalog entry is filed, not of adding one: a `BuiltinId` is
> a binding's index in the direct-lowering table read in file order, so an
> entry appended at the end renumbers nothing. `Pipeline.map` and
> `Pipeline.filter` are lowered as of ADR 0239, along with `List.sort`, which
> this ADR's list of `special` combinators missed.

## Compatibility and migration

MIR gains six instruction variants. `MirInstruction` is `#[non_exhaustive]`-free
but internal to this workspace and reconstructed from source on every build; no
MIR is persisted. The emitted v1 bytecode uses opcodes, operand names and
register conventions the encoder and the register VM already accepted, so an
Artifact produced before this change verifies and runs unchanged, and an
Artifact produced after it uses no new opcode.

No ABI or wire type changes. `rsscript-abi-model` is untouched.

The inference change alters which programs are *accepted*: programs previously
rejected because a combinator's result was inferred as `Unit` or left unresolved
now check. No program that checked before stops checking — the change only
removes a fabricated type and adds proved ones. The workspace suite, the
`pass`/`fail` fixture corpora and the build corpus pin that.

Rollback is a revert.

## Verifier and security impact

Neither verifier is weakened. The MIR verifier treats each combinator's
receiver and callback as ordinary operands, with the callback's own ABI still
proved by the `MakeClosure` contract it already checks; `ListSortWith`'s
receiver is a mutable place and gets the same liveness check as `ListPush`. The
bytecode typed-facts verifier sees the same registers it saw for the
hand-written opcodes. Removing a fabricated `Unit` narrows what is asserted, it
does not widen what is accepted.

## Provider and backend impact

No Provider surface changes. The register VM already implements all six
opcodes. The native tier declines regions containing a `CallClosure` whose
closure operand it cannot resolve, which covers these combinators' callback
entries unchanged.

## Evidence

- `rsscript-sdk` (lib, `execution`): `closure_taking_combinator_programs_verify_and_run`.
- `rsscript-sdk` fixtures: `pass/list-map-closure.rss`,
  `pass/option-and-then-nested-closure.rss`, `pass/result-map-closure.rss`,
  `pass/set-for-each-closure.rss`, each covered by `fixture_corpus` and
  `fixture_build_corpus`.
- A sweep of all 39 core-interface functions taking an `Fn(...)` parameter,
  each called with an inline closure: 37 check, build, verify and run;
  `Pipeline.map` and `Pipeline.filter` remain the recorded gap.
