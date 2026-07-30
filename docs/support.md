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
- Metal policy plus real-device execution;
- self-hosting parity and exhaustive corpus work;
- using the `0.1.x` GitHub Action or REIR schemas as a production authorization
  control without an independent audit.

Promotion to Core requires a documented compatibility contract, inclusion in the
default supported surface, per-PR coverage on supported platforms, and closure
of threat-model debt relevant to the promoted feature.

### Unsupported-for-untrusted

`Unsupported-for-untrusted` is a security qualifier, not a lower maturity tier.
It applies whenever input can be controlled by an attacker and the operation can
execute, compile, or load that input. Core status does not override this label.

The following are unsupported for untrusted input:

- generated Cargo builds, build scripts, and executable package dependencies;
- in-process native plugins or native wrappers;
- in-process tier-0/native JIT execution and dynamically supplied GPU shaders;
- host filesystem, environment, network, process, database, or device access;
- multi-tenant execution based only on RSScript capabilities, VM budgets,
  process limits, a container, or the review action.

Outside the Linux `UntrustedIsolated` worker profile, static inspection is the
only project-supported operation for third-party source: do not build
dependencies, run hooks, load native code, execute shaders, or provide ambient
credentials. RSScript and REIR evidence alone does not enforce OS isolation.

## Deployment Profiles

| Profile | Allowed input and operations | Required controls | Project status |
| --- | --- | --- | --- |
| `LocalTrusted` | Source and dependencies controlled by the developer; Core execution and explicitly enabled Experimental paths | Review native/build-script changes; acknowledge trusted native execution; keep normal resource budgets unless deliberately debugging | Supported for development, not an adversarial boundary |
| `TrustedCI` | Reviewed organization repositories in disposable CI; Core gates and dedicated Experimental jobs | Immutable action/tool pins, locked dependencies, least-privilege token, no secrets for fork PR code, protected-base policy, isolated ephemeral runner, explicit native/JIT/Metal jobs | Supported for CI experiments; not a production authorization system |
| `UntrustedIsolated` | Bounded source evaluation and explicit worker operations; no in-process fallback | Immutable input, no ambient secrets or network, private filesystem, strict resource/time limits, killable process tree, verified launcher | Experimental on Linux with root-owned `/usr/bin/bwrap`; unsupported elsewhere |

The `rss run --deployment-profile` spellings are `local-trusted`, `trusted-ci`,
and `untrusted-isolated`. `TrustedCI` may run bounded pure code in the reference
VM. That path carries an explicit deny-all host context and rejects every
host-touching intrinsic before dispatch. It does not permit AOT, native, JIT,
GPU, process, network, database, environment, or filesystem effects.
`UntrustedIsolated` routes `rss run --vm` through the separate
`rss-execution-worker`; the absolute worker path is supplied through
`RSS_EXECUTION_WORKER`. The library also exposes isolated native-JIT,
digest-pinned native-call, and Metal request entrypoints. Every operation uses
one bounded request/response exchange and has no in-process fallback. Native
package builds remain denied in this profile.

The implemented Linux boundary requires a verified root-owned bubblewrap
launcher and fails closed when user namespaces or required process limits are
unavailable. This is an Experimental hostile-workload boundary, not a claim
that RSScript is an audited production multi-tenant sandbox. Windows and macOS
execution remain unavailable; in particular, Metal requests cannot run as
`UntrustedIsolated` until macOS has an equivalent verified launcher.

## CI Contract

| Layer | Trigger | Blocking contract |
| --- | --- | --- |
| Core | Every pull request and push to `main` | Locked full manifest, supply-chain audit, Windows/macOS containment and native authorization, review-action smoke, and other always-on Core workflows |
| Security-sensitive | Pull requests and pushes touching boundary paths; manual | Deployment-policy tests, unsafe-boundary Clippy, JIT differential safety, native ABI, process containment, runtime, REIR/LSP, and database boundary tests |
| Experimental | Matching JIT/Metal paths, nightly, manual | Full native-JIT suite on Linux and real-device Metal suite on macOS |
| JIT performance/hardening | Matching JIT performance paths or scheduled/manual hardening | Performance regression gate, sanitizer/Miri/fuzz sweeps according to the dedicated workflow |
| Release | Version tag or manual release | Core validation plus native JIT, generated backend parity, self-host corpus, and real-device Metal before artifact promotion |

Path filtering is a scheduling optimization, not a security exemption.
Experimental code under a security-sensitive path must pass both layers. Release
artifacts are still built once after the locked validation and promoted without
rebuilding.
