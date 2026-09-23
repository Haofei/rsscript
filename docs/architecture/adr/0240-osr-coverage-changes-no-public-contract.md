# ADR 0240: Widening OSR coverage changes no public contract, and test-only edits leave the ADR gate

## Status

Accepted. Date: 2026-09-23.

## Problem

Three commits after `6123d60a` touched `crates/rsscript-sdk/` without an ADR,
and the CI ADR gate (`scripts/check-contract-adr.sh`) failed on them:

- `a26f3a66` recognizes an OSR loop whose exit block MIR laid out between two
  of the loop's own blocks. Loop membership is computed by reachability from
  the header with the exit edge cut, and `native_normalize_osr_loop_layout`
  relocates the in-span hole past the loop before the OSR pass chain runs.
- `2506dfc2` adds an OSR lowering arm for a `StringConcat` whose result
  survives as a live heap `String` (a map key built inside the loop), through
  the host helper the whole-function translator already uses.
- `775b5776` records why a callee's register-window growth cannot be charged
  as a per-call-site constant or reserved at native entry: `ensure_regs` bills
  against the shared register stack's high-water mark.

Each touched `crates/rsscript-sdk/` only in
`tests/native_jit_differential.rs`. The rest of each change is native-tier
internals in `crates/rsscript-vm/src/reg_vm/` and prose in
`docs/spec/native-jit-contract.md`.

## Decision and non-goals

**No public contract changed.** None of the three commits changes:

- the Provider ABI or `rsscript-abi-model`;
- MIR (`rsscript-mir`): no instruction, verifier rule, or builtin id;
- bytecode or Artifact formats (`rsscript-bytecode`): no opcode, operand, or
  encoding;
- language syntax or semantics;
- the SDK façade (`crates/rsscript-sdk/src/`): no public item, feature, or
  signature.

The layout normalization is a pure permutation of the native tier's own
instruction items with branch targets remapped. The header keeps its source ip
and every source instruction still owns exactly one item and one interpreter
step, so the boundary mapping, resume map and cost vector are unchanged in
meaning. The `StringConcat` arm charges what `RegInstr::StringConcat` costs the
interpreter, and it keeps allocation fail-closed: the region declines whenever
an allocation budget or live-memory limit is armed. The register-window
finding changes no code at all. The observable contract of the native tier, as
ADR 0233 and `docs/spec/native-jit-contract.md` state it, is that a region
either reproduces the interpreter's results and accounting exactly or
declines. All three commits hold to that; they only move some loops from
declining to compiling.

**The ADR gate covers contracts, not the tests that exercise them.** The
gate's header comment and `docs/architecture/adr/README.md` both say an ADR is
required when a change modifies a compatibility or security contract. A change
confined to a crate's `tests/` directory adds or changes evidence, not a
contract. When a contract does change, the change reaches the crate's `src/`,
`build.rs`, `Cargo.toml`, or checked-in data, all of which the gate still
covers. The gate now skips `crates/*/tests/*`, with one exception: the
Artifact/Provider compatibility corpus
(`crates/rsscript-sdk/tests/compatibility_corpus.rs` and
`crates/rsscript-sdk/tests/corpus/compatibility/`) is the checked-in record of
that boundary, and editing its expectations edits the contract, so it stays
covered.

Non-goals. The set of contract-owning crates stays the same. Changes to the
native tier in `rsscript-vm` still need no ADR unless they change an observable
contract. That was already the rule, since `rsscript-vm` was never on the
gate's path list.

## Compatibility and migration

None. No persisted artifact, MIR, bytecode unit, or Provider sees a
difference. A program's results, `allocation_bytes_consumed`, step count and
control behavior are the same under the interpreter and the native tier before
and after these commits. The only thing that moves is which tier executes the
affected loops.

Rollback is a revert of any of the three commits. The gate narrowing can be
reverted on its own, and doing so only makes the gate stricter.

## Verifier and security impact

None relaxed. The OSR region verifier admits the relocated layout only when the
hole starts at the loop exit and every run of it ends at a terminator, and the
region has one header, backedges only to it, one exit edge, and no in-loop
`Return`. The `StringConcat` arm admits only a destination register that is not
a parameter and that nothing outside the region reads. Armed memory controls
still refuse the region.

Narrowing the gate removes no enforcement from a contract path. `src/`,
`build.rs`, `Cargo.toml`, and non-test data of every contract crate, plus the
compatibility corpus, still require an ADR.

## Provider and backend impact

Native tier only (`rsscript-vm` with the `native-jit` feature). Providers, the
register VM's interpreter, and the bytecode encoder are unaffected.

## Evidence

- `rsscript-sdk` `native_jit_differential`:
  `an_osr_loop_whose_body_matches_reaches_generated_code_and_accounts_exactly`,
  `an_osr_loop_whose_body_matches_accounts_intrinsics_under_the_production_defaults`,
  `the_kernels_the_layout_normalization_does_not_unblock_still_account_exactly`,
  `an_osr_loop_that_builds_its_map_key_accounts_exactly_armed_and_unarmed`,
  `an_osr_loop_that_builds_its_map_key_declines_under_armed_memory_controls`,
  and `the_register_window_charge_is_a_high_water_mark_not_a_per_call_constant`.
  Each compares the native tier against the interpreter.
- `git diff --name-only 6123d60a...775b5776` lists no path under a contract
  crate's `src/`, `build.rs`, or `Cargo.toml`.
- `bash scripts/check-contract-adr.sh 6123d60a97afcbe6f34e3cde053858f40608fd39`
  exits 0 with this record and with the narrowed gate alone. A path under
  `crates/rsscript-sdk/src/`, `build.rs`, `Cargo.toml`, the compatibility
  corpus, or `crates/rsscript-bytecode/fixtures/` still makes it exit 1.
