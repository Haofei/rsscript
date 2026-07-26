# Review Remediation Report - 2026-07-26

## Scope

This batch addresses every actionable High finding in the supplied summary for
commit `39681ec869ea2d412e797903e4b4e1886fdf12a3`. The complete 78-item attachment
was not present in the workspace, so this report does not claim closure of
items that appeared only in that attachment.

## Package And Native Execution

- Native package loading now runs the package review/authorization check before
  generating, building, caching, or loading a native shim.
- Native dependency build specifications preserve sorted Cargo features and the
  `default-features` setting in generated manifests and cache identities.
- Package artifact writes use confined relative paths, reject symlinked parent
  components and destinations, write through a same-directory temporary file,
  synchronize it, and atomically rename it.
- Vendor roots and destinations reject symlinks and vendor metadata uses the
  shared atomic package artifact writer.
- Package graph expansion emits references for already-seen canonical packages,
  preventing diamond dependency graphs from reloading and rescanning shared
  subgraphs.

Native authorization remains authorization to run full host code. It is not a
sandbox. The portable path checks substantially reduce accidental traversal and
symlink escapes, but strict race-free confinement still requires directory
handle-relative APIs such as `openat2` on Linux and reparse-point-safe handles on
Windows.

## Execution Limits

- VM CLI execution enables conservative defaults for steps, cumulative
  allocation growth, output, host calls, recursion, and wall time.
- Unlimited VM execution requires the explicit `--trusted-unlimited` flag.
- Source inputs are capped at 16 MiB.
- Generated Rust checks and AOT execution use bounded child-process deadlines
  and output capture.
- Workspace release builds now unwind panics so CLI, LSP, and native ABI
  boundaries can translate failures instead of unconditionally aborting.
- Rayon integer reductions use checked arithmetic and preserve a catchable
  overflow boundary.

The VM watchdog is cooperative and cannot preempt a host call that blocks
without returning. AOT child limits bound time and captured output, not CPU or
resident memory; hostile native/AOT work still belongs in an isolated worker.

## Async And Network Runtime

- Native timer sleeps run on the shared Tokio runtime instead of creating one OS
  thread per timer.
- Cancellation tokens notify registered waiters.
- TCP and WebSocket connections split read and write ownership so a pending read
  cannot hold the write path's mutex.
- HTTP requests reuse a shared client, default to a 30-second timeout, cap
  request and response bodies at 16 MiB, bound retries by a total deadline, and
  redact credentials and large bodies from debug output.

## Terraform And LSP

- Terraform source traversal rejects symlinks and enforces depth, file-count,
  per-file, and aggregate-byte limits.
- Heuristic `.tf` source results are explicitly unverified/unknown evidence;
  plan and state JSON remain the verified evidence path.
- LSP documents use immutable `Arc<str>` snapshots, debounce analysis, abort
  superseded tasks, cancel pending work on close, and retain revision/version
  checks before publishing diagnostics.

Aborting an LSP task prevents stale publication but cannot interrupt a checker
already executing inside `spawn_blocking`. True incremental analysis remains a
separate architectural project.

## Supply Chain

- Self-host, JIT hardening, and JIT performance workflows pin actions and Rust
  toolchains to immutable revisions or dated versions.
- The development image pins the Rust base image by digest and verifies the
  pinned `cargo-nextest` archive checksum.

## Verification

The integrated workspace passed:

- `cargo fmt --all -- --check`
- `cargo check --workspace --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test -p rsscript --test static` (654 passed)
- `cargo test -p rsscript --test runtime --features native-jit`
  (480 passed, 1 release-only performance test ignored)
- `cargo test -p rsscript --features native-jit --lib`
  (608 passed, 7 ignored)
- `cargo test -p rsscript-runtime` (216 passed)
- `cargo test -p reir`
- `cargo test -p rss-lsp` (24 passed)
- `cargo test -p rsscript --bin rss` (51 passed)
- `cargo test --release -p rss_rayon_native` (2 passed)
- `docker build --check .`
