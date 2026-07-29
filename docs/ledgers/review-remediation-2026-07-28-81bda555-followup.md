# Review Remediation Follow-up - 2026-07-28

## Scope

This ledger reconciles the static review of commit
`81bda555804856052925b21013ae1f6deea4db37`.

Status meanings:

- `FIXED`: executable behavior and regression coverage changed.
- `MITIGATED`: the unsafe default or common path is bounded, but a stronger
  platform or isolation project remains.
- `OPEN`: the finding remains a release or deployment blocker for the stated
  threat model.
- `ACCEPTED-DEBT`: a maintainability migration remains and is not represented
  as a correctness closure.

## Findings

| Finding | Status | Disposition |
| --- | --- | --- |
| RSS-001 untrusted execution lacks a mandatory worker/OS sandbox | OPEN | The supported deployment model remains trusted local development and trusted CI. In-process native plugins, JIT code, and dynamic GPU shaders are not offered as a multi-tenant sandbox. Killable workers with OS identity, filesystem, network, and resource isolation remain mandatory before enabling an `UntrustedIsolated` profile. |
| RSS-002 package check and execution snapshot can observe different trees | OPEN | `AuthorizedPackage` prevents post-authorization path rediscovery and native files are snapshotted, but the package is still checked before the complete package/dependency closure is captured. The durable fix is snapshot-first review over one immutable dependency graph. Re-checking or hashing a mutable path would not close a hostile A-to-B-to-A race and is therefore not claimed as a fix. |
| RSS-003 malformed native error buffers can be freed before validation | FIXED | A failed native call must return the strict empty buffer state. The host validates a successful buffer before taking RAII ownership and never calls a plugin free callback for malformed, unowned metadata. Malformed status/buffer combinations and successful decode failures have regression coverage. |
| RSS-004 atomic replacement can widen private file permissions | FIXED | Replacement files inherit existing permissions before the atomic rename. Unix regression coverage verifies that replacing a `0600` file preserves `0600`. Append no longer uses an `exists`/`open` sequence. Full Windows ACL/xattr preservation remains part of the secure-filesystem project. |
| RSS-005 child processes inherit ambient secrets and can run forever by default | FIXED | Child environments are cleared and rebuilt from a small platform allowlist plus explicit request values. Non-positive timeout requests use a finite 30-second default. Capture uses bounded channels and the existing total output ceiling. Explicit unlimited trusted-process execution is not part of the default API. |
| RSS-006 TCP and WebSocket calls lack target-level authorization | FIXED | Runtime networking accepts an injectable target policy. The strict policy rejects loopback, private, link-local, multicast, unspecified, documentation, and other non-global targets after DNS resolution, and connections use the authorized IP without a second resolution. The compatibility allow-all policy remains for trusted local callers. Hosted deployments must inject the strict or a narrower policy. |
| RSS-007 SQLx pool identity retains credential-bearing DSNs | FIXED | Pool registry keys are process-salted SHA-256 identities. Plaintext DSNs, passwords, and token query values are no longer retained in the registry key or its debug representation. |
| RSS-008 LSP diagnostics publication holds global state across network await | FIXED | State commit and publication are separated. A bounded, coalescing publisher keeps only the latest pending publication per URI and performs client I/O outside document/global locks. Slow-client coverage verifies that another document can continue progressing. |
| RSS-009 REIR reconciliation can perform quadratic candidate scans | FIXED | Reconciliation uses normalized category/prefix indexes and a hard comparison budget. Budget exhaustion produces a fail-closed unknown result rather than a partial pass. Pathological same-category regression coverage exercises the bound. |
| RSS-010 unsupported Terraform resources disappear from coverage | FIXED | Unsupported or unmappable resources emit explicit unknown coverage evidence. JSON traversal continues to enforce byte, depth, node, resource, fact, and evidence budgets. |
| RSS-011 JIT/GPU/native trust depends on caller convention | MITIGATED | The public raw Metal source entry was removed; runtime callers must make the trusted-shader transition explicit or use digest policy. Native execution remains explicitly trusted-only and bounded JIT integration rejects unsafe limited execution paths. Sealed metered JIT types and out-of-process native/JIT/GPU workers remain required for hostile workloads. |
| RSS-012 security-sensitive workflow misses boundary paths | FIXED | The workflow covers runtime filesystem/network/budget code, REIR, database adapters, the composite review action, release workflow, Docker/Compose, and its own manifest. Boundary tests and Clippy run when those paths change. |

## Additional Changes

| Area | Status | Disposition |
| --- | --- | --- |
| SQLite long-running statements | FIXED | SQLite operations have a finite default deadline and a progress handler that checks deadlines and cancellation. Plain `:memory:` instances remain isolated. |
| Process output backpressure | FIXED | Non-streaming capture uses a bounded channel in addition to the byte ceiling. |
| Review action repeated reconciliation | FIXED | The action computes one canonical decision artifact and renders Markdown, CI JSON, and SARIF from that immutable decision. Temporary evidence is removed with an exit trap. |
| Container default identity | FIXED | The development image runs as a configurable non-root UID/GID. This reduces ambient authority but is not represented as an untrusted-code sandbox. |
| Metal arbitrary-source compatibility export | FIXED | The public compatibility export was removed from the Metal crate. The runtime public surface exposes the explicit trusted-shader operation. |

## Accepted Architecture Debt

The following projects remain explicit:

- snapshot-first package review over an immutable, content-addressed dependency
  closure;
- out-of-process workers for untrusted native plugins, JIT, GPU shaders, and
  hostile child processes;
- a machine-enforced deployment profile that cannot construct unsupported
  in-process capabilities;
- one injected `ExecutionContext` replacing process-wide runtime, database,
  native, and Metal registries;
- directory/file/database/executable capabilities replacing ambient host paths
  and command strings;
- sealed metered JIT types and a validated execution plan;
- a Windows SID/DACL secure artifact store and suspended guarded launcher;
- module decomposition for LSP, VM-JIT, register VM, package native review,
  runtime domains, and REIR;
- a typed semantic database/`ValidatedProgram`, structural generic
  substitution, and a versioned public API facade;
- an asynchronous native database ABI and live PostgreSQL/Metal release
  runners.

These projects are required before claiming multi-tenant isolation. They are
not required to describe the current trusted-local review and execution model
accurately.

## Verification

The integrated tree passed:

- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
  - RSScript library: 494 passed, 7 ignored
  - static suite: 656 passed
  - runtime integration: 331 passed
  - runtime crate: 256 passed
  - VM-JIT: 125 passed
  - LSP: 37 passed
  - REIR library/CLI/integration: 128 passed
  - SQLx: 14 passed
  - SQLite: 9 passed
  - native ABI: 11 passed
- `cargo test -p rsscript --features native-jit --lib --locked`: 655 passed,
  7 ignored
- the native/interpreter seed differential test with 16 property cases
- `docker compose config`
- a full Docker image build using the host UID/GID

Live PostgreSQL remains environment-gated. The macOS host exercised the Metal
test suite; Windows behavior remains covered by the repository's platform CI.
