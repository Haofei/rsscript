# Review Remediation Report - 2026-07-25

## Scope

This remediation batch addresses the outstanding JIT, package, runtime, REIR,
LSP, SQL adapter, release-gate, and test-speed findings reviewed through commit
`f6f2fa9f`.

## JIT

- Added deterministic compile-shape telemetry for direct list bounds checks,
  memoized and ordinary host calls, and forwarded list loads.
- Added conservative entry guards and proof-driven bounds-check elimination for
  direct flat-list accesses.
- Extended direct list store-to-load forwarding across safe scalar instructions
  within a basic block.
- Moved memo state out of the public JIT register window into private Cranelift
  state.
- Added validated activation-scoped memo metadata and safe nested-loop resets.
- Added receiver-domain, heap-provenance, and projection-aware memo
  invalidation.
- Changed OSR warmup from raw backedge counts to deterministic interpreted-work
  units.
- Added bounded multi-region OSR keyed by function and loop header, with at most
  four candidates per function.
- Added bounded recursive materialization recipes for clean OSR exits involving
  options and nested structs. Unsupported layouts continue to fall back.
- Added shared machine-code and compile-time admission budgets.
- Added separate baseline and optimized Cranelift modules with deterministic
  promotion. Recursive and native-call groups remain baseline-only.
- Added bounded runtime shape multiversioning. Shape keys contain stable ABI
  classes and interned aggregate layout identities. Closure target identities
  remain in the existing bounded PIC rather than consuming whole-function shape
  versions. Common shape keys use inline, allocation-free storage. Each
  site/tier admits at most two versions.
- Preserved the eager/benchmark contract by compiling a single `speed` module
  when the tier threshold is zero; non-eager execution retains the baseline to
  optimized ladder.
- Added function-level negative caching for cost-model declines and
  no-amortization give-up. Closure-profile pending state now retries only after
  its bounded sampling window freezes instead of probing translation on every
  hot call.
- Folded non-escaping `Bytes.slice(...); Bytes.len(...)` into overflow-free
  scalar clamp arithmetic. Dynamic inputs retain validation at the original
  slice site, while activation-local memoization removes repeated helpers.
- Fused Map and SortedMap integer/float lookup matches so payload and `found`
  cross one helper boundary. Removed the thread-local found-flag side channel,
  retained edge-sensitive definite assignment, and bumped the public JIT IR to
  version 25.

The implementation deliberately does not claim executable-code reclamation.
Cranelift code rejected after emission remains owned until VM teardown. Logical
inlined-frame reconstruction and mutable-argument frame reconstruction remain
conservative fallback boundaries.

## Security And Resource Boundaries

- Narrow deny intersections now block validated REIR gates and count against
  missing-capability budgets.
- Native async work retains an abort handle and is stopped on cancellation or
  pending drop.
- Process timeout/cancellation remains the primary result when termination also
  causes a stdin broken pipe.
- File reads and stream chunks use a shared 64 MiB ceiling.
- VM file intrinsics use the same bounded runtime readers; rejected cursor reads
  leave the cursor unchanged.
- Recursive directory listing rejects symlinks and enforces depth, entry-count,
  and path-byte budgets.
- Process execution uses one streaming, capped engine. Stdin writing no longer
  blocks timeout startup, zero output caps select the 64 MiB ceiling, and
  timeout/cancellation/drop terminate the child process group.
- Package locks reject duplicate `(name, version, source)` identities and
  distinguish version changes from source changes.
- Vendor destinations use full SHA-256 source identities.
- Native package paths reject rooted, prefixed, and parent components and
  canonicalize existing paths.
- Native package/shim preparation uses symlink-safe deterministic traversal,
  streaming copy/hash operations, file/depth/byte budgets, bounded Cargo output,
  deadlines, process-group termination, and a reduced deterministic environment.
  Native authorization remains full host-code execution, not sandboxing.
- LSP document edits, versions, and revisions are committed atomically;
  diagnostics publication does not hold the document mutex across an await.

## SQL

- SQLite and SQLx expose additive string-parameter APIs.
- Query results default to 10,000 rows and 16 MiB.
- SQLx queries stream rows rather than using `fetch_all`.
- The SQLx pool registry is capped at 32 URLs and supports `close` and
  `close_all`.
- The high-level SQLx facade exposes parameter and pool lifecycle APIs.

The current native ABI has no heterogeneous SQL value type, so parameter
binding is string-only. A single database cell is decoded by the driver before
the adapter can apply its byte ceiling.

## Test And Release Engineering

- Cargo commands that resolve dependencies in the all-test manifest use
  `--locked`, and the gate rejects `Cargo.lock` drift.
- Release artifacts now depend on a dedicated complete-workspace validation job
  covering runtime, LSP, vm-jit, native ABI/adapters, generated Rust backends,
  native-JIT, and self-host parity. Hardware/live-service tests are explicitly
  excluded rather than silently skipped.
- Seven generated Rust fixtures now share one workspace, lockfile, target
  directory, and Cargo process.
- Measured fixture-process count fell from seven to one. A controlled warm check
  fell from 4.87 seconds to 0.10 seconds; an immediate warm workspace check took
  0.17 seconds.
- The self-host corpus manifest includes the new SQLx FFI smoke fixture.

## Verification

The batch was validated incrementally with:

- `cargo fmt --all -- --check`
- `cargo check --workspace --locked`
- `cargo test -p vm-jit --lib` (123 passed)
- `cargo test -p rsscript --features native-jit --lib` (594 passed, 7 ignored)
- focused OSR, memoization, shape, admission, deopt, and performance-gate tests
- the complete release JIT performance gate (20 kernels)
- `cargo test -p reir`
- `cargo test -p rsscript-runtime` (207 passed)
- `cargo test -p rss-lsp`
- native SQLite and SQLx tests and VM smoke packages
- the consolidated generated-fixture locked workspace check

The final release performance gate passed with zero native bails. Against the
committed baseline, the largest collection improvements were:

| Kernel | Baseline | Final | Change |
| --- | ---: | ---: | ---: |
| Collection `is_empty` | 193.042 ms | 10.120 ms | -94.8% |
| Map/Set `len` | 32.345 ms | 3.068 ms | -90.5% |
| SortedSet `len` | 7.966 ms | 2.955 ms | -62.9% |
| Bytes slice/length | 3.757 ms | 5.405 ms | within gate; down from the 139 ms regression |

Scalar code remained within 2.7% of the committed baseline and the native call
chain within 7.8%.

Platform limitations remain explicit: PostgreSQL live smoke requires
`RSS_SQLX_TEST_POSTGRES_URL`; non-Unix descendant-process cleanup still needs a
platform job-object implementation; real Metal execution needs a macOS CI
runner.
