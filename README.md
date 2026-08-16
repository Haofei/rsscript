# RSScript

RSScript is a platform-neutral constrained language with explicit ownership,
retention, resource-lifetime, and structured asynchronous semantics. Parsing,
validation, lowering, and generated programs do not depend on operating-system
permissions, deployment grants, runner policy, or REIR.

The language keeps the correctness concepts that affect program meaning:

- `read`, `mut`, and `take` parameter data effects;
- `retains(param)` escape contracts;
- `fresh`, `noescape`, and `owned` type qualifiers;
- `local` values and `manage`;
- `resource` values and `with` scopes;
- handle/weak reference rules;
- `async`, `await`, `task_group`, channels, cancellation, and bounded execution.

The source language deliberately has no file feature header, deployment profile,
generic effect list, `native` declaration marker, source-level unsafe boundary, or
host-permission type. Dynamic protocol dispatch is written as `Dyn<P>`.

```rsscript
protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        retains(message)
}

fn append<W: Writer>(writer: mut W, message: read String) -> Unit
    retains(message)
{
    Writer.write(self: mut writer, message)
}
```

## Host integration

Filesystem, environment, process, network, wall-clock, entropy, logging, and
command-line services are not default core APIs. A package that needs a host
service declares an ordinary bodyless function in a `.rssi` interface and maps
that symbol to a runtime provider with binding metadata. `.rss` implementation
files contain source bodies; `.rssi` files contain declarations.

Runner arguments cross the entry ABI explicitly: a program may declare either
`fn main()` or `fn main(args: read List<String>)`. `Arguments.*` helpers operate
only on that list; the compiler and VM never read ambient process arguments.

```rsscript
module host.fs

resource File

pub fn open_read(path: read String) -> Result<File, FsError>
pub fn read_all(file: mut File) -> Result<fresh Bytes, FsError>
```

```toml
schema = "rsscript.bindings.v1"

[[function]]
symbol = "host.fs.open_read"
provider = "rsscript_host_fs"
entry = "file_open_read"
```

The compiler records external symbols. The runner chooses providers at execution
time through the external-function registry. Provider choice does not change the
compiled program. Optional review tooling may combine binding/provider metadata
with the validated call graph; its conclusions never change language validity.

## CLI

```text
rss check <file-or-package>
rss fmt <file>
rss build [--out <artifact.rssbundle>] [--analysis-out <analysis.json>] <file-or-package>
rss verify <artifact.rssbundle>
rss diff [--json|--markdown] <old-input> <new-input>
rss profile [--json] [profile-name]
rss run [--json] [--profile <profile-name>] <file-package-or-bundle> [-- <args>...]  # isolated process
rss run --trusted-in-process [--json] <file-package-or-bundle> [-- <args>...]
rss inspect <imports|bytecode|analysis|resources|async|call-graph> <input>
```

The product CLI execution build contains only the verified VM and isolated
runner path. Rust AOT is an experiments-workspace backend and is not selectable
through `rss run`.

Building a package captures one immutable workspace snapshot. Every build emits
a versioned Artifact Bundle containing verified bytecode, neutral analysis,
provenance, and exact interface requirements. `--analysis-out` optionally writes
the embedded analysis as a separate JSON file. `rss diff` compares semantic
facts without making an allow/deny decision or producing a risk score.

The default Cargo feature set builds the frontend-only `check`, `fix`, and
`fmt` path without runtime dependencies. Build the CLI with `--features
execution` to enable `build`, `run`, `inspect`, and package execution tooling.

Execution is bounded by step, memory, host-call, output, recursion, cancellation,
deadline, and child-process limits where applicable. Those controls are resource
limits, not a language authority model or a sandbox claim.

`rss run` uses the experimental reference isolated runner by default. It creates
a separately bounded child process, re-verifies the Artifact Bundle inside that
process, and links only Providers installed by the runner profile. This process
boundary is defense in depth, not a claim that the VM itself is a security
sandbox. Trusted hosts can opt into same-process execution explicitly with
`--trusted-in-process`.

Frontend tooling uses `rsscript-compiler` with its default features; that
closure contains no runtime or provider. Rust hosts depend on `rsscript-sdk`
and enable its `execution` feature to use the stable embedding surface: `Compiler`,
`BuiltArtifact`, `VerifiedArtifact`, `Runtime`, `LinkedArtifact`,
`ProviderRegistry`, `ExecutionRequest`, `RunLimits`,
`Diagnostic`, and `ExecutionReport`. VM registers, JIT plans, generated Rust source maps, and
review implementation types are not part of that embedding contract.

Provider authors should follow the versioned linkage, lifecycle, cancellation,
and conformance rules in [`docs/provider-sdk.md`](docs/provider-sdk.md).

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

These are the Core workspace checks. AOT, native JIT, REIR, and self-hosting
are isolated experiments with their own manifests and must not be enabled by a
normal Core verification command.

The normative language description is in
[`docs/spec/RSScript_v0.7_Spec.md`](docs/spec/RSScript_v0.7_Spec.md), and the layer
boundaries are summarized in
[`docs/architecture/ARCHITECTURE.md`](docs/architecture/ARCHITECTURE.md).
