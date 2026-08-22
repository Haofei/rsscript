# Development Rules

RSScript development is spec-first and product-contract-driven. The goal is not
to accumulate fixtures or backend experiments; it is to keep one small,
deterministic compilation and execution path correct and auditable.

## Implementation Discipline

1. Implement the real prerequisite first.

   If a feature needs syntax, HIR facts, type inference, runtime support, or
   source-map behavior that does not exist yet, build that prerequisite before
   the feature. Do not encode a one-off lowering shortcut, fixture-only bypass,
   or runtime fallback that preserves a different language model.

2. Treat self-hosted validation as Research evidence, not as a product driver.

   A useful self-hosted validation file may model a real RSScript tool concern:
   diagnostics, source-map remapping, lowering facts, or package analysis. It
   must not expand source syntax, Core ABI, or the formal release path merely to
   improve self-host parity.

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
4. Semantic-fact and execution-report quality.
5. Provider conformance and isolated-runner boundary hardening.
6. Research validation only when it protects an existing Core invariant.

## Testing Loop

Use the pinned toolchain and locked dependency graphs. The supported local gate
mirrors Core CI:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked -p rsscript-sdk --features execution
cargo test --locked -p rsscript-cli --features execution
cargo run --locked -p rsscript-xtask -- validate-ci
git diff --check
```

The SDK has explicit integration-test targets because `autotests = false`.
`validate-ci` checks workflow package, feature, and test references against
Cargo metadata and rejects unregistered top-level SDK test files. A filtered
test is useful only after the containing registered target has proved that the
filter matches real coverage.

Providers and the runner have focused gates:

```sh
cargo test --locked -p rsscript-provider-api
cargo test --locked -p rsscript-provider-conformance
cargo test --locked -p rsscript-provider-fs
cargo test --locked -p rsscript-provider-env
cargo test --locked -p rsscript-provider-process
cargo test --locked -p rsscript-provider-http
cargo test --locked -p rsscript-runner-protocol
```

The separate experiments workspace consumes Core contracts but is not a Core
release dependency:

```sh
cargo clippy --locked --manifest-path experiments/Cargo.toml --workspace --all-targets --all-features -- -D warnings
cargo test --locked --manifest-path experiments/Cargo.toml --workspace --all-features
```

Native JIT is an explicit trusted-host SDK mode. It is absent from default
closures and requires both correctness and performance evidence:

```sh
cargo test --locked -p rsscript-sdk --features native-jit
cargo test --locked --release -p rsscript-sdk --features native-jit --test native_jit_smoke native_hot_loop_release_gate_beats_the_interpreter -- --nocapture
```

Fuzz targets are maintained in their own manifest:

```sh
cargo check --locked --manifest-path fuzz/Cargo.toml --bins
```

Avoid concurrent workspace Cargo commands because their build lock makes the
feedback slower. Use a focused test during diagnosis, then return to the broad
gate. Long-running or platform-specific hardening belongs in its dedicated
workflow, not in an invented hidden SDK target.

## Why Not Always Run One Test First?

A single focused test is useful only after a broad command has identified the
failure or while editing a narrow regression. Starting every change with a
single test costs extra tokens and time because it still needs a full test pass
later. The default flow should be:

```text
broad quiet test -> inspect failure -> focused loop -> broad quiet test -> full gate
```

This keeps the evidence strong while avoiding repeated command output.
