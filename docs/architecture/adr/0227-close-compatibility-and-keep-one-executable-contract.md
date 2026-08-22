# ADR 0227: Close compatibility surfaces and keep one executable contract

## Status

Accepted

## Problem

The architecture migration had completed, but Core still contained inactive
SDK and Provider compatibility code, a verifier-only bytecode v2 prototype,
and workflow references to targets that no longer existed. At the same time,
the optional native JIT provides a measured hot-loop benefit and deleting it
would remove useful trusted-host performance without simplifying the canonical
Artifact or Provider contracts.

## Decision

Core has one Provider value model (`WireValue`) and one executable Artifact
contract (`rsscript.bytecode.v1`). The inactive SDK compatibility façade,
dynamic Provider callables, compiler package/review adapters, and the
verifier-only bytecode v2 prototype are deleted. The native plugin ABI remains
archived research source outside every active workspace.

The Cranelift native JIT is retained as an explicit, trusted-in-process engine:

- default builds and the isolated runner use the verified interpreter;
- callers select native execution with typed `NativeJitOptions`;
- engine selection never removes or widens execution limits;
- reports record typed native/interpreter evidence;
- the shipped CLI exposes native execution only behind the explicit
  `--trusted-in-process --native` combination;
- Core accepts and verifies the same bytecode regardless of engine.

CI validates workflow package, feature, and test names against Cargo metadata.
Because the SDK disables automatic test discovery, every top-level SDK test
source must also be an explicit Cargo target.

## Consequences

There is no dormant second bytecode contract or dynamic Provider ABI in Core.
A future bytecode replacement requires a new cutover ADR, a complete emitter,
verifier, decoder and VM path, and an explicit v1 reader lifetime. Native JIT
correctness and performance remain opt-in gates rather than reasons to leak JIT
state into stable SDK types.
