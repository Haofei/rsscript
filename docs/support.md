# Support And Deployment Policy

Effective 2026-07-29. This document defines what the project maintains, what it
only experiments with, and where its security boundary ends. A feature's support
level and its deployment profile are separate decisions.

## Support Levels

### Core

`Core` is the maintained product surface. It is enabled in the default feature
graph, documented as current behavior, and validated on every pull request.
Regressions in Core, its supply chain audit, or its security boundary tests block
merge and release.

Core currently includes:

- static frontend checks, formatting, linting, diagnostics, and source maps;
- package review, semantic diff/lock metadata, REIR collection,
  reconciliation, reports, and fail-closed policy decisions;
- Rust lowering and the default register-VM interpreter path for trusted input;
- default runtime budgets, process containment reporting, native authorization,
  and portable platform-policy tests;
- the protected-base behavior and failure propagation of the review action.

Core means maintained within the documented `0.1.x` prototype contract. It does
not make unstable schemas stable, turn declared capabilities into independently
verified facts, or make the runtime a sandbox.

### Experimental

`Experimental` is opt-in, narrower than the reference semantics, or dependent on
specialized hardware/toolchains. It may change without compatibility guarantees.
Matching code paths run dedicated CI on pull requests and pushes; the complete
experimental matrix also runs nightly and for releases. A matching pull-request
failure and every release validation failure are blocking. A nightly failure is
triaged but does not retroactively change a published Core claim.

Experimental currently includes:

- the off-by-default Cranelift `native-jit` feature and JIT performance/hardening
  sweeps;
- self-hosting parity and exhaustive corpus work;
- using the `0.1.x` GitHub Action or REIR schemas as a production authorization
  control without an independent audit.

Promotion to Core requires a documented compatibility contract, inclusion in the
default supported surface, per-PR coverage on supported platforms, and closure
of threat-model debt relevant to the promoted feature.

### Trusted-only execution

`Trusted-only` is a security qualifier, not a lower maturity tier. It applies
whenever an operation executes, compiles, or loads input. Core status does not
override this label.

The following are trusted-only:

- generated Cargo builds, build scripts, and executable package dependencies;
- in-process native plugins or native wrappers;
- in-process tier-0/native JIT execution;
- host filesystem, environment, network, process, database, or device access;
- execution based only on RSScript capabilities, VM budgets, process limits, a
  container, or the review action.

Static inspection is the only project-supported operation for third-party
source: do not build dependencies, run hooks, load native code, execute shaders,
or provide ambient credentials. RSScript and REIR evidence do not authorize
execution and do not enforce OS isolation.

## Deployment Profiles

| Profile | Allowed input and operations | Required controls | Project status |
| --- | --- | --- | --- |
| `LocalTrusted` | Source and dependencies controlled by the developer; Core execution and explicitly enabled Experimental paths | Review native/build-script changes; acknowledge trusted native execution; keep normal resource budgets unless deliberately debugging | Supported for development, not an adversarial boundary |
| `TrustedCI` | Reviewed organization repositories in disposable CI; Core gates and dedicated Experimental jobs | Immutable action/tool pins, locked dependencies, least-privilege token, no secrets for fork PR code, protected-base policy, isolated ephemeral runner, explicit native/JIT jobs | Supported for CI experiments; not a production authorization system |

The `rss run --deployment-profile` spellings are `local-trusted` and
`trusted-ci`. `TrustedCI` may run bounded pure code in the reference VM. That
path carries an explicit deny-all host context and rejects every host-touching
intrinsic before dispatch. It does not permit AOT, native, JIT, process,
network, database, environment, or filesystem effects. This is a controlled-CI
convenience for repositories the operator already trusts, not an untrusted-code
execution boundary.

RSScript has no `UntrustedIsolated` profile, worker protocol, sandbox launcher,
or third-party execution API. Projects that need hostile-code execution must
provide and audit a separate system outside this repository.

## CI Contract

| Layer | Trigger | Blocking contract |
| --- | --- | --- |
| Core | Every pull request and push to `main` | Locked full manifest, supply-chain audit, Windows/macOS containment and native authorization, review-action smoke, and other always-on Core workflows |
| Security-sensitive | Pull requests and pushes touching boundary paths; manual | Deployment-policy tests, unsafe-boundary Clippy, JIT differential safety, native ABI, process containment, runtime, REIR/LSP, and database boundary tests |
| Experimental | Matching JIT paths, nightly, manual | Full native-JIT suite on Linux |
| JIT performance/hardening | Matching JIT performance paths or scheduled/manual hardening | Performance regression gate, sanitizer/Miri/fuzz sweeps according to the dedicated workflow |
| Release | Version tag or manual release | Core validation plus native JIT, generated backend parity, and self-host corpus before artifact promotion |

Path filtering is a scheduling optimization, not a security exemption.
Experimental code under a security-sensitive path must pass both layers. Release
artifacts are still built once after the locked validation and promoted without
rebuilding.
