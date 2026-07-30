# Scope Remediation Ledger - 2026-07-29

## Scope

This ledger records the CI/documentation product contraction audited from
`3ab65da5`. The scope-owned changes are documentation and workflow scheduling
only. Concurrent Rust work added CLI profile enforcement and is recorded here as
current-state evidence, not as work completed by this ledger.

Status meanings:

- `COMPLETE`: represented by documentation or executable CI configuration in
  this change.
- `FROZEN`: explicitly outside the current product claim; work must not expand
  the claim without a promotion decision.
- `OPEN-DEBT`: required for a stronger threat model or maturity claim and not
  represented as complete.

## Complete

| Item | Evidence |
| --- | --- |
| One support vocabulary | `docs/support.md` defines `Core`, `Experimental`, and the orthogonal `Unsupported-for-untrusted` security qualifier. |
| Deployment profiles | `LocalTrusted`, `TrustedCI`, and `UntrustedIsolated` define allowed input, controls, and current implementation status. The CLI executes only under `LocalTrusted`; the other profiles fail closed except for non-executing lowering. |
| Core on every PR | `ci.yml` retains the locked full manifest, Cargo audit, and Windows/macOS containment/native authorization on every pull request and `main` push. |
| Experimental scheduling | `experimental.yml` runs full native-JIT and real-device Metal suites for matching paths, nightly, and manually. Existing JIT performance and weekly hardening workflows remain dedicated. |
| Release coverage | `release.yml` retains native-JIT validation and now requires macOS real-device Metal validation before artifact promotion. |
| Security gates preserved | `security-sensitive.yml` still runs unsafe-boundary Clippy, JIT differential safety, native ABI, process containment, runtime, REIR/LSP, and database tests. It now covers deployment-policy entry points and self-audits changes to `experimental.yml`. |
| User-facing scope aligned | README, docs index, and development guidance point to one binding support/deployment policy and label native JIT Experimental. |

## Frozen

| Claim or investment | Frozen disposition |
| --- | --- |
| Untrusted or multi-tenant execution | Do not claim or enable it. Static inspection is the only supported operation for attacker-controlled source. |
| In-process hostile native/JIT/GPU | Do not treat native authorization, VM budgets, process limits, or shader digest policy as sandboxing. |
| JIT and Metal promotion | Keep Experimental until their compatibility contract, default surface, platform coverage, and relevant isolation debt meet the promotion rule. |
| Self-hosting as product requirement | Keep as Experimental parity/pressure testing; it is not a v0.7 or Core delivery requirement. |
| Production authorization claim | Do not present `0.1.x` action output, capability declarations, or unstable REIR schemas as sufficient deployment authorization. |
| New product surface | Prefer correctness, evidence quality, and boundary closure on the declared Core over adding language/runtime/backend scope. |

## Open Debt

| Debt | Why it remains open | Promotion/blocking effect |
| --- | --- | --- |
| Immutable snapshot-first package review | Authorization can still precede capture of the complete package/dependency closure. | Blocks hostile package build/execution claims. |
| Out-of-process hostile workers | Native plugins, JIT, GPU shaders, and hostile children lack one mandatory killable OS-isolated worker model. | Blocks `UntrustedIsolated` execution and multi-tenant claims. |
| End-to-end profile enforcement | `rss run` enforces the profile matrix, but embedding/runtime APIs do not yet require one profile context across every host capability. | Blocks claiming runtime-wide profile enforcement or sandboxing. |
| Sealed validated execution plan and metered JIT types | Trust still depends partly on caller/API convention. | Blocks hostile JIT promotion. |
| Windows secure native path | SID/DACL artifact verification and suspended create/assign/resume process launch remain absent. | Blocks hostile native/process execution on Windows. |
| Independent capability verification | Package bindings are author declarations unless separately verified. | Blocks production authorization claims. |
| Stable schemas and public API | RSScript/REIR remain `0.1.x`; artifact schemas are unstable unless explicitly marked. | Blocks compatibility guarantees. |
| External integration runners | Live PostgreSQL remains environment-gated; broader release artifacts/platform support are not provided. | Limits release evidence to the documented runner matrix. |
| Structural/compiler maintainability work | Typed semantic database, structural generic substitution, dependency-injected execution context, and module decomposition remain accepted architecture debt. | Does not block current trusted Core claims; required before mature platform claims. |

## Exit Rules

A frozen item moves only through an explicit support-policy update that names its
threat model, default state, compatibility promise, CI coverage, and rollback
plan. Closing an open debt requires executable enforcement and regression
evidence; documentation or caller convention alone is not closure.

## Verification

Verification for this ledger is recorded against the final diff:

- all scope-owned modified paths are under `README.md`, `docs/**`, or
  `.github/workflows/**`;
- all workflow YAML files parse successfully;
- workflow trigger/job assertions confirm Core per-PR, Experimental
  path/nightly/manual, and release JIT/Metal coverage;
- Markdown links resolve locally;
- `git diff --check` passes.
