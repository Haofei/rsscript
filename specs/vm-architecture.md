# RSScript VM Architecture Baseline

This document is the implementation baseline for the VM path. The VM should not
grow broad new language/runtime feature coverage until the execution skeleton
below is in place and covered by benchmarks.

## Current Goal

Move from the temporary recursive VM toward a frame-based VM that can become the
main interpreter replacement:

- one shared operand/local stack
- explicit call frame stack
- no per-call function cloning
- no per-call argument Vec allocation for common arities
- benchmark coverage for every major VM feature
- parity checks against the HIR interpreter for supported features

## Execution Model

The VM should execute a prepared `VmExecutable`:

```text
VmExecutable
  functions: Vec<Rc<VmFunction>>
  function_ids: HashMap<String, FunctionId>
```

The runtime VM should own:

```text
Vm
  stack: Vec<VmValue>
  frames: Vec<VmFrame>
  args: Vec<String>
  stdout: String
```

A frame records where a function lives on the shared stack:

```text
VmFrame
  function: Rc<VmFunction>
  ip: usize
  base: usize
```

`base` is the index of local slot 0 in `stack`. Function arguments are already in
the first local slots. Extra locals are initialized to `Unit` by extending the
shared stack to `base + local_count`.

## Calling Convention

The stable calling convention should be:

1. Caller evaluates arguments left-to-right and leaves them on the shared stack.
2. `CallUser(function, argc)` computes `base = stack.len() - argc`.
3. VM pushes `VmFrame { function, ip: 0, base }`.
4. VM extends `stack` to hold all locals.
5. `Return` pops the return value, truncates stack to `base`, pops the frame, and
   pushes the return value for the caller.

Closure calls use the same mechanism, with captures placed before closure
parameters in the callee local layout.

Runtime intrinsics may still use small inline argument containers for now, but
user function and closure calls should not allocate a fresh argument vector.

## Feature Gate

Do not expand VM language/runtime feature coverage until this skeleton exists:

- explicit `VmFrame`
- single shared VM stack for locals and operands
- non-recursive user-function call/return
- closure calls routed through the same frame machinery
- existing benchmark matrix still passing

After the skeleton lands, add features in groups only when each group has:

- interpreter parity tests
- VM coverage accounting
- a focused benchmark in `benchmark/`
- inclusion in `benchmark/run-matrix.sh` if it is part of the default VM
  performance story

## Benchmark Gates

Use release-built benchmark driver commands:

```sh
cargo run --quiet --release --bin rss -- bench --json --mode vm-internal <file> -- <size>
cargo run --quiet --release --bin rss -- bench --json --mode release-internal <file> -- <size>
```

The matrix should compare `vm-internal` with `release-internal`. Programs used
for release comparison must include data dependencies that prevent the generated
Rust backend from optimizing the whole workload into a closed-form result.

## Migration Order

1. Keep the current minimal VM feature set stable.
2. Introduce `VmFrame` and shared stack call/return for ordinary user functions.
   This is implemented for `CallUser`; the run loop now pushes frames instead of
   recursively invoking Rust for ordinary user-function calls.
3. Route closure calls through the same frame machinery. This is implemented:
   `call_closure` now pushes captures and call arguments onto the shared stack,
   pushes the closure frame, and runs until the previous frame depth. It no
   longer materializes a temporary argument vector for captured closures.
4. Remove recursive `run_function` as the normal execution path. Ordinary calls
   are non-recursive. Closure calls from runtime intrinsics still re-enter the
   same frame-loop so map/filter/fold can synchronously receive callback results;
   making collection iteration itself frame-native remains a later runtime
   state-machine step.
5. Re-run the benchmark matrix and record the new ratios.
6. Resume feature migration only after this architecture is stable.
