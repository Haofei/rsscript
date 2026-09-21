# ADR 0239: The remaining closure combinators are lowered, and a new builtin is appended

- Status: Accepted
- Date: 2026-09-20

## Problem

ADR 0237 closed six closure-taking combinators and left three programs that
`rss check` accepts and the backend refuses. A check/build split is the worst
failure shape this project has: the checker is the contract models are told to
trust, and a clean check that dies in lowering teaches them to distrust it.

**`List.sort` had an opcode and no MIR case.** It is declared
`lowering = "special"` in `crates/rsscript-compiler/intrinsics.toml`, which
hands it to the checked-HIR-to-MIR lowerer, and the lowerer had no case for it.
The register VM's `ListSort` opcode and the v1 bytecode operand table already
existed. It is the one `special` list combinator that takes no callback at all:
an `Ord`-bounded mutation of a list place, structurally `ListSortWith` without
the comparator.

**`Pipeline.map` and `Pipeline.filter` had neither.** ADR 0237 recorded them as
deferred, with this reasoning: closing them means adding catalog entries, and a
new catalog entry "would renumber every later `BuiltinId` — a separate change
with a real compatibility cost."

That reasoning was about where the entry goes, not whether one can be added.

## Decision and non-goals

**A `BuiltinId` is a position, so a new direct binding is appended.**
`write_mir_builtin_catalog` filters `intrinsics.toml`'s `binding` table to
`lowering = "direct"`, in file order, and each binding's `BuiltinId` is its
index in that filtered list. Filing a new direct binding alphabetically —
`Pipeline.filter` and `Pipeline.map` belong at line 702 — inserts it in the
middle and shifts every direct binding after it. Appending it to the end of the
table shifts nothing.

So the two bindings are appended, out of alphabetical order, with a comment at
the append point saying why they are there. Measured over the generated
catalog: 400 direct bindings before, 402 after, **zero** existing ids moved,
`Pipeline.filter` and `Pipeline.map` taking 400 and 401.

The compatibility cost ADR 0237 priced was therefore avoidable entirely, and no
release-versus-artifact trade-off had to be made. `BUILTIN_REGISTRY_DIGEST`
still changes, because two entries joined the registry; that is the digest
doing its job, and no persisted artifact predates it — this project has no
tagged release and every artifact is rebuilt from source.

**Lowering.**

1. `MirInstruction::ListSort { destination, list: PlaceId }`, with a lowerer
   case, a `rsscript-codegen-vm` emitter onto the existing `ListSort` opcode,
   MIR verifier definition/use/liveness rules (the receiver is a mutable place
   and there is no value operand at all), and a real conformance-oracle arm.
   Unlike the six in ADR 0237 it enters no callback body, so the oracle
   executes it rather than declining: it sorts the scalar element kinds it can
   compare and reports `InvalidOperation` for anything else, including a
   `Float` list holding a NaN. An ordering the oracle invented would be an
   oracle answer with nothing behind it.
2. `Pipeline.filter` and `Pipeline.map` become `lowering = "direct"` with the
   new `PipelineFilter` and `PipelineMap` VM ids, and are implemented in
   `reg_vm/intrinsics` the way `Pipeline.each` and `Pipeline.try_map` beside
   them already are: a `Pipeline<T>` is its backing list, and both stages walk
   it eagerly. `direct` is what their four sibling `Pipeline` and
   `FalliblePipeline` stages already use, so this removes an inconsistency
   rather than introducing one, and it needs no new opcode — a direct builtin
   travels through the existing `CallIntrinsic` path, so the JIT, the native
   translator and the bytecode opcode table are untouched.

Non-goals. No diagnostic code is added or retired, no program's accept/reject
outcome changes, and no `special` binding other than `List.sort` gains a MIR
case. `Pipeline` remains eager; making it lazy is a language question, not a
lowering one.

## Compatibility and migration

MIR gains one instruction variant. `MirInstruction` is internal to this
workspace and reconstructed from source on every build; no MIR is persisted.

The builtin registry gains two entries at the end. Every existing `BuiltinId`
keeps its value, which is the point of appending, so a MIR module or bytecode
unit built before this change names the same builtins after it.
`BUILTIN_REGISTRY_DIGEST` changes, as it must when the registry does.

The emitted v1 bytecode uses opcodes, operand names and register conventions
the encoder and register VM already accepted: `ListSort` for the sort, and
`CallIntrinsic` for the two pipeline stages.

Rollback is a revert. A reverted `Pipeline` binding returns to
`lowering = "special"` and its programs return to being unbuildable, which is
the state this ADR replaced.

## Verifier and security impact

None relaxed. `ListSort` gets `ListPush`'s liveness check on its receiver place
and an ordinary definition rule on its destination. The two pipeline stages are
ordinary direct builtin calls whose closure argument's ABI is still proved by
the `MakeClosure` contract.

## Provider and backend impact

Register VM only. `RegIntrinsic::PipelineMap` and `RegIntrinsic::PipelineFilter`
are generated from the catalog and implemented beside their siblings. No
Provider ABI, no native/JIT translation arm — a direct builtin is dispatched
through `CallIntrinsic`, which the native tier already handles.

## Evidence

- `rsscript-sdk`: `closure_taking_combinator_programs_verify_and_run` gains
  `List.sort` and a `Pipeline.map`/`Pipeline.filter` stage pair, each built,
  verified and run for its value.
- `rsscript-sdk`: `fixture_build_corpus` over three new `pass` fixtures with a
  `main` — `list-sort-closure-free.rss`, `pipeline-map-closure.rss` and
  `pipeline-filter-closure.rss` — one per function, so a regression in one
  cannot hide behind another.
- The `BuiltinId` stability claim is checkable directly: the generated
  `rss-mir-builtin-catalog.rs` before and after this change agree on every
  namespace/name to id mapping that existed before it.
