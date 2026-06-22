# Production auto-OSR performance matrix

Date: 2026-06-22
Source baseline: `benchmarks/vm-jit/baseline/baseline-prod-osr.json`
(`run-baseline.sh --iterations 7 --warmup 2`, auto-OSR default-on, so the
`jit-native` (`native`) column already exercises OSR).

Machine: Docker `dev` container on the rsscript dev host. All numbers are
**same-machine relative** (median ms), not absolute SPEC-style figures — use the
ratios, not the raw milliseconds, for cross-machine reasoning.

## OSR kernels (median ms)

Each kernel is a hot loop inside an I/O-tangled (`Log.write`-bracketed),
once-called function, so the whole function is native-INELIGIBLE; only OSR can
run the loop natively. `reg_vm` is the register VM (no JIT); `jit-native` is the
Cranelift native tier with auto-OSR on.

| category                | reg_vm ms | jit-native ms | speedup (reg_vm / native) |
| ----------------------- | --------: | ------------: | ------------------------: |
| osr-scalar              |   219.469 |         5.052 |                   43.44x |
| osr-option              |   209.248 |         4.042 |                   51.77x |
| osr-variant             |   238.666 |         5.434 |                   43.92x |
| osr-struct              |   234.645 |         3.052 |                   76.88x |
| osr-inline-variant      |   178.095 |         3.175 |                   56.10x |
| osr-closure             |   237.904 |        18.602 |                   12.79x |
| osr-multifield-variant  |   243.020 |         6.528 |                   37.23x |
| osr-float-closure       |   239.940 |        16.938 |                   14.17x |

## Aggregate

Across the 8 OSR kernels, auto-OSR (plain `jit-native`) speedups over the
register VM span **min 12.79x, median 43.68x, max 76.88x**. Auto-OSR fired on
all 8 kernels — every `nat/reg` ratio is between 0.01 and 0.08 (12x–77x faster),
confirming the trigger is genuinely default-on in the production `jit-native`
path for every targeted loop shape (scalar, scalar-Option, single-/multi-field
variants, flat struct, inline-leaf-call variant, and Int/Float capturing
closures).

The two capturing-closure kernels (`osr-closure` 12.79x, `osr-float-closure`
14.17x) win less than the value-loop kernels because their inlined native loop
still carries per-iteration closure-body arithmetic with a captured value rather
than a pure scalar/SROA loop — they OSR correctly but to a heavier native body.

Across the **whole suite** (46 kernels with both a `reg_vm` and a `jit-native`
median), the native tier beats the register VM on **32 / 46** kernels. The
non-wins are the expected cases where the native subset bails or the kernel is
dominated by collection/recursion/allocation work the native tier does not yet
cover (e.g. `recursion-tree`, `recursion-linear`, `closure-alloc`,
`list-closure`, `bytes`, `variant-combinator`).

### Notes / `n/a` cells

- `osr-closure` and `osr-float-closure` (and the existing `async`) report `n/a`
  in the **rust** column only: their `features: local` capturing-closure shape
  is not compiled by the generated-release-Rust path. The `reg_vm` and
  `jit-native` cells are valid, so the OSR comparison is unaffected.
- No OSR kernel hit the 180s per-mode cap; no OSR cell degraded to `n/a` for the
  `reg_vm` or `native` modes.
