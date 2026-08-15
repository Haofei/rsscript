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

The example's unit test runs both the successful and pre-cancelled execution
paths, so the report assertions are part of the workspace regression suite:

```text
cargo test -p structured-async-pipeline
```

For a Provider injection example, see
[`embedded-report-pipeline`](../embedded-report-pipeline/README.md). This
example deliberately uses the trusted in-process SDK path. It does not claim
process isolation or sandboxing; untrusted inputs must use the reference
runner and OS-level controls.
