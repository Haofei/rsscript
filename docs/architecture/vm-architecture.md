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

> Note: the original design called for a push/pop stack machine that *truncates*
> the stack on return. The register VM (`src/reg_vm`) instead uses **fixed register
> windows on a shared, append-only register stack**. The actual convention is:

1. Each function has a statically known register count (`regs`); registers
   `base..base + regs` are its window. Parameters occupy the first slots; captures
   (for closures) precede parameters.
2. The caller picks the callee's `base` past its own live registers and copies the
   argument values into the callee's parameter slots, then pushes a frame
   (`function`, `ip: 0`, `base`).
3. `prepare_frame` grows the shared stack to `base + regs` if needed (it only ever
   **grows** — windows are reused in place, never truncated) and clears the
   per-register *written* bits for the new window.
4. `Return { src }` copies the value from the callee's `src` register into the
   caller's destination register and pops the frame. The stack is **not**
   truncated; the freed window is reused by the next call.

Each register carries a `written` bit, asserted on read/take, so a lowering bug
that reads an uninitialized slot (e.g. a stale value left in a reused window) fails
loudly instead of silently observing garbage.

User function and closure calls do not allocate a fresh argument vector — arguments
are written directly into the callee window. Runtime intrinsics may still use small
inline argument containers.

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
- a focused benchmark in `benchmarks/micro/`
- inclusion in `benchmarks/micro/run-matrix.sh` if it is part of the default VM
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
