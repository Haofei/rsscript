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

Before committing a semantic change, run the static local gate:

```sh
cargo fmt --check
cargo clippy -q --workspace -- -D warnings
cargo test -q --workspace
cargo run --quiet --bin rss -- run rss/test-runner -- rss/test-runner/manifests/check.rsstest.toml
cargo build --quiet --bin rss
target/debug/rss run rss/test-runner -- rss/test-runner/manifests/lint-sources.rsstest.toml
git diff --check
```

For the default full development gate, use:

```sh
cargo run --quiet --bin rss -- run rss/test-runner -- rss/test-runner/manifests/full.rsstest.toml
```

This full manifest must stay static-first. Do not add executable examples,
checked-in self-hosted tool runs, ignored checker tests, or any other e2e test
back into the default development loop. Static language, lowering,
package-review, and lint coverage carry development verification.

For package-manager dogfood work, use the focused TDD gate first:

```sh
cargo run --quiet --bin rss -- run rss/test-runner -- rss/test-runner/manifests/package-manager-tdd.rsstest.toml
```

This runs package-manager-focused static tests, the core JSON/lowering hooks
needed by the RSS implementation, and the checked-in Rayon package wrapper
facts. It intentionally avoids executable self-hosted package-manager sweeps.
It is the intended sub-10-second loop while iterating on
`rss/package-manager/main.rss`; run the full gate only before committing or when
touching shared lowering/runtime behavior.

Release/demo e2e tests live in a separate opt-in manifest:

```sh
cargo run --quiet --bin rss -- run rss/test-runner -- rss/test-runner/manifests/demo-e2e.rsstest.toml
```

These tests may build native demo binaries, start local mock servers, generate
temporary Rust packages, or run timing-sensitive demo clients. They must be
marked `#[ignore]` at the Cargo test level so `cargo test --workspace` and the
default full manifest remain static-first.

CI runs the RSScript test-runner full manifest. It is intentionally static-first:
it runs the unignored workspace test suite, generated-package compile checks,
and RSScript lint checks without executing every example, release demo, or
self-hosted tool as a behavior test.

Generated Rust package targets and temporary generated packages are disposable.
Default local development uses a memory-backed workspace. On macOS, `rss`
automatically creates or reuses `/Volumes/RSScriptRAMDisk` and derives
`rsscript-generated-target` and `rsscript-temp` under that root. Set
`RSSCRIPT_RAMDISK_GIB=N` to change the default macOS size.

You can still override the memory-backed root directly:

```sh
export RSSCRIPT_RAMDISK_PATH=/Volumes/RSScriptRAMDisk
```

On Linux, `/dev/shm` is usually memory-backed:

```sh
export RSSCRIPT_RAMDISK_PATH=/dev/shm/rsscript
```

On Windows, use a pre-mounted RAM disk drive or directory from your RAM disk
tool, then set the same variable in PowerShell:

```powershell
$env:RSSCRIPT_RAMDISK_PATH = 'R:\rsscript'
```

Windows does not provide a built-in, non-admin API for creating a true RAM disk,
so the repository only consumes a mounted RAM disk path instead of trying to
create one.

```sh
cargo run --quiet --bin rss -- run rss/test-runner -- rss/test-runner/manifests/full.rsstest.toml
```

Do not point these paths back at the SSD for normal development; if a test has
file conflicts, give it an isolated ramdisk subdirectory or copy the ramdisk
seed target, then clean the copy after the test.

No unignored e2e tests are allowed in this repository. Do not add default tests
that execute RSScript programs through `rss run`, drive `verify-rust` as an
end-to-end compiler invocation, sweep examples as behavior tests, run checked-in
self-hosted scripts as acceptance tests, build native demo binaries, or start
mock servers. When behavior needs coverage, test the parser, analyzer,
lowering, package metadata, source-map, runtime helper, or review function
directly. Any unignored test that exceeds 10 seconds must be deleted, split, or
rewritten as a smaller static/unit-level check. Release-grade demos may exist
only as ignored tests wired through `demo-e2e.rsstest.toml`.

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
