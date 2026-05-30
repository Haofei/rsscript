# Development Rules

RSScript development is spec-first and self-hosted-validation-driven. The goal is not to
accumulate many small fixtures; the goal is to make the language capable of
reviewing and eventually implementing its own core tooling.

## Implementation Discipline

1. Implement the real prerequisite first.

   If a feature needs syntax, HIR facts, type inference, runtime support, or
   source-map behavior that does not exist yet, build that prerequisite before
   the feature. Do not encode a one-off lowering shortcut, fixture-only bypass,
   or runtime fallback that preserves a different language model.

2. Treat self-hosted validation and implementation as one loop.

   A useful self-hosted validation file should model a real RSScript tool concern: review-map
   classification, diagnostics, package contract review, source-map remapping,
   lowering facts, or package risk. If self-hosted validation reveals an unsupported shape, fix
   the parser/checker/lowering/runtime layer that is actually missing.

3. Do not treat unknown as safe.

   `unknown` is a product signal. A low unknown ratio on realistic self-hosted validation is
   evidence that the review protocol is working; a high unknown ratio is a
   language or analyzer gap. Review code must preserve unknown risk instead of
   folding it into low semantic risk.

4. Prefer fewer, harder tests.

   Tiny fixtures are useful for a specific diagnostic regression. They are not
   enough to prove the language. Every new semantic rule should either be
   covered by a focused negative fixture or be exercised by self-hosted validation that looks
   like real RSScript code.

5. Keep package tooling behind the language core.

   Package manager work should not outrun executable language semantics. When a
   package feature depends on checker, `.rssi`, lowering, source-map, or runtime
   behavior, stabilize that core behavior first.

6. No aliases or compatibility shims by default.

   RSScript is still pre-adoption. Prefer one canonical spelling and migrate
   all tests, examples, specs, and self-hosted validation code to it. Do not keep legacy aliases
   unless there is a current external compatibility contract.

7. Keep documents synchronized with the language model.

   The language specification is the authority for RSScript syntax and semantics;
   package-manager design documents consume that model instead of redefining it.
   README status sections should describe implemented commands, while future or
   design-only commands must be labeled as such. Prefer the canonical spellings
   used by the current tooling, including `--json` for machine-readable output
   and fully qualified `.rssi` symbols instead of namespace shorthands.

## Priority Order

1. Core spec invariants: features gating, named arguments, `read`/`mut`/`take`,
   `local`, `manage`, `fresh`, `retains`, resources, handles, weak handles, and
   runtime conflict behavior.
2. `.rssi` public contracts and package-local checking where they prove core
   semantic boundaries.
3. Rust lowering, source maps, and rustc diagnostic remapping for already
   supported source constructs.
4. Self-hosted validation programs that implement RSScript review/package/diagnostic logic in
   RSScript and keep review-map unknown low.
5. Review map and semantic diff quality.
6. Package manager surface area after the underlying language behavior is hard.

## Testing Loop

Use the broadest cheap signal first, then narrow only after a failure:

```sh
cargo test -q
```

If that fails, run the specific failing test while editing. After the fix, return
to the broad check instead of stacking many single-test passes.

Before committing a semantic change, run the full local gate:

```sh
cargo fmt --check
cargo clippy -q --workspace -- -D warnings
cargo test -q --workspace
bash scripts/check.sh
bash scripts/lint_sources.sh
bash scripts/run_examples.sh
bash scripts/run_selfhost.sh
git diff --check
```

For a release-like verification, use:

```sh
RSSCRIPT_FULL_TESTS=1 bash scripts/check.sh
```

CI sets `RSSCRIPT_FULL_TESTS=1 RSSCRIPT_E2E_TESTS=1`, so the same scripts run
the full workspace test suite, execute every `examples/*.rss` file, run the
ignored checker e2e tests, and run the checked-in self-hosted RSScript tools
through `rss run`.

The full gate defaults both Rust's test harness and the RSScript script runners
to twice the CPU count because many jobs spend substantial time on
generated-package and filesystem IO. Set `RSSCRIPT_TEST_THREADS=N` for
`cargo test`, `RUST_TEST_THREADS=N` for the raw libtest setting, or
`RSSCRIPT_JOBS=N` for script-runner fan-out when you need to cap concurrency or
reproduce a parallel-run issue.
Generated Rust packages share `target/rsscript-generated-target` during local
gates through `RSSCRIPT_GENERATED_TARGET_DIR`; keep that cache unless you are
debugging a clean backend build.

The generated target cache is disposable and can be moved to memory-backed
storage when local disk IO dominates test time. On macOS, create a 2 GiB
ramdisk and point the generated Cargo target there:

```sh
diskutil erasevolume HFS+ RSScriptRAMDisk "$(hdiutil attach -nomount ram://4194304)"
export RSSCRIPT_GENERATED_TARGET_DIR=/Volumes/RSScriptRAMDisk/rsscript-generated-target
RSSCRIPT_FULL_TESTS=1 bash scripts/check.sh
diskutil eject /Volumes/RSScriptRAMDisk
```

On Linux, `/dev/shm` is usually enough for the same cache:

```sh
mkdir -p /dev/shm/rsscript-generated-target
export RSSCRIPT_GENERATED_TARGET_DIR=/dev/shm/rsscript-generated-target
RSSCRIPT_FULL_TESTS=1 bash scripts/check.sh
```

The cache was about 380 MiB in the current local workspace; use a larger
ramdisk if you run the ignored e2e gate or want room for incremental rebuilds.

No individual checker integration test should exceed 10 seconds. Expensive
`rss run` / `rss verify-rust` tests are marked ignored in the normal Rust
harness. Run them through the slow-test audit when changing RSScript execution,
Rust verification, source-map remapping, or self-hosted scripts:

```sh
bash scripts/check_slow_tests.sh
```

Override the threshold or test-name pattern with
`RSSCRIPT_SLOW_TEST_THRESHOLD=N` and `RSSCRIPT_SLOW_TEST_PATTERN=...`.
The audit defaults to one job so it measures individual test time without Cargo
target-lock contention; set `RSSCRIPT_JOBS=N` only when you want faster wall
time rather than strict per-test timing. To include the ignored e2e tests in the
full gate, run `RSSCRIPT_FULL_TESTS=1 RSSCRIPT_E2E_TESTS=1 bash scripts/check.sh`.

Avoid running multiple workspace Cargo commands in parallel. Cargo's build lock
makes that slower and noisier. Independent RSScript script checks may run in
parallel after `rss` has been built; those jobs use isolated generated packages.
Use parallel shell reads/searches freely, but keep workspace Cargo verification
sequential.

## Why Not Always Run One Test First?

A single focused test is useful only after a broad command has identified the
failure or while editing a narrow regression. Starting every change with a
single test costs extra tokens and time because it still needs a full test pass
later. The default flow should be:

```text
broad quiet test -> inspect failure -> focused loop -> broad quiet test -> full gate
```

This keeps the evidence strong while avoiding repeated command output.
