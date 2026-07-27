# Review Remediation Completion Report - 2026-07-26

## Scope

This report reconciles every finding in
`docs/ledgers/rsscript-code-review-39681ec.md`, the review of commit
`39681ec869ea2d412e797903e4b4e1886fdf12a3`.

The checked-in review has SHA-256
`93e4e775665ea9988342a06ab99adf6aef777d4114af36465ccddc716fd40a25`.
It contains 78 finding IDs. This ledger accounts for every one.

Status meanings:

- `FIXED`: the reported defect or missing guard has a code, test, workflow, or
  documentation correction.
- `MITIGATED`: the unsafe or misleading claim is closed and the remaining
  limitation is explicit, but the broader redesign is intentionally incomplete.
- `ACCEPTED-DEBT`: a maintainability refactor, not a correctness closure. It is
  recorded rather than misrepresented as completed.

Summary: **59 fixed, 10 mitigated, 9 accepted architectural debts, 0
unaccounted findings**.

## Item-By-Item Ledger

| ID | Status | Disposition |
| --- | --- | --- |
| ARCH-01 | FIXED | Release tooling uses unwind; expected adapter failures return structured errors. |
| ARCH-02 | ACCEPTED-DEBT | Cargo profiles remain workspace-wide; process isolation and explicit worker limits are the current product boundary. |
| DOC-01 | FIXED | Documentation authority and the complete Cargo-facing test inventory are explicit and current. |
| DOC-02 | MITIGATED | Documentation layout checks remain repository-relative, but freshness and authority checks fail visibly in CI. |
| CI-01 | FIXED | Auxiliary workflows and toolchains are pinned to immutable revisions or explicit versions. |
| CI-02 | FIXED | PR CI includes RustSec and the shared security/regression gates; release depends on the complete validation job. |
| CI-03 | FIXED | SARIF generation failures are no longer swallowed; the action fails when the artifact is absent. |
| SUPPLY-01 | FIXED | The development image is digest-pinned and nextest installation is version/checksum pinned. |
| FS-01 | FIXED | Package reads and copies use opened-handle identity checks and descriptor-relative no-follow traversal. |
| PROC-01 | FIXED | Process-tree termination no longer shells out to a PATH-resolved `kill`. |
| PROC-02 | FIXED | Copy/hash operations account bytes from opened files and detect source or staging replacement. |
| SEC-01 | FIXED | Vendor roots, staging trees, and destinations reject links/reparse points and use confined directory handles. |
| REL-01 | FIXED | Vendor publication is staged, synchronized, and atomically swapped; failed staging preserves the previous tree. |
| SEC-02 | FIXED | Package artifacts use no-follow, same-directory staging and atomic replacement rather than direct `fs::write`. |
| QUALITY-01 | FIXED | `pkg add` uses `toml_edit`, preserving comments and formatting. |
| REL-02 | FIXED | Package creation stages the complete package and lockfile before one rename; package mutations are serialized. |
| PERF-01 | FIXED | Package review retains a deduplicated DAG and emits references instead of rematerializing dependency paths. |
| PERF-02 | FIXED | Archive and checksum traversal enforce depth, count, per-file, and aggregate-byte budgets. |
| SEC-03 | MITIGATED | Native source scanning is explicitly heuristic evidence and cannot authorize host-code execution. |
| SEC-04 | FIXED | Temporary package/native artifacts use private `tempfile` directories and atomic publication. |
| QUALITY-02 | FIXED | Native manifests and binding DTOs use centralized schemas and reject unknown fields. |
| DOC-03 | FIXED | Publish documentation describes validation/dry-run behavior and does not claim a complete registry service. |
| SEC-05 | FIXED | Native build/load and package AOT paths require a privately constructed authorized package. |
| BUG-01 | FIXED | Native dependency features and `default-features` are preserved in generated manifests and cache identities. |
| PERF-03 | FIXED | VM CLI execution uses conservative default limits; unlimited execution requires `--trusted-unlimited`. |
| PERF-04 | FIXED | AOT/build execution uses bounded streaming capture, deadlines, and process guards. |
| PERF-05 | FIXED | Standalone source reads are bounded before allocation. |
| BUG-02 | FIXED | Exit codes preserve 0-255, map signals explicitly, and avoid lossy casts. |
| PERF-06 | FIXED | Test child output is capped while being read instead of using unbounded `read_to_string`. |
| PERF-07 | MITIGATED | Test subprocesses are bounded and cleanly terminated; top-level `--all` remains sequential and does not yet expose fail-fast scheduling. |
| ARCH-03 | ACCEPTED-DEBT | Register-VM responsibilities are still being split by invariant; a wholesale file move was not mixed into security fixes. |
| PERF-08 | FIXED | Native eligibility reachability is linear in graph size and has 100k-node chain/star/SCC regressions. |
| QUALITY-03 | MITIGATED | Streaming output propagates sink errors and is hard-capped, but the compatibility API still retains bounded captured output. |
| ARCH-04 | ACCEPTED-DEBT | `vm-jit` remains large; validator/analysis/codegen boundaries are tested but not mechanically split in this batch. |
| ARCH-05 | ACCEPTED-DEBT | Rust lowering still shares backend/file-output concerns; typed semantic fixes were prioritized over a broad module migration. |
| PERF-09 | FIXED | Analyzer phases share a work budget for nodes, substitutions, diagnostics, and recursion and emit `RS0040` on exhaustion. |
| ARCH-06 | ACCEPTED-DEBT | The compiler crate retains a broad compatibility re-export surface pending a versioned API reduction. |
| PERF-10 | FIXED | Native timers use the shared Tokio runtime instead of one OS thread per timer. |
| REL-03 | FIXED | Completed/dropped native tasks deregister cancellation abort handles. |
| PERF-11 | FIXED | Ready wake keys are deduplicated and task-group polling avoids repeated linear queue growth. |
| ARCH-07 | FIXED | Tokio worker count is deployment-configurable and hard bounded. |
| CONC-01 | FIXED | TCP uses independent owned read/write halves with cancellation and byte budgets. |
| CONC-02 | FIXED | WebSocket sink/stream ownership is split; pending receive no longer blocks send or close. |
| SEC-06 | FIXED | WebSocket and network errors redact URL credentials and query secrets. |
| PERF-12 | FIXED | Runtime process concurrency is globally capped regardless of caller-provided job counts. |
| PROC-03 | MITIGATED | Windows Job Objects kill descendants and cap resources; attachment still occurs immediately after spawn rather than suspended creation. |
| CONC-03 | MITIGATED | Owned process operations observe cancellation and kill their process tree; arbitrary external `spawn_blocking` work is not preemptible. |
| ARCH-08 | ACCEPTED-DEBT | Runtime domain services remain in a large module; common budgets and typed services now enforce shared boundaries. |
| PERF-13 | FIXED | HTTP reuses a client and enforces request/response limits, retry count, cancellation, and a total deadline. |
| SEC-07 | FIXED | HTTP, WebSocket, and SQL diagnostics redact URLs, bodies, credentials, and connection strings. |
| CONC-04 | FIXED | External stream bridges use bounded channels, cancellation-aware drop, and shared async wake infrastructure. |
| BUG-03 | FIXED | Allocation capacities and tensor/list dimensions use checked target-width conversion and hard ceilings. |
| BUG-04 | FIXED | Resource-pool factories run outside the mutable pool borrow and reentrancy returns a structured error. |
| REL-04 | MITIGATED | Script-facing borrow conflicts use fallible APIs and debug formatting is non-panicking; legacy trusted Rust helpers still provide panic wrappers. |
| FS-02 | FIXED | Durable atomic writes synchronize file and parent directory and are the package/runtime artifact path. |
| NABI-01 | FIXED | Release unwind matches the in-process ABI catch boundary; expected native failures use status/results. |
| NABI-02 | FIXED | Libraries are streamed into a private content-addressed store, verified, and loaded from that immutable copy. |
| NUM-01 | FIXED | Metal dimensions, byte products, threadgroups, and integer conversions are checked before allocation/dispatch. |
| PERF-14 | FIXED | Metal device/queue state is process-wide and pipelines use a bounded LRU cache. |
| BUG-05 | FIXED | CLI positional parsing no longer consumes the value after a boolean flag; value-taking options are explicit. |
| CRYPTO-01 | FIXED | Constant-time equality uses the audited `subtle` implementation. |
| PROD-01 | MITIGATED | The sample HTTP server propagates I/O errors and is explicitly demo-only; it is not presented as a production server. |
| BUG-06 | FIXED | Rayon integer reductions return checked `Result` errors; overflow cannot abort the host process. |
| DB-01 | FIXED | SQLite uses bounded LRU connection reuse, busy timeout, and an explicit transaction batch path. |
| DB-02 | FIXED | SQLx enforces deadlines, redacts URLs, streams bounded results, and evicts/closes bounded pool entries. |
| PERF-15 | FIXED | Reconciliation indexes facts by category and caps aggregated evidence with an explicit truncation marker. |
| SEC-08 | FIXED | Terraform discovery rejects links, tracks visited directories, and enforces depth/file/byte limits. |
| SEC-09 | MITIGATED | Heuristic HCL source scanning remains preview-only `Unknown` evidence; production proof uses structured plan/state JSON. |
| OBS-01 | FIXED | Embedded policy/state parse failures produce machine-readable blocking diagnostic facts. |
| ARCH-09 | ACCEPTED-DEBT | REIR CLI remains a large orchestration module; bounded I/O and atomic output isolate its current side effects. |
| SEC-10 | FIXED | REIR evidence/policy reads are bounded, no-follow, and outputs are confined atomic writes. |
| ARCH-10 | ACCEPTED-DEBT | The RSScript REIR adapter remains large; semantic normalization is covered but not reorganized in this batch. |
| PERF-16 | FIXED | LSP uses immutable snapshots, debounce, generations, cancellation, caching, and a two-job blocking-work semaphore. |
| CONC-05 | FIXED | Feature handlers release the document lock before parsing/scanning and stale package jobs cannot publish. |
| ARCH-11 | ACCEPTED-DEBT | LSP transport/state/features still share one file; concurrency invariants are now explicit and stress-tested. |
| TEST-01 | FIXED | PR/release suites cover package links, process trees, limits, native ABI, adapters, JIT, LSP, and policy failures. |
| TEST-02 | MITIGATED | Existing fuzz/hostile suites cover parser, VM and JIT IR; raw-byte adapter/schema fuzzing is still a nightly expansion item. |
| TEST-03 | FIXED | Complexity/resource invariants now accompany throughput tests for graphs, analysis budgets, streams, memoization, and process concurrency. |

## Material Corrections In This Batch

- Package and vendor traversal now holds directory/file identities across
  validation and use. Windows identity checks use stable Win32
  `GetFileInformationByHandle`; Unix uses descriptor-relative traversal and
  device/inode identity.
- Package creation is a transaction: manifest, source, and lock are generated
  in a private sibling staging directory and committed by rename.
- The compiler has a shared analysis budget and an explicit incomplete-analysis
  diagnostic instead of silent truncation.
- Register-VM native eligibility no longer allocates an O(V^2) reachability
  matrix.
- LSP analysis is package-generation aware, cancellable, snapshot-based, and
  globally bounded to two blocking jobs.
- Runtime async wake state, cancellation registration, external streams,
  process concurrency, resource-pool reentrancy, and durable writes have bounded
  ownership.
- SQLite and SQLx have bounded connection/pool lifecycles, deadlines, redacted
  diagnostics, bounded query results, and parameterized APIs.
- Metal allocation math is checked and its expensive device/pipeline state is
  reused through a bounded cache.
- Native libraries are copied and verified into a private content-addressed
  store before dynamic loading.
- Rayon overflow, CLI positional parsing, crypto comparison, HTTP demo error
  propagation, CI supply-chain checks, and SARIF failure visibility are covered
  by focused corrections.

## Explicit Remaining Limits

The nine `ACCEPTED-DEBT` rows are module/API decomposition projects. Performing
large mechanical moves solely to reduce line counts would increase review risk
without changing the reported security boundary.

The ten `MITIGATED` rows are also intentional and visible:

- Native Rust authorization remains permission to execute full host code, not a
  sandbox.
- macOS process memory limiting is best-effort; Windows hostile-code isolation
  is stronger with suspended worker creation than post-spawn Job attachment.
- Arbitrary synchronous foreign work cannot be forcibly cancelled in-process.
- Source HCL evidence cannot authorize production policy.
- Public compatibility helpers still include bounded capture and trusted-Rust
  panic wrappers.
- Full raw-byte adapter/schema fuzzing and fail-fast parallel top-level test
  scheduling remain follow-up engineering work.

There are no unaccounted review IDs and no known unfixed wrong-result,
path-escape, unbounded-allocation, or policy-bypass defect from the supplied
review.

## Verification

The integrated workspace passed:

- `cargo fmt --all -- --check`
- `cargo check --workspace --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test -p rsscript --test static` (654 passed)
- `cargo test -p rsscript --test runtime --features native-jit`
  (480 passed, 1 release-only test ignored)
- `cargo test -p rsscript --features native-jit --lib`
  (643 passed, 7 full-corpus tests ignored)
- `cargo test -p rsscript-runtime` (246 passed)
- `cargo test -p reir` (116 passed across library, CLI, integration, and report
  tests)
- `cargo test -p rss-lsp` (31 passed)
- `cargo test -p rsscript --bin rss package` (12 passed)
- `cargo test -p rss-native-abi --features host` (5 passed)
- Metal, process-guard, CLI, crypto, HTTP demo, Rayon, SQLite, and SQLx native
  suites (40 passed)
- `cargo check -p rsscript --target x86_64-pc-windows-gnu --locked`
- `docker build --check .`
