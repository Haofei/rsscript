# Structured async pipeline

This is the second official RSScript embedding example. It demonstrates the
language semantics that are independent of host services:

1. an immutable source snapshot is compiled into a provider-neutral Artifact;
2. the Artifact is verified before the VM can execute it;
3. two `async let` children are owned by one `task_group` and consumed exactly
   once through `await`;
4. the host applies bounded execution limits and receives an
   `ExecutionReport`, including its termination reason and usage;
5. the *same linked Artifact* is run again with a host-owned cancellation
   token, producing a complete `Cancelled` report without executing a Provider;
   and
6. no Provider is registered, proving that structured async itself is a core
   language/runtime concern rather than an ambient host capability.

Run from the repository root:

```text
cargo run -p structured-async-pipeline
```

The same source also runs through the product-default isolated runner. This
does not inject any Provider—the program has no external imports—but it proves
the Artifact is built, re-verified in a child process, linked under the
fail-closed `no_providers` profile, and reported through the runner protocol:

```text
cargo run -p rsscript-cli --bin rss --features execution -- run examples/structured-async-pipeline/script/main.rss
```

The example's unit test runs both the successful and pre-cancelled execution
paths, so the report assertions are part of the workspace regression suite:

```text
cargo test -p structured-async-pipeline
```

For a Provider injection example, see
[`embedded-report-pipeline`](../embedded-report-pipeline/README.md). This
example's Rust embedding code deliberately uses the trusted in-process SDK
path. Neither route claims a universal sandbox: untrusted inputs require the
reference runner plus host-selected OS-level controls.
