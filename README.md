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
rss build [--out <artifact.rssbc>] <file-or-package>
rss run [--json] <file-or-package> [-- <args>...]
rss run --aot <file-or-package> [-- <args>...]  # Experimental
rss inspect <imports|bytecode|analysis|resources|async|call-graph> <input>
```

Execution is bounded by step, memory, host-call, output, recursion, cancellation,
deadline, and child-process limits where applicable. Those controls are resource
limits, not a language authority model or a sandbox claim.

Frontend-only tools should depend on `rsscript-compiler` with its default
features; that closure contains no runtime or provider. Rust hosts enable its
`execution` feature to use the stable embedding surface: `Compiler`,
`CompiledPackage`, `Runtime`, `ProviderRegistry`, `RunLimits`, `Diagnostic`, and
`ExecutionReport`. VM registers, JIT plans, generated Rust source maps, and
review implementation types are not part of that embedding contract.

Provider authors should follow the versioned linkage, lifecycle, cancellation,
and conformance rules in [`docs/provider-sdk.md`](docs/provider-sdk.md).

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The normative language description is in
[`docs/spec/RSScript_v0.7_Spec.md`](docs/spec/RSScript_v0.7_Spec.md), and the layer
boundaries are summarized in
[`docs/architecture/ARCHITECTURE.md`](docs/architecture/ARCHITECTURE.md).
