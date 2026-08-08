# Architecture migration baseline

This document freezes the behavior and dependency baseline used while RSScript
moves semantic ownership, executable IR, bytecode code generation, and VM
responsibilities. It is a migration guardrail, not a release plan.

## Scope freeze

Until the migration exit criteria below are met, Core accepts correctness,
security-boundary, conformance, diagnostics, and measured-regression changes.
Core does not add language syntax, qualifiers, public intrinsics, official
Providers, JIT tiers or speculation, AOT/native surfaces, package publishing,
language-level policy, or a registry.

The authoritative package maturity inventory is
[`workspace-tiers.toml`](workspace-tiers.toml). Architecture tests require every
workspace package to occur in exactly one tier and require only Core,
applications, and the reference runner to be root default members.

## Migration invariants

The following are mechanical exit conditions, not architectural aspirations:

1. Syntax does not depend on semantics, runtime, Providers, or review.
2. Semantic validation does not depend on a runtime, concrete Provider, review,
   JIT, or AOT implementation.
3. HIR remains source-shaped; the future MIR is typed, owned, CFG-shaped, has no
   syntax dependency, has no unresolved symbol identity, and has no `Unknown`
   execution node.
4. Compiler code generation does not depend on the VM interpreter.
5. The VM accepts only a verifier-created program and does not depend on syntax,
   HIR, semantic databases, package loading, or compiler orchestration.
6. The SDK exposes an explicit reviewed façade; it must not acquire new root
   glob exports from implementation crates.
7. Provider replacement cannot alter compiled Artifact bytes.
8. Analysis, Artifact, and semantic diff carry the same snapshot/module
   identity.
9. Existing and replacement execution paths remain differential-tested until
   the old path is deleted.
10. Experiments consume stable Core contracts and cannot add state to Core VM
    program types.

## Behavior preservation baseline

| Contract | Existing guard | Migration rule |
| --- | --- | --- |
| Source diagnostics | `static`, semantic property, hostile and fuzz corpora | Diagnostic code/span digest changes require an intentional fixture update |
| Source to Artifact | schema contracts and `migration_baseline` | Canonical bundle digest changes require an intentional fixture update |
| Artifact verification | bytecode properties, malformed corpus and fuzz targets | Unverified bytes never enter execution |
| VM behavior | runtime, VM parity, differential and soak suites | New and old lowering paths must produce equivalent reports |
| Cancellation and budgets | hostile, JIT acceptance, runtime and Core metrics | Termination reason and cleanup behavior cannot regress |
| Provider boundary | Provider conformance and replacement demo | Signature mismatch fails before execution |
| Runtime telemetry | execution report schema and Core metrics | Telemetry remains observational and redacted by policy |
| Determinism | package/schema tests and canonical Artifact encoding | Same snapshot must produce byte-identical bundle bytes |

`benchmarks/core/slo.v1.json` remains the performance regression envelope. It is
not a release gate and does not justify JIT expansion; it protects check,
compile, verify, execute, Provider-call, cancellation, and Artifact-size
baselines during internal refactoring.

## Current asset ownership

| Asset | Current owner | Migration disposition |
| --- | --- | --- |
| Parser/CST/AST | `rsscript-syntax` | Keep |
| Immutable snapshots, semantic database and validation phase types | `rsscript-semantics` | Migrated; compiler only assembles them through the analyzer boundary |
| Analyzer orchestration and most checks | `rsscript-compiler` | Move remaining semantic checks and queries to `rsscript-semantics` |
| Typed HIR model | `rsscript-semantics` | Keep source-shaped |
| Owned executable IR | `rsscript-exec-ir` | Transitional; replace source-shaped nodes with typed CFG MIR |
| HIR projection | `rsscript-lowering` | Evolve into HIR-to-MIR lowering |
| VM bytecode emission | `rsscript-vm` | Move to a codegen boundary after MIR exists |
| Artifact envelope/verifier | `rsscript-bytecode` | Keep; evolve through a versioned typed wire model |
| Interpreter, limits, scheduler | `rsscript-vm` | Keep only verified execution responsibilities |
| Dynamic Provider ABI/linking | `rsscript-provider-api` | Keep; tighten wire values and resource handles later |
| Stable embedding path | `rsscript-sdk` | Shrink to explicit phase APIs before public compatibility promises |
| Package capture and persistence | `rsscript-compiler` plus workspace loader | Move OS/persistence concerns out of compiler core |
| AOT/JIT/native/REIR/selfhost | Experimental/Integration/Research tiers | Frozen except correctness and differential value |

## Ordered migration

1. Preserve diagnostics, Artifact, execution, cancellation, and Provider behavior.
2. Move semantic ownership behind one query/session boundary.
3. Introduce typed CFG MIR beside the existing executable IR.
4. Differential-test both lowering paths and remove the old path only after
   parity.
5. Move MIR-to-bytecode emission out of the VM and require verified-only VM
   construction.
6. Split VM primitives, deterministic core library calls, and Provider calls.
7. Design bytecode v2 only after MIR identities and instruction semantics settle.

## Exit criteria for this preparation phase

- Root default commands select only Core, applications, and the runner.
- Full `--workspace --all-features` maintenance tests remain available.
- CI has separate Core and experimental workflows.
- Workspace classification and dependency direction are machine checked.
- A canonical compilation/diagnostic baseline is checked in.
- New disabled `#[cfg(any())]` cemetery code is rejected.
- Scope freeze and migration ownership are visible from the roadmap.
