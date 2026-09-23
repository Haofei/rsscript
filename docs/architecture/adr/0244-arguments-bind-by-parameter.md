# ADR 0244: A call evaluates its arguments as written and passes each to the parameter it names

## Status

Accepted. Date: 2026-09-23.

## Problem

`CallBinding` (`crates/rsscript-semantics/src/call_binding.rs`) records two
facts for every argument: `evaluation_index`, the order the language evaluates
it in (the receiver, then explicit arguments as written, then omitted defaults
in declaration order), and `parameter_index`, the parameter it binds to. MIR
lowering used only the first. `lower_direct_call`, the `async let`/`select`
spawn path, the catalog-builtin call, and every fixed-arity intrinsic case
sorted the arguments by `evaluation_index` and then *passed* them in that
order, and `MirInstruction::Call` carries no parameter index to correct it.
So a call whose labels were written out of declaration order checked clean,
built, and bound the wrong values:

- `fn sub(left: Int, right: Int)` called as `sub(right: 3, left: 10)` returned
  `-7`; a same-name argument written out of order (`sub(right, left)`) was
  swapped the same way;
- `String.concat(right: "b", left: "a")` returned `"ba"`;
- `fn sub(left: Int, middle: Int = 100, right: Int)` called as
  `sub(left: 10, right: 3)` received `middle = 3, right = 100`, because the
  synthesized default was appended after the explicit arguments;
- the same held for generic function instances, receiver-call sugar, external
  Provider calls (where a misplaced `mut` argument's write-back could land on
  another place), protocol dispatch through `Dyn<P>`, and the calls `async let`
  and a `select` arm spawn.

When the misplaced value's type differed from the parameter's, the mistake
surfaced loudly instead — as bytecode verification failure ("typed call
parameter disagrees with its argument register"), a MIR mode mismatch, or a
runtime type error. When the types agreed it was a silent miscompile. Record
and sum-variant constructors placed each field by `parameter_index` and were
correct.

A related hole: a call through a closure value (`f(b: 3, a: 10)` where `f` is a
`local` closure or a `noescape Fn(...)` parameter) resolves to no signature, so
the checker bound nothing and ignored the labels; the arguments were passed by
position whatever they were called.

## Decision and non-goals

The rule, already stated by the language specification (§11 "Evaluation and
arithmetic") and now implemented on every call path: **a call's arguments are
evaluated left to right as written; each value is then passed to the parameter
it names; an omitted defaulted parameter's value is placed in its declaration
position.**

- Lowering has one routine for this,
  `lowerer_calls.rs::lower_arguments_by_parameter`: it lowers the arguments in
  `evaluation_index` order (so their instructions, and any side effects, are
  emitted as written) and returns them in `parameter_index` order. Every call
  instruction's argument list is therefore in declaration order, which is the
  order the callee's ABI, its parameter modes, the Provider signature, and
  `mut` write-back all index by. A checked argument with no parameter index,
  or two arguments naming one slot, is refused at lowering rather than placed
  by guesswork.
- Fixed-arity intrinsics go through `lower_builtin_operands_as`, which states
  each operand's lowering shape (value, `mut` place, retained value) by
  parameter, so no intrinsic case depends on the order the caller wrote.
- A closure value's type `Fn(A, B) -> R` has no parameter names, so its
  arguments are positional and a label on one is `RS0203`
  (`callbacks.rs::callback_call_label_diagnostic`). Lowering refuses a labelled
  closure argument as a second line of defence.

Non-goals: the evaluation order itself is unchanged, and so are `CallBinding`,
the checker's binding rules (named, same-name, positional), and the MIR,
bytecode, and Artifact formats. No new instruction carries a parameter index;
the argument list's order is the binding.

## Compatibility and migration

This is a language-semantics correction, not a new contract: the specification
always said an argument reaches the parameter it names.

**Previously compiled Artifacts that contain a call with labelled (or
same-name) arguments written out of declaration order, or a call that omits a
defaulted parameter declared before a supplied one, computed wrong results.**
The Artifact records the arguments in the order the old lowering emitted them,
so such an Artifact keeps computing the wrong result until it is rebuilt from
source; rebuilding with this compiler produces the declared binding. There is
no format change to detect, so a host that cannot rebuild should treat any
Artifact built before this change from source containing such calls as
suspect. A source scan of this repository's examples, fixtures, eval reference
solutions, packages, stdlib, and documentation examples found no affected call
that checked clean; the only hits were model-written eval samples calling
`Json.field(name: …, value: …)`, whose mismatched types failed loudly.

Source that labelled a closure-value call now fails with `RS0203`; remove the
label and write the arguments in the closure's parameter order. No checked-in
source did this.

Rollback is a revert, which reintroduces the miscompile.

## Verifier and security impact

None to the verifier: argument lists were and remain positional. Verification
had been rejecting some wrongly bound calls (mismatched register types); those
programs now verify because their bindings are right.

## Provider and backend impact

None to the Provider ABI, whose arguments and `mut` write-backs were always in
declaration order; lowering now supplies that order. The VM and the native
engine consume the same MIR and bytecode, so both are corrected by the
lowering change alone.

## Evidence

- `crates/rsscript-sdk/tests/argument_order.rs`: for a representative callee of
  every call kind — private and `pub` user functions, a defaulted parameter
  omitted and supplied, a `mut` parameter, a generic instance, a qualified
  method call with the receiver permuted, receiver-call sugar on a user method
  and on a pure and a mutating builtin, record and variant constructors,
  special intrinsics (`String.concat`, `List.get`, `List.set`, `Map.insert`,
  `List.map`), catalog builtins (`String.replace`, `String.pad_left`,
  `List.slice`, `Math.clamp`), `Dyn<P>` and bounded static protocol dispatch,
  `async let`, a `select` arm, and external Provider calls with and without a
  `mut` write-back — every permutation of the argument order is run, and the
  result must equal the declaration-order call's and the arguments' side
  effects must occur in the written order. Before this change it failed for
  every kind except the two constructors. A further test covers same-name
  arguments written out of order.
- `native_jit_differential.rs`: the `labelled-argument-order.rss` corpus case
  and `labelled_argument_order_runs_natively_with_the_declared_binding`, which
  pins the value both engines compute.
- `fail/closure-call-labelled-argument.rss` (`RS0203`).
