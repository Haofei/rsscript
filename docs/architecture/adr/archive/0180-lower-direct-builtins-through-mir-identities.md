# ADR 0180: Lower direct builtins through MIR identities

- Status: Accepted
- Date: 2026-08-14

## Problem

MIR already defined `BuiltinId`, but direct checked-HIR builtin calls could not
use it. The lowerer rejected ordinary core-library calls and the default
pipeline fell back to the legacy source-shaped executable IR. This left source
callee spelling in the effective backend boundary and prevented the primary
embedding example from using even a simple deterministic builtin.

## Decision and non-goals

The intrinsic catalog now generates a shared direct-builtin lookup for MIR.
Checked semantic namespace/name resolution becomes `MirCallTarget::Builtin`,
which carries only `BuiltinId`. `rsscript-codegen-vm` is the sole place that
projects that identity back to a v1 `CallIntrinsic` string, because v1 bytecode
still has a string intrinsic field.

Result and Option constructors remain dedicated MIR operations. Receiver
syntax, generic/typed intrinsics, async builtins, a complete builtin signature
table, and a numeric bytecode-v2 intrinsic encoding are outside this change.

## Compatibility and migration

Existing v1 artifacts and VM dispatch retain their intrinsic spellings. Newly
compiled direct-builtin source produces the same v1 opcode spelling, but no
longer requires the legacy executable-IR lowering path. The generated mapping
is derived from the checked-in intrinsic catalog, so catalog changes remain
reviewable and deterministic.

## Verifier and security impact

MIR validation rejects unknown builtin IDs. No Provider linkage, execution
budget, resource, cancellation, or isolation policy changes. The v1 bytecode
verifier continues to own untrusted-byte validation; this change only removes
source-name reconstruction before that boundary.

## Provider and backend impact

Providers are unaffected. The reference VM receives its existing
`CallIntrinsic` representation. Future backends consume `BuiltinId` from MIR
and must not inspect syntax callee text.

## Evidence

- direct-HIR builtin → verified-bytecode → VM migration regression
- `embedded-report-pipeline` end-to-end execution
- MIR/lowering/codegen focused suites and default workspace tests
