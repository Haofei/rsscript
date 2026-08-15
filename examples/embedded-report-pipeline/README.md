# Embedded report pipeline

This is the primary embedding example for RSScript's platform-neutral product
boundary. A Rust host:

1. compiles one source/interface snapshot into verified bytecode;
2. runs that exact artifact with an in-memory filesystem and captured log;
3. runs it again with the real filesystem and stderr providers;
4. asserts that provider selection did not change the artifact bytes.

The script reads CSV-shaped text, converts it to uppercase, writes it as a
report, and emits a log message. Its `.rssi` files describe only semantic external symbols;
the Rust composition root chooses implementations.

Run from the repository root:

```text
cargo run -p embedded-report-pipeline
```

The real-filesystem Provider is explicitly rooted at a temporary directory and
rejects absolute, parent, and symlink escape paths. Rooting narrows filesystem
authority but is not a process-isolation boundary. The example cleans the
directory up after the run.
The output includes stable hashes for the provider-neutral Artifact and the
semantic `.rssi` descriptors. It also verifies that a bundle compared with
itself produces an empty `rsscript.semantic_diff.v2`, demonstrating that
interface contract evidence and Artifact identity are reviewable inputs rather
than provider-specific runtime state.

`script/isolated.rss` is a no-import companion for the reference runner. It
keeps the filesystem/log Provider selection in the trusted embedding path while
documenting the default isolated Artifact→report route separately:

```text
cargo run -p rsscript-cli --bin rss --features execution -- run --json \
  examples/embedded-report-pipeline/script/isolated.rss
```
