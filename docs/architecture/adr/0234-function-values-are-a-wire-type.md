# ADR 0234: Represent a function value as a wire type so callbacks can execute

- Status: Accepted
- Date: 2026-09-15

## Problem

`noescape Fn(...)` callbacks are a headline RSScript feature: the README, the
language specification, and the `pass` fixture corpus all show a user function
that takes one. None of those programs could run.

`fn apply(values: read List<Int>, f: noescape Fn(Int) -> Int) -> fresh List<Int>`
checked clean and then died in checked-HIR-to-MIR lowering with

```
cannot lower `apply` to the typed MIR control-flow subset:
function type in direct MIR signature
```

Closure *values* already lowered and ran: `local f = |x| { ... }` became
`MakeClosure`, `f(i)` became `CallClosure`, and a closure handed to a core
intrinsic such as `List.map` travelled through the intrinsic path. The missing
piece was the ABI position. `WireType` — the canonical, serializable type model
shared by Artifact imports, MIR's interned type table, and the typed executable
facts — had no form for a function value, so `checked_type_to_wire` failed
closed on `ResolvedTypeKind::Function` rather than inventing a synthetic named
type. Every construct downstream of that refusal (the parameter's MIR `TypeId`,
the argument at the call site, the `CallClosure` inside the callee) was
therefore unreachable.

Failing closed was right; leaving the type model without the form was the gap.
A callback parameter is not an unproved position: `noescape Fn(read String, Int)
-> String` writes down the arity, each parameter's type and data effect, and the
result. Projecting all of that as `Unknown`, or as an opaque handle, would
discard facts the checker proved.

## Decision and non-goals

`WireType` gains one variant:

```rust
Function {
    parameters: Vec<WireType>,
    parameter_effects: Vec<DataEffect>,
    result: Box<WireType>,
}
```

It is a real type, not a placeholder. Arity, per-position data effects, and the
result are carried structurally, so a consumer reading a signature knows how
many arguments a `CallClosure` through that position passes and how each one is
borrowed. `WireType::parse` accepts the canonical `Fn(...) -> T` spelling,
including effect prefixes (`Fn(mut List<Int>)`), and an omitted result reads as
`Unit`, matching the source language.

`WireType::is_resolved` recurses into a function type. An unannotated closure
parameter still carries the checker's unresolved marker, so a function type
built from one remains unresolved and the typed executable facts continue to
report it `Unknown` (ADR 0228's rule: `Unknown` is a valid proof result and is
never replaced by an optimistic default). A *declared* contract is resolved and
is published as `Known`.

With the type in place, lowering treats a function-typed parameter as an
ordinary MIR ABI position:

- `checked_type_to_wire` maps `ResolvedTypeKind::Function` onto the new variant,
  so the parameter interns a `TypeId` like any other.
- The callee registers the parameter's callable contract — the same
  `parameter_types`/`parameter_modes` pair a `MakeClosure` publishes — so `f(x)`
  inside the body lowers to `CallClosure` on the parameter register. The same
  projection runs for a closure that itself declares a callback parameter.
- A binding whose own inferred type is `Fn(...)` carries that contract too, not
  only an inline closure literal, so a callback obtained from a call or a field
  read is callable through the binding.
- Call-site argument lowering is unchanged: an inline `|x| { ... }` is an
  ordinary value argument produced by `MakeClosure`, and a named user `fn`
  reaches lowering already desugared into one.

Non-goals. `noescape` enforcement stays with the checker (the RS0803 family);
lowering only passes the value through. Calling a callback directly through a
struct field (`holder.f(1)`) remains unresolved at `rss check` time (`RS0206`),
and a `noescape` qualifier nested inside an `Fn(...)` parameter list remains a
parse error (`RS0015`) — both are checker-reported, not build failures. Provider
interfaces still cannot declare a function type: a `.rssi` has no syntax for one,
and generated Rust falls back to the opaque wire value rather than inventing a
callable the host could not invoke.

## Compatibility and migration

Additive. `WireType` is a `kind`-tagged serde enum, so the new `"function"` tag
appears only in artifacts that actually use a function type; every previously
emitted artifact decodes and re-encodes byte-for-byte as before. No existing
program changes shape, and no bytecode ISA, runtime ABI, or typed-facts schema
version moves.

`DataEffect` gains `PartialOrd`, `Ord`, and `Hash` so that `WireType` keeps its
existing derives. That is additive on a `Copy` fieldless enum.

`fixtures/pass/noescape-callback-manage-local.rss` leaves the
`CANNOT_BUILD_YET` allowlist in `fixture_build_corpus`, which is the acceptance
test for closing this gap.

## Verifier and security impact

Neither verifier is weakened. The MIR verifier sees a parameter type like any
other interned `TypeId`. The bytecode typed-facts verifier compares a callback
parameter register against the executable signature structurally: function types
unify position-wise, and the per-position effects must agree, so a signature
that claims a different arity or a different borrow mode is rejected rather than
tolerated. Where the checker proves nothing, the facts still say `Unknown`,
which stays compatible with any argument register instead of asserting a layout
that does not exist.

The native tier fails closed. A `CallClosure` whose closure operand is a
parameter has no in-region `MakeClosure` to prove which body it enters, so the
inlining candidate never forms and the region is declined: no compile, no OSR
entry, no continuation entry. Outcome and `steps_consumed` match the interpreter
at every step boundary.

## Provider and backend impact

- `rsscript-provider-bindgen` and `rsscript-provider-conformance` handle the new
  variant explicitly, as an opaque wire value, because a Provider interface
  cannot declare one.
- The register VM's native fact projection classifies a function value as a
  handle, which is what `VmValue::Closure` is.
- The legacy signature spelling and the JSON codegen path render the canonical
  `Fn(...) -> T` text derived from the type table, never from a debug
  representation.

## Evidence

- `crates/rsscript-lowering/src/mir.rs`: a declared contract lowers to the wire
  function type with its effects; an unproved parameter keeps the whole type
  unresolved.
- `crates/rsscript-sdk/src/tests.rs`: build → verify → run for a callback called
  in a loop, a two-parameter callback taking a `read String`, a callback
  capturing a caller local, a named user `fn` passed as the callback, nested and
  forwarded callback parameters, and a returned callback; the typed facts
  publish the declared contract and its `ReadBorrow` ownership; calling a
  callback struct field is `RS0206`.
- `crates/rsscript-sdk/tests/fixtures/pass/`: `callback-parameter-*`,
  `callback-captures-a-caller-local`, `named-function-as-callback`, and
  `nested-callback-parameters`, all exercised by `fixture_corpus` and
  `fixture_build_corpus`.
- `crates/rsscript-sdk/tests/native_jit_differential.rs`: a callback-parameter
  loop pins interpreter/native parity of outcome and `steps_consumed` across
  every step budget, and pins that the region is declined.
