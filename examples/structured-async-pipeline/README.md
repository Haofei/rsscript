# Structured async pipeline

This is the second official RSScript embedding example. It demonstrates the
language semantics that coordinate with an explicit, replaceable host service:

1. an immutable source snapshot is compiled into a provider-neutral Artifact;
2. the Artifact is verified before the VM can execute it;
3. two `async let` children are owned by one `task_group` and consumed exactly
   once through `await`;
4. `host.session.open` is declared in `.rssi`, compiled into the Artifact as a
   structural resource import, and implemented through generated typed Provider
   contract glue;
5. the resource is created and cleaned exactly once after the task group drains.
   RSScript deliberately rejects a resource that lives across an `await`, so the
   source makes that boundary visible instead of relying on runtime cleanup;
6. the host applies bounded execution limits and receives an
   `ExecutionReport`, including its termination reason and usage;
7. the *same linked Artifact* is run again with a host-owned cancellation
   token, producing a complete `Cancelled` report without executing a Provider;
8. a second run intentionally fails resource cleanup and retains cleanup counts
   in the execution report rather than losing the audit evidence; and
9. the Provider returns a generation-safe `WireValue::Resource` handle. The
   Artifact import and generated descriptor are asserted equal before linking,
   so a source-string or type-name mismatch fails before any instruction runs.
10. the exact same admitted Artifact is linked once with an in-memory Provider
    and once with a production-like Provider. Their host-side cleanup evidence
    differs while the script output and Artifact digest remain equal.

Run from the repository root:

```text
cargo run -p structured-async-pipeline
```

The example's unit test runs successful, pre-cancelled, and cleanup-failure
paths, so resource lifecycle and report assertions are part of the workspace
regression suite:

```text
cargo test -p structured-async-pipeline
```

The companion `script/isolated.rss` has the same structured-async task shape
without an external import. It is intentionally runnable through the reference
runner's fixed no-Provider profile, which verifies the production
Artifact→runner→report path without permitting request-selected Provider code:

```text
cargo run -p rsscript-cli --bin rss --features execution -- run --json \
  examples/structured-async-pipeline/script/isolated.rss
```

For a Provider injection example, see
[`embedded-report-pipeline`](../embedded-report-pipeline/README.md). This
example deliberately uses the trusted in-process SDK path because the reference
runner's fixed profiles do not install this example-only Provider; the companion
fixture demonstrates the isolated path. Neither route claims a universal
sandbox: untrusted inputs require the reference runner plus host-selected
OS-level controls.
