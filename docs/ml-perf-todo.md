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

- [ ] **Slice 1 — `matmul` kernel** (the headline). Native cache-blocked matmul
  via the `matrixmultiply` crate (pure Rust, no system-BLAS dep; add `rayon` for
  large sizes). Expose as a runtime intrinsic; route `Tensor.matmul` to it.
  _Verify:_ VM↔compiled parity + a benchmark showing the VM big-matmul cliff is
  gone (e.g. 512×512, 1024×1024 before/after).
- [ ] **Slice 2 — elementwise kernels.** Binary (`add/sub/mul/div`) and unary
  (`neg/exp/log/sqrt/relu/…`) over the packed buffer, as intrinsics; route the
  framework's elementwise ops to them. Keep results bit-identical to the current
  path. _Verify:_ parity + big-tensor benchmark.
- [ ] **Slice 3 — reductions.** `sum/max/mean` (and argmax) over axes as native
  kernels. _Verify:_ parity + benchmark.
- [ ] **Slice 4 — movement ops zero-copy.** `reshape/permute/expand/broadcast`
  via shape+strides on the shared buffer (no copy) if not already. _Verify:_
  parity.
- [ ] **Slice 5 — no per-op marshaling.** Hold tensor data as native buffers
  (`Rc<Vec<f32>>`) across ops so a chain of ops doesn't convert the whole buffer
  to/from `VmValue` each call (matters for elementwise on big tensors; matmul is
  compute-bound so this is secondary there). _Verify:_ parity + benchmark.

_Why this is the priority:_ it removes the big-matrix VM cliff directly, and it
demotes the 3-min AOT off the dev hot loop (you iterate on the now-fast VM; AOT
becomes a ship/verify step).

## P2 — AOT build time (secondary; for ship/CI, not the inner loop)

Confirmed: the generated `Cargo.toml` has **no profile tuning** (only
`[profile.release] overflow-checks = true`).

- [ ] **Tuned generated build profile.** Add a "fast-ish" build profile to the
  generated crate (`opt-level=1/2`, `codegen-units=256`, `lto=off`,
  `incremental=true`) so ship/verify builds aren't full-fat release every time.
  Quick win.
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
