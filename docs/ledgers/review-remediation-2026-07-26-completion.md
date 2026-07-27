# Review Remediation Completion Report - 2026-07-26

## Scope

This batch closes the actionable findings available in the supplied review
summary for commit `39681ec869ea2d412e797903e4b4e1886fdf12a3`,
plus the concrete residual findings found during the post-fix audit.

The original 78-item Markdown attachment is not present in the workspace.
Accordingly, this report does not claim item-by-item closure for findings that
exist only in that unavailable attachment.

## Authorization And Native Execution

- Native Cargo build and dynamic loading now require a privately constructed
  `AuthorizedPackage`.
- Authorization performs package review, lock, dependency graph, and native
  policy checks before a native build or load can begin.
- AOT lowering and generated-package cache identity use the authorized lowering
  snapshot for packages with native dependencies. A previously generated cache
  cannot bypass a later authorization failure.
- Native dependency build specifications preserve Cargo features and
  `default-features`.
- Cargo/build-script execution uses a reduced environment, bounded output,
  deadlines, process-tree ownership, and OS resource limits.
- AOT builds and generated program execution are separate processes, so build
  scripts do not inherit the program environment.

## Confined Package Artifacts

- Package metadata, lockfiles, manifests, source skeletons, vendor metadata, and
  review artifacts use a shared `ArtifactStore`.
- Artifact paths are relative and validated. Unix writes use descriptor-relative
  traversal with `O_NOFOLLOW`, same-directory staging, synchronization, and
  atomic replacement.
- Package mutations are serialized by a store lock. Vendor trees are staged and
  swapped so a failed copy preserves the previous tree.
- Package inputs are bounded before authorization: manifests, sources, source
  count, aggregate bytes, and directory depth all have limits.
- Lockfiles and artifact reads are bounded.
- Archive collection has file, entry, depth, per-file, and aggregate-byte
  limits. Native and archive hashing is streamed through fixed-size buffers.

## Runtime Resources

- A shared `ResourceBudget` supports cumulative byte consumption and refundable
  reservations.
- File, CSV, gzip, random, buffer, channel, HTTP, TCP, and WebSocket operations
  enforce hard allocation limits or shared budgets.
- Config, rule, image, JSON, TOML, and YAML file helpers use bounded runtime file
  reads rather than `read` or `read_to_string`.
- HTTP uses a shared client, bounded request and response bodies, incremental
  response accounting, total retry deadlines, cancellation, and redacted debug
  summaries.
- TCP and WebSocket read/write ownership is split. Pending reads do not block
  writes.
- Register-VM TCP and WebSocket payloads are checked before allocation, and
  closed handles are removed from resource tables so their descriptors are
  released.
- Runtime and CLI child processes enforce wall time, output limits, CPU, file
  size, descriptor count, and platform memory limits.
- `rss test` now applies the same process guard and a 16 MiB output cap per
  stream.
- SQLite and SQLx string results are borrowed and byte-accounted before copying
  into VM-owned strings.

## Platform Process Boundaries

- Linux and Android use `RLIMIT_AS` in addition to CPU, descriptor, and file-size
  limits.
- macOS uses `RLIMIT_DATA` as a verifiable best-effort memory ceiling. It does
  not cover every `mmap` allocation and is not presented as Linux-equivalent
  isolation.
- Windows uses a Job Object with kill-on-close, per-process memory, aggregate job
  memory, and process-tree termination.
- Unsupported operating systems fail guard setup instead of silently running
  without kernel limits.

## Terraform, LSP, And Evidence

- Terraform source traversal is canonical-root confined, rejects symlinks and
  reparse points, tracks visited directories, and enforces depth, file-count,
  per-file, and aggregate-byte limits.
- Heuristic Terraform source results are `Unknown`, `Scanned`, and
  `SourceScan`; they cannot prove production authorization.
- LSP analysis uses immutable document/package snapshots, workspace generations,
  cancellation tokens, debounce, superseded-task aborts, and publication guards.

## Remaining Architectural Limits

These are explicit contract limits, not silently accepted safety claims:

- Native Rust dependencies remain full host-code execution authorization, not a
  sandbox.
- macOS memory enforcement is best-effort. Windows Job Object attachment occurs
  immediately after spawn; stronger hostile-code isolation still requires a
  dedicated suspended worker/container boundary.
- HTTP and WebSocket public APIs return owned messages, so they incrementally
  enforce a hard cap before returning rather than exposing a streaming body API.
- Synchronous file and CSV reads cannot observe asynchronous cancellation while
  blocked in a system call.
- LSP cancellation is cooperative between analyzer phases; an individual
  analyzer call does not yet accept a cancellation token.
- The unavailable 78-item attachment must be restored before asserting literal
  closure of every original review row.

## Verification

The integrated workspace passed:

- `cargo fmt --all -- --check`
- `cargo check --workspace --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test -p rsscript --test static` (654 passed)
- `cargo test -p rsscript --test runtime --features native-jit`
  (480 passed, 1 release-only performance test ignored)
- `cargo test -p rsscript --features native-jit --lib`
  (633 passed, 7 parity/performance corpus tests ignored)
- `cargo test -p rsscript-runtime` (234 passed)
- `cargo test -p reir` (110 passed across library, CLI, integration, and report
  tests)
- `cargo test -p rss-lsp` (28 passed)
- `cargo test -p rsscript --bin rss` (55 passed)
- `cargo test -p rss-process-guard` (3 passed)
- SQLite and SQLx native adapter tests (9 passed)
- `cargo check -p rsscript --target x86_64-pc-windows-gnu --locked`
- `docker build --check .`
