# Embedded report pipeline

This is the primary embedding example for RSScript's platform-neutral product
boundary. A Rust host:

1. compiles one source/interface snapshot into verified bytecode;
2. runs that exact artifact with an in-memory filesystem and captured log;
3. runs it again with the real filesystem and stderr providers;
4. asserts that provider selection did not change the artifact bytes.

The script reads CSV-shaped text, creates an uppercase report, writes it, and
emits a log message. Its `.rssi` files describe only semantic external symbols;
the Rust composition root chooses implementations.

Run from the repository root:

```text
cargo run -p embedded-report-pipeline
```

The real-filesystem run is isolated in a temporary directory and cleans it up.
The printed SHA-256 identifies the provider-neutral bytecode artifact.
