# RSScript

RSScript is a small scripting language for scripts that run inside Rust
applications. It is ownership-aware: every parameter states whether the call
reads, mutates, or consumes its argument, and the checker proves those claims
before the program can run. Compilation depends on nothing but the source and
the interfaces it declares, so one build produces one provider-neutral,
verified Artifact that any host can run.

It is built for code that a machine writes and a host has to trust: an agent
generates a script, the compiler answers precisely what is wrong with it, and
the host runs the resulting Artifact under limits it chose.

- **Explicit ownership and effects.** `read`, `mut`, and `take` parameter data
  effects; `retains(param)` escape contracts; `fresh`, `noescape`, and `owned`
  type qualifiers; `local` values and `manage`; `resource` values and `with`
  scopes; handle and weak-reference rules.
- **Structured concurrency.** `async`, `await`, `task_group`, channels,
  cancellation, and bounded execution. Dynamic protocol dispatch is written as
  `Dyn<P>`.
- **Provider-neutral artifacts.** Host services are ordinary declarations in a
  `.rssi` interface, bound to a runtime provider at load time. The compiled
  program is identical whichever provider the host installs.
- **Bounded execution.** The bytecode interpreter is the reference execution
  model, bounded by step, memory, host-call, output, recursion, cancellation,
  deadline, and child-process limits where applicable.
- **Opt-in native speed.** A trusted host can enable the Cranelift JIT for
  CPU-bound work. It runs the same verified bytecode, reports through the same
  Execution Report, and falls back to the interpreter for regions it does not
  compile.

## A script and its interface

```rsscript
// script/main.rss - the program
module report.pipeline

use host.fs.*
use host.log.*

fn main() -> Unit {
    let input = read_text(path: read "input.csv")
    let report = String.to_uppercase(value: read input)
    write_text(path: read "report.txt", text: read report)
    emit(message: read report)
    return Unit
}
```

```rsscript
// interfaces/fs.rssi - what the host must supply
module host.fs

pub fn read_text(path: read String) -> String
pub fn write_text(path: read String, text: read String) -> Unit
```

`.rss` files hold implementations: every ordinary function has a body. `.rssi`
files hold interfaces: their functions are bodyless declarations, and each one
is an external symbol the host resolves. A binding file maps that symbol to a
provider entry point:

```toml
schema = "rsscript.bindings.v1"

[[function]]
symbol = "host.fs.open_read"
provider = "rsscript_host_fs"
entry = "file_open_read"
```

The compiler records the external symbols; the runner picks the implementations
at execution time through the external-function registry. Review tooling may
combine binding and provider metadata with the validated call graph, and its
conclusions stay evidence: they do not change whether a program is valid.

Programs take their arguments explicitly. A script declares either `fn main()`
or `fn main(args: read List<String>)`, and the `Arguments.*` helpers operate
only on that list, so the compiler and VM never read ambient process arguments.

## Run it

```bash
cargo run -p rsscript-cli --bin rss --features execution -- run examples/scripts/basic/hello.rss
cargo run -p embedded-report-pipeline   # the script above, driven from a Rust host
```

```text
rss check <file-or-package>
rss fmt <file>
rss generate prefix-status [--json] <file>
rss generate continuations [--json] [--no-core] [--interface <file.rssi>] [--max-names <n>] <file>
rss build [--out <artifact.rssbundle>] [--analysis-out <analysis.json>] <file-or-package>
rss verify <artifact.rssbundle>
rss diff [--json|--markdown] <old-input> <new-input>
rss profile [--json] [profile-name]
rss run [--json] [--profile <profile-name>] <file-package-or-bundle> [-- <args>...]  # isolated process
rss run --trusted-in-process [--native] [--json] <file-package-or-bundle> [-- <args>...]
rss inspect <imports|bytecode|analysis|resources|async|call-graph> <input>
```

The default Cargo feature set builds the frontend-only `check`, `fix`, and
`fmt` path with no runtime dependencies. Build the CLI with
`--features execution` to get `build`, `run`, `inspect`, and the package
execution tooling; that build contains the verified VM and the isolated runner.

Building a package captures one immutable workspace snapshot and emits a
versioned Artifact Bundle holding verified bytecode, neutral analysis,
provenance, and exact interface requirements. `--analysis-out` also writes the
embedded analysis as standalone JSON. `rss diff` reports how the semantic facts
changed between two inputs; it produces no allow/deny decision and no risk
score.

`rss check --json`, `rss fix --json`, and `rss generate` are the machine-facing
side of the same frontend: they report what the current source and interfaces
permit as structured facts, so a code-generating agent can iterate on a script
from diagnostics it can parse. That contract is described in
[`docs/product.md`](docs/product.md).

## Architecture

```text
 .rss + .rssi ──▶ syntax ──▶ semantics ──▶ compiler ──▶ Artifact Bundle
 source           parse       ownership,     lowering    verified bytecode
 snapshot         + spans     types,         to MIR      + neutral analysis
                              retention,                 + provenance
                              async                          │
                                                             ▼
                                                    verify + link Providers
                                                             │
                              ┌──────────────────────────────┴───────────┐
                              ▼                                          ▼
                    isolated runner (default)              in-process VM (trusted)
                    child process, re-verifies,            bytecode interpreter
                    runner-profile Providers               └─ opt-in Cranelift JIT
                              └──────────────┬───────────────────────────┘
                                             ▼
                                     Execution Report
```

Execution reports identify the artifact, give a structured termination reason,
and record steps, allocation, storage, output, intrinsic and Provider calls, and
resource cleanup, as the versioned `rsscript.execution_report.v2` schema.

**Trust boundary.** RSScript validates language and runtime invariants; it does
not make a script, Provider, native plugin, or generated program trustworthy,
and the in-process runtime is not a sandbox. Its execution limits are
availability controls, not permissions. So `rss run` defaults to the reference
isolated runner: a separately bounded child process that re-verifies the
Artifact Bundle and links only the Providers its runner profile installs —
defense in depth around code the host does not fully trust. A host that already
trusts the script opts into same-process execution with `--trusted-in-process`,
and may add `--native` there — the flag is available only in that combination —
for the Cranelift JIT, which uses the same verified
Artifact, Provider linker, and Execution Report as the interpreter. Native
execution is deliberately unavailable to the isolated runner: native regions
report the interpreter's exact step count whether or not a limit is armed and
stop for the same cancellation and deadline reasons, but allocation controls
need a per-region proof, the intrinsic-call budget is still interpreter-owned,
and the internal call ABI does not yet carry the language's `max_depth`, so the
bounded runner keeps the interpreter until those gaps close (status in
`docs/spec/native-jit-contract.md`). An Artifact can never request it: enabling the JIT
is the host's decision alone. Providers are trusted host code with the full
authority of the process, and JIT-generated executable memory is not an
isolation boundary. The full boundary analysis is in
[`docs/threat-model.md`](docs/threat-model.md).

## Embedding

Frontend queries use the syntax and semantics crates with no runtime or
Provider dependencies. Compilation consumers use `rsscript-compiler`. Rust hosts
depend on `rsscript-sdk` and enable its `execution` feature for the stable
embedding surface: `Compiler`, `BuiltArtifact`, `VerifiedArtifact`, `Runtime`,
`LinkedArtifact`, `ProviderRegistry`, `ExecutionRequest`, `RunLimits`,
`Diagnostic`, and `ExecutionReport`. VM registers, JIT plans, generated Rust
source maps, and review implementation types stay internal to their crates.

Provider authors should follow the versioned linkage, lifecycle, cancellation,
and conformance rules in [`docs/provider-sdk.md`](docs/provider-sdk.md).

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

These are the Core workspace checks. Rust AOT, REIR, and self-hosting research
are archived on the `archive/experiments-2026-09` branch. The native JIT is a required trusted-host performance tier of the
VM under active development; it is feature-gated out of the default Core
verification closure, and its correctness and workload evidence run separately
(see [`docs/roadmap.md`](docs/roadmap.md)).

## Where to go next

- [`docs/README.md`](docs/README.md) — the documentation index.
- [`docs/spec/RSScript_v0.7_Spec.md`](docs/spec/RSScript_v0.7_Spec.md) — the
  normative language description.
- [`docs/architecture/ARCHITECTURE.md`](docs/architecture/ARCHITECTURE.md) —
  layer boundaries and dependency rules.
- [`docs/product.md`](docs/product.md) — who the product is for and what it
  guarantees.
- [`language card`](docs/generated/language-card.md) — the generated,
  source-backed quick reference, with machine-readable companions
  [`language-card.json`](docs/generated/language-card.json),
  [`grammar.json`](docs/generated/grammar.json),
  [`diagnostic-catalog.json`](docs/generated/diagnostic-catalog.json), and
  [`core-interfaces.json`](docs/generated/core-interfaces.json). Refresh them
  with `cargo run -p rsscript-xtask -- language-card` and verify them with
  `--check`.
