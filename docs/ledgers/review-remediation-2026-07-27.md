# Review Remediation Report - 2026-07-27

## Scope

This report reconciles the review of
`dce88ec7a1638b1226acfe86bc06fd2cf22936cd`. Status meanings:

- `FIXED`: an executable correction and regression coverage landed.
- `MITIGATED`: the unsafe default is closed, but the larger isolation design is
  not represented as complete.
- `ACCEPTED-DEBT`: a maintainability or platform architecture project remains
  explicit and is not a correctness claim.

## Security And Correctness Boundaries

| ID | Status | Disposition |
| --- | --- | --- |
| P0-1 native review/build TOCTOU | FIXED | Authorization captures native crates, ABI source, lock inputs, features, bindings, and RSScript lowering input into a private content-addressed snapshot. VM and AOT consume snapshot paths and immutable source contents. Cache identity is content-based. Mutation-after-authorization and AOT-lifetime regressions are covered. |
| P0-2 live Cargo resolution | MITIGATED | Native metadata and shim builds are offline; the final shim build is frozen and uses a lock generated offline from reviewed immutable path inputs. Registry resolution cannot use the network. A general container/VM, independent UID, and read-only vendored registry service is not implemented; native execution is therefore trusted-only. |
| P0-3 process containment attach failure | FIXED | All process callers use `spawn_guarded`. Attach failure kills and reaps the child. Unix process groups and limits are installed before exec, representational overflow fails closed, and timeout arithmetic is checked and capped. |
| P0-3 Windows suspended creation | MITIGATED | Job attachment is fail-closed and never returns an unguarded child, but `std::process` still cannot provide suspended-create/assign/resume atomically. A Windows-native spawn implementation remains required for hostile-code containment. |
| P0-4 untrusted in-process plugin | MITIGATED | CLI native execution is denied by default and requires explicit `--trusted-native`; README and ABI documentation state that in-process native code has full host authority. There is no claim of sandboxing. Out-of-process RPC workers/containers remain required before untrusted native execution can be offered. |
| P0-5 parser/front-end budget | FIXED | One `FrontendBudget` now covers source bytes, tokens, parse depth, AST nodes, diagnostics, semantic work, substitutions, recursion, deadline cancellation, and incomplete-analysis diagnostics. Parser consumes the existing token stream, eliminating duplicate lexing. CLI interface reads and LSP documents are bounded. |
| P0-6 native ABI output state | FIXED | `OwnedNativeBuffer` releases output on status, shape, UTF-8/JSON, and application errors. Null/length/capacity invariants and request/result/name/registry limits are enforced. Malformed-state tests cover release behavior. |

## Compiler, Runtime, And Tooling

| Area | Status | Disposition |
| --- | --- | --- |
| Analysis diagnostics bypass | FIXED | Mutable deref to the underlying diagnostic vector was removed; writes consume the shared budget. |
| Builtin/interface semantic cache | ACCEPTED-DEBT | Duplicate source lexing is removed, but a process-wide immutable semantic index is a separate compiler database project. |
| Semantic database / lowering facts | ACCEPTED-DEBT | Lowering still contains repeated semantic walkers. A sealed `ValidatedProgram`/semantic database migration is not mixed into this boundary patch. |
| Generated Rust backend check | FIXED | Cargo check is offline and target-directory creation errors are reported instead of ignored. |
| Artifact provenance | FIXED | Package review producers now include source revision, content build ID, rustc version, target, enabled features, and ruleset digest. |
| Manifest diagnostic reread | MITIGATED | Diagnostic rereads are now bounded, no-follow regular-file reads. Retaining all manifest source/spans in a parsed snapshot remains the stronger long-term model. |
| Runtime process timeout | FIXED | Timeout conversion, ceiling, and deadline addition fail closed. |
| Runtime environment inheritance | MITIGATED | Compiler/native builds use a reduced environment and offline Cargo. General `Process` remains an explicit host-process capability and intentionally inherits selected caller configuration. |
| Runtime context and domain modules | ACCEPTED-DEBT | Global runtime compatibility and large domain modules remain; resource limits and fallible boundaries are enforced independently. |
| Managed panic helpers | MITIGATED | Fallible APIs remain canonical and panic wrappers are explicitly named/documented as generated-code compatibility boundaries. |
| Directory-scoped filesystem capability | ACCEPTED-DEBT | Existing no-follow, byte, depth, count, and atomic-write controls remain. Replacing host paths with handle-relative directory capabilities is a future API break. |
| Metal arbitrary shader source | MITIGATED | A fail-closed SHA-256 allowlist policy API was added. The compatibility API is documented trusted-only; non-preemptible hostile kernels still require a worker process. |
| VM-JIT module split | ACCEPTED-DEBT | Validation and process tests remain the safety boundary; splitting the backend into sealed modules is a maintainability project. |

## LSP, REIR, Database, And Examples

| Area | Status | Disposition |
| --- | --- | --- |
| LSP malformed ranges | FIXED | Reversed ranges, out-of-range lines/columns, and UTF-16 surrogate splits are rejected without mutation or panic. |
| LSP document allocation | FIXED | Full documents and incremental results have a 16 MiB cap before analysis scheduling. |
| LSP package error cache/watch invalidation | ACCEPTED-DEBT | Generation/revision cancellation remains, but typed package-load error caching and complete external-file watcher invalidation require a workspace-service refactor. |
| LSP module split | ACCEPTED-DEBT | Transport, scheduler, workspace, and feature extraction remain a mechanical follow-up. |
| REIR aggregate input | FIXED | Collect/merge account aggregate bytes and merge rejects excessive input-file counts before reading. |
| REIR stdout output | FIXED | JSON stdout serialization uses a bounded writer and fails before exceeding the output cap. |
| REIR Windows file identity | ACCEPTED-DEBT | Unix handle identity is strong; the non-Unix fallback still needs a shared Win32 handle/file-ID secure-I/O implementation. |
| REIR module split | ACCEPTED-DEBT | CLI/use-case/I/O/presentation separation remains architectural work. |
| SQLx pool identity | FIXED | Pool keys compare normalized full URLs rather than non-cryptographic digest pairs; structured secret redaction and a bounded fixed runtime queue replace per-call threads. |
| SQLite path identity | FIXED | Equivalent database paths normalize to one bounded connection-cache identity. |
| HTTP sample | FIXED | Package documentation labels the implementation demo-only and lists its non-production limits. |
| Crypto comparison | FIXED | A fixed-size decoded SHA-256 comparison API rejects invalid encoding/length; arbitrary-string comparison documents its observable length boundary. |

## CI, Documentation, And API Governance

| Area | Status | Disposition |
| --- | --- | --- |
| GitHub token permissions | FIXED | Workflows default to `contents: read`; jobs elevate only where already required. |
| Review Action build | MITIGATED | Source dependencies are fetched from the lock, then the pinned checkout builds frozen. Shipping attested prebuilt binaries remains a release-distribution project. |
| Test diagnostics profile | FIXED | Normal tests remain lean; opt-in `test-debug` retains debug information for sanitizer/postmortem work. |
| Architecture test inventory | FIXED | Documentation now records four primary targets plus two process-isolated JIT targets and current module directories. |
| Public API facade / aliases | ACCEPTED-DEBT | A versioned `api::v1` facade and compatibility-alias removal require a planned breaking API migration. |
| Doctests and crate-wide lint allows | ACCEPTED-DEBT | Stable-facade doctests and localization of historical lint allows remain follow-up quality work. |
| Large module decomposition | ACCEPTED-DEBT | Analyzer, lowering, runtime, LSP, REIR, and VM-JIT decomposition must follow invariant boundaries rather than line-count-only moves. |

## Explicit Production Limits

This remediation does **not** turn RSScript into a sandbox:

- `--trusted-native` grants full host-process authority.
- Third-party native packages remain static-review-only.
- Native build worker containers/VMs and native execution RPC workers are not
  implemented.
- Windows hostile child creation still needs suspended spawn before Job
  assignment.
- Arbitrary Metal kernels are not preemptible in-process.
- Windows REIR secure file identity needs a shared handle-based implementation.

These limits are product constraints, not hidden implementation details.

## Verification

The integrated tree passed:

- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo check --workspace --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test -p rsscript --features native-jit --lib --locked`: 653 passed,
  7 ignored
- `cargo test -p rsscript --test runtime --features native-jit --locked`: 480
  passed, 1 ignored, including the JIT performance gate
- `cargo test -p vm-jit --locked`: 123 passed
- `cargo test -p rsscript-runtime --locked`: 246 passed
- `cargo test -p rss-process-guard --locked`: 4 passed
- native ABI host tests: 10 passed
- package authorization tests: 7 passed
- native loader tests: 12 passed
- `cargo test -p rsscript --test soak --locked`: 7 passed, 2 ignored
- `cargo test -p rss_sqlite_native --locked`: 6 passed
- `cargo test -p rss_sqlx_native --locked`: 13 passed
- Metal tests: 13 passed
- LSP tests: 33 passed
- REIR library/CLI/integration/report tests: 119 passed
- HTTP example tests: 5 passed
- crypto example tests: 2 passed

`cargo check -p rss-process-guard --target x86_64-pc-windows-gnu --locked`
also passed. The broader Windows cross-check could not run because this macOS
host does not have `x86_64-w64-mingw32-gcc`, which `ring` requires; CI remains
the authoritative full Windows build.
