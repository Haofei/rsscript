# ML framework performance TODO

Focused work to make RSScript viable as the runtime for the (already-built) ML
framework. Driven by two measured findings:

- **VM is fast on small loads but slow on big matrix calculations.**
- **AOT-compiled code runs fast, but the build takes ~3 minutes.**

## Root cause (diagnosis)

Both findings trace to **one missing layer**: there are **no native tensor
kernels** in the tree (no `matmul`/`gemm`/`ndarray` in `crates/runtime/src` or
`stdlib`). So the framework's heavy math (matmul, large reductions) is written in
RSScript and:

- **Interpreted element-by-element by the reg-VM** → an n×n matmul is n³ bytecode
  multiply-adds → fine on small inputs, falls off a cliff on big matrices.
- The only way to get speed is to **AOT-compile the whole port** → the ~3-min
  build is on the dev hot path.

The fix is one layer that removes both: back the hot ops with native kernels so
the VM *orchestrates* and native code *computes* (like Python→C++/CUDA in real
frameworks). Both backends call the same kernel ⇒ VM↔compiled parity is automatic.

## P1 — Native tensor kernels (the lever; fixes both findings)

Mechanism: a packed buffer value (`Vec<f32>` + shape/strides + dtype) plus a
family of **runtime intrinsics** wired the same way as existing native intrinsics
(`runtime_abi` maps `Ns.method → rsscript_runtime::fn`, dispatched in the reg-VM
and lowered by `rust_lower`). The framework's existing RSScript `Tensor` API is
re-pointed at these kernels — additive, the surface stays the same.

- [x] **Slice 1 — `matmul` kernel** (the headline). DONE: native `RssTensor`
  (`Rc<Vec<f32>>` + shape) runtime-backed value type mirroring `Channel`, working
  in both reg-VM and AOT; hand-written `ikj` f32 matmul (no new deps — optimized
  `matrixmultiply`/`rayon` deferred to P3) + `from/to_f32_slice` + `shape`/`rank`.
  Both backends call the identical `rsscript_runtime::tensor_*` kernels, so parity
  is bit-identical. (`packages/ml/interface/tensor.rssi`.)
- [x] **Slice 2 — elementwise kernels.** DONE: `Tensor.add/sub/mul/div`
  (same-shape) and `neg/exp/log/sqrt/relu` as native intrinsics, parity-tested
  (incl. 256×256 bulk). Broadcasting handled via `broadcast_to` (slice 4).
- [x] **Slice 3 — reductions.** DONE: `sum_all` (global→scalar) +
  `sum_axis/max_axis/mean_axis/argmax_axis` over an axis, native, parity-tested.
- [x] **Slice 4 — movement ops.** DONE: `reshape` (true zero-copy via `Rc::clone`)
  + `transpose/permute/broadcast_to` (materializing, contiguous-preserving).
  `RssTensor` deliberately stays row-major contiguous (no `strides` field) so the
  matmul/elementwise/reduce kernels stay simple; true zero-copy strided views are
  a future optimization. Parity-tested.
- [x] **Slice 5 — no per-op marshaling.** SATISFIED BY DESIGN (slice 1): tensors
  flow as `VmValue::Native` handles (an id into the executor's `tensors` map), and
  buffers are `Rc<Vec<f32>>`. A chain of ops passes handles + `Rc`-clones — the
  full buffer is only converted to/from `VmValue` at the `from_f32_slice` /
  `to_f32_slice` boundary, never per-op.

_Why this is the priority:_ it removes the big-matrix VM cliff directly, and it
demotes the 3-min AOT off the dev hot loop (you iterate on the now-fast VM; AOT
becomes a ship/verify step).

## P2 — AOT build time (secondary; for ship/CI, not the inner loop)

Confirmed: the generated `Cargo.toml` has **no profile tuning** (only
`[profile.release] overflow-checks = true`).

- [x] **Tuned generated build profile.** DONE: generated `Cargo.toml` now sets
  `[profile.release]` `codegen-units=256, lto=false, incremental=true` (keeping
  `overflow-checks=true`) and a `[profile.dev]` `opt-level=1, incremental=true,
  codegen-units=256`. (`crates/rsscript/src/rust_lower/mod.rs`.)
- [ ] **Per-module split of the generated package.** The port lowers to one huge
  generated `lib.rs`; any edit recompiles the whole crate. Emit one module/file
  per source unit so rustc's incremental units are small → a one-file edit
  recompiles one module, not everything.
- [ ] **(optional) Cranelift codegen backend for dev builds** — 2–5× faster debug
  compiles; nightly/component dependency, evaluate.
- [x] _Done:_ unchanged-source run cache (`fix/aotcache`) — repeated `rss run` of
  unchanged source skips re-lowering (~46× on a warm run).

## P3 — Later / follow-ons

- [ ] **Parallelism + SIMD in kernels** (`rayon` over rows/tiles; explicit SIMD
  or rely on autovectorization) once the kernel layer exists.
- [ ] **Kernel fusion** — fuse elementwise chains into single loops, and
  eventually a fused-kernel codegen/JIT (tinygrad-style scheduler). This is the
  real long-term AOT story for ML: *generate fast kernels*, not *compile the crate
  faster*.
- [ ] **GPU/accelerator backend** for kernels (far future).

## Explicitly NOT worth doing

- **`List.fold`/`map` numeric recognizer extensions** in the reg-VM. They speed up
  boxed-`List<Float>` closures, but ML tensor math should go through the native
  Tensor kernels above, not per-element list closures. The existing `List.fold`
  fast path stays (harmless), but don't invest further there.

## Verification discipline (every kernel slice)

- Hold **VM↔compiled parity** (`vm_eval_parity`): both tiers call the same kernel,
  so results must be bit-identical.
- Add a **benchmark** proving the win (big matmul / big elementwise, VM before vs
  after) — a perf claim without a measurement doesn't land.
- Build/test in the Docker dev container; merge each slice only when green.
