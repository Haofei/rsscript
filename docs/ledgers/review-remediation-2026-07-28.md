# Review Remediation Report - 2026-07-28

## Scope

This report reconciles the static review of
`044a76a3ef0be7ecf6cce020ef9283d52f141d1d`.

Status meanings:

- `FIXED`: an executable correction and regression coverage landed.
- `MITIGATED`: unsafe behavior is denied or bounded, but a larger isolation
  design remains.
- `ACCEPTED-DEBT`: a maintainability or platform project remains explicit and
  is not represented as a correctness fix.

## P0 And P1 Findings

| Finding | Status | Disposition |
| --- | --- | --- |
| Granular native unsafe policy was folded into one string | FIXED | RSS unsafe APIs, wrapper unsafe blocks, and transitive unsafe blocks remain independent typed policy dimensions. Legacy `unsafe` expands only when no granular field is present; mixed configuration is rejected. Diff, review, publish, risk classification, and wrapper enforcement consume the structured policy. |
| Native semantic scan skipped unreadable files | FIXED | Scan results carry completeness and structured errors. Enumeration/read failures produce incomplete evidence and `Unknown` risk instead of an empty finding set. The scanner remains advisory and is not treated as proof of macro-expanded or transitive Rust semantics. |
| Windows process executes before Job attachment | MITIGATED | Process support now reports whether containment and each requested limit are enforced, best-effort, or unsupported. Platforms without atomic containment fail closed. A suspended-create/assign/resume Win32 launcher is still required to execute hostile children on Windows. |
| Unix descendants survive normal root exit | FIXED | `GuardedChild` owns the child and process-tree guard together. Normal completion, timeout, cancellation, stream drop, poll error, and RAII drop terminate descendants and reap the root. A background-descendant regression test verifies the boundary. |
| Process limits silently degrade across platforms | FIXED | Requested unsupported limits are rejected and strict callers can require every limit to be fully enforced. macOS address-space enforcement remains explicitly `BestEffort`; Windows strict guarded execution is rejected until an atomic launcher exists. |
| LSP continues from stale text after an invalid incremental edit | FIXED | Document updates return `Applied`, `IgnoredStale`, or `Desynchronized`. Invalid UTF-16 ranges, reversed ranges, and oversized results enter desync, cancel analysis, clear diagnostics, and exclude the document from semantic requests until a full-text update restores synchronization. |
| LSP package cache misses external changes | FIXED | The server registers package input watchers, invalidates affected package roots, advances revisions, and reschedules current synchronized documents. Poisoned cache mutexes recover by discarding poison rather than taking down the server. Typed package-load errors remain a workspace-service follow-up. |
| SQLite `:memory:` databases share global state | FIXED | Plain `:memory:` opens an uncached connection for each request. Regression coverage proves two instances cannot observe each other's schema or data. Explicit shared-memory SQLite URIs retain their documented opt-in semantics. |
| SQLx single worker serializes every pool and tenant | FIXED | A multi-thread Tokio runtime executes jobs concurrently. Global and per-pool semaphores provide fail-fast bounded admission. Tests verify independent jobs overlap and saturation reports its scope. The public native ABI remains a synchronous bridge over asynchronous execution. |
| SQLx URL identity and logs expose or merge sensitive endpoints | FIXED | Pool identity compares full normalized URLs without reordering query parameters. User-visible fingerprints are SHA-256 over redacted endpoints. Structured redaction covers encoded credentials, standalone passwords, and sensitive query values. |
| JIT code-size setting is only a post-allocation counter | FIXED | Cranelift modules use fixed `ArenaMemoryProvider` mappings reserved from one shared hard budget before code generation. Baseline and optimized tiers cannot map beyond the configured total; failed compilation cannot grow the arena, and dropping a module returns its reservation. Cranelift still cannot release one failed function independently inside a live arena. |
| Release rebuilds after validation | FIXED | Release validation builds the frozen release binaries, smokes the exact files, records checksums/build information, and uploads one immutable workflow artifact. The publishing job downloads, verifies, attests, and publishes that artifact without rebuilding it. |
| Terraform plan/state JSON is unbounded | FIXED | Bounded readers and budgets cover input bytes, JSON depth/nodes, modules, resources, facts, and evidence-producing multiplication. State traversal is iterative. Limit exhaustion fails closed before publishing partial evidence. |
| Platform security code is absent from platform CI | FIXED | CI now runs process containment and package/native cache tests on Windows and macOS, with Metal policy/device tests on macOS. A separate path-filtered workflow runs unsafe-boundary Clippy, JIT differential tests, native ABI tests, and process containment tests. |
| Artifact store returns plain error after commit point | FIXED | Directory replacement distinguishes `Committed` from `CommittedWithCleanupWarning`. Failures after the staging-to-destination rename cannot be reported as not committed. Vendor callers preserve warnings and elevate review risk. |
| Non-Unix cache owner/ACL checks silently succeed | MITIGATED | Native authorization and native plugin cache operations fail closed on platforms where ownership and private ACLs cannot be verified. A SID/DACL-backed secure cache implementation is still required to enable native execution on Windows. |

## Additional Corrections

| Area | Status | Disposition |
| --- | --- | --- |
| Gate policy default | FIXED | `GatePolicy::default()` is the production fail-closed policy. Development behavior requires the explicit `development()` constructor. |
| Runtime path helper names | FIXED | APIs now state that they provide lexical checks; deprecated compatibility wrappers remain. `directory_exists` checks directories only, and `path_exists` is separate. |
| Stream collection | MITIGATED | Collection has a default item cap and deadline plus configurable item budget, cancellation, and deadline. Generic heap-byte accounting still belongs in the VM/resource budget because the channel cannot measure transitive size of arbitrary `T`. |
| Socket and WebSocket operations | FIXED | Connect/read/write/send/receive/close paths have default deadlines and explicit budget/deadline/cancellation variants while retaining split read/write halves. |
| Metal raw shader boundary | MITIGATED | Policy callers use `TrustedShader` and `gpu_run_1d_trusted`; the compatibility raw entry is hidden from docs and explicitly trusted-only. A hostile non-preemptible kernel still requires a killable worker process. |
| Native plugin cache and source paths | MITIGATED | The previous immutable snapshot, offline/frozen build, digest, and trusted-only controls remain. This patch adds fail-closed platform ownership behavior; it does not claim that in-process native code is sandboxed. |

## Accepted Architecture Debt

The following are useful projects but are not represented as completed fixes:

- a suspended Win32 launcher and SID/DACL secure artifact store;
- out-of-process workers for untrusted native plugins, JIT, and dynamic GPU
  shaders;
- a sealed typed semantic database/`ValidatedProgram` consumed by every
  backend;
- generic substitution entirely over structural type IR instead of display
  strings;
- module decomposition for LSP, VM-JIT, register VM, package native review,
  runtime domains, and REIR;
- removal of global runtime/registry compatibility state through dependency
  injection;
- a stable versioned public API facade and workspace-wide lint migration;
- a truly asynchronous native database ABI and live PostgreSQL release runner.

These are not required to claim that the concrete wrong-result, stale-state,
resource-boundary, and release-artifact findings above are corrected. They are
required before claiming multi-tenant sandboxing or a mature production
enforcement platform.

## Verification

The integrated tree passed:

- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo check --workspace --all-targets --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
  - RSScript library: 494 passed, 7 ignored
  - RSScript runtime integration: 331 passed
  - RSScript static suite: 656 passed
  - runtime crate: 250 passed
  - VM-JIT: 125 passed
  - LSP: 36 passed
  - REIR library/CLI/integration: 123 passed
  - process guard: 7 passed
  - SQLx: 14 passed
  - SQLite: 7 passed
- `cargo test -p rsscript --features native-jit --lib --locked`: 655 passed,
  7 ignored
- `cargo test -p rsscript --features native-jit --test runtime --locked`: 480
  passed, 1 ignored
- `cargo check -p rss-process-guard --target x86_64-pc-windows-gnu --locked`
- all GitHub workflow YAML files parsed successfully

The macOS host exercised real Metal tests. Live PostgreSQL tests remain
environment-gated by `RSS_SQLX_TEST_POSTGRES_URL`; CI now provides the platform
matrix for Windows and macOS security paths.
