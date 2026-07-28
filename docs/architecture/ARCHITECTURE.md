# RSScript Architecture

This repository is organized around one product boundary:

```text
RSScript source -> frontend semantics -> review metadata -> Rust lowering
```

The language core stays ahead of package-manager surface area. Package tooling
may exercise the language, but it must not redefine language semantics.

## Layers

1. Syntax

   `src/syntax/` owns parsing and source-preserving AST shapes. It should not
   know package policy, review risk, Rust lowering, or runtime hooks.

2. HIR and Frontend Checks

   `src/hir.rs`, `src/analyzer.rs`, and `src/checks/` own semantic facts and
   RSScript diagnostics: features, named arguments, data effects, local/fresh
   state, resource lifetime, weak/handle access, and unsupported executable
   surfaces.

3. Review Protocol

   `src/review.rs` owns review-map and semantic diff classification. Unknown is
   preserved as a first-class product signal and is never folded into low risk.

4. Rust Lowering

   `src/rust_lower/` owns generated Rust, source maps, backend verification,
   rustc remapping, runtime diagnostic parsing, and runtime hook selection.
   Lowering consumes checked RSScript semantics; it should not be the first
   place that discovers language errors.

5. Package Tooling

   `src/package/` owns package manifests, `.rssi` public contracts, dependency
   graphs, semantic locks, package review, publish dry-runs, vendoring, and
   package metadata. It consumes syntax/check/review/lowering APIs.

6. Runtime

   `runtime/` owns the reference runtime ABI: managed handles, weak handles,
   resources, diagnostics, and core native hooks used by lowered Rust.

7. CLI

   `src/main.rs` is only the process entrypoint. `src/cli/` owns command-line
   parsing and command dispatch, and should remain an application shell around
   the library APIs.

## Current Hotspots

These files are intentionally tracked as refactoring targets:

```text
tests/static.rs          non-executing frontend/lowering/package checks
tests/runtime.rs         fast VM/runtime/JIT behavior checks
tests/differential.rs    backend agreement, corpus, generative, metamorphic checks
tests/soak.rs            slow demo, benchmark, release-shaped checks
tests/checker_frontend.rs   frontend checker, parser, diagnostics, and fixtures module
tests/checker_lowering.rs   Rust lowering, source maps, runtime diagnostics module
tests/checker_package.rs    package review, package manager, REIR adapters module
tests/checker_review.rs     review map and semantic diff behavior module
src/package/           package domain model, graph, review, lock, publish, vendor
src/rust_lower/        lowering, backend checks, source maps, remapping, intrinsics
src/analyzer.rs        frontend orchestration
src/checks/*.rs        large semantic checker implementations
```

## Refactoring Order

1. Keep `src/main.rs` thin and move CLI application code under `src/cli/`.
2. Split package code by responsibility:

   ```text
   src/package/source_set.rs
   src/package/native.rs
   src/package/types.rs
   src/package/graph.rs
   src/package/review.rs
   src/package/lock.rs
   src/package/diff.rs
   src/package/publish.rs
   src/package/vendor.rs
   src/package/format.rs
   ```

   Current completed package splits:

   ```text
   src/package/types.rs       public package/review/lock data shapes
   src/package/check.rs       package check aggregation and semantic lock validation
   src/package/dependency.rs  path dependency specs, interface loading, feature resolution
   src/package/format.rs      JSON/TOML/human output formatting
   src/package/source_set.rs  rsspkg.toml loading and source/interface selection
   src/package/native.rs      native binding metadata and native Rust risk checks
   src/package/policy.rs      review policy parsing and enforcement helpers
   src/package/review.rs      package review aggregation, API summary, and risk scoring
   src/package/contract.rs    .rssi public contract extraction and comparison
   src/package/lock.rs        semantic lockfile, package archive, and hash logic
   src/package/graph.rs       dependency tree and package graph validation
   src/package/diff.rs        manifest/interface semantic diff and risk rules
   src/package/metadata.rs    package review metadata and lowering input assembly
   src/package/vendor.rs      path dependency vendoring and vendor metadata
   src/package/publish.rs     publish dry-runs, registry index, and archive targets
   ```

3. Split lowering by backend responsibility:

   ```text
   src/lower/mod.rs
   src/lower/rust.rs
   src/lower/source_map.rs
   src/lower/rustc_remap.rs
   src/lower/runtime_diagnostics.rs
   src/lower/intrinsics.rs
   ```

4. Integration tests use four primary semantic domains plus two
   process-isolated JIT configuration targets:

   ```text
   tests/static.rs
   tests/runtime.rs
   tests/differential.rs
   tests/soak.rs
   tests/jit_cost_model.rs
   tests/jit_env.rs
   ```

   Internal module files are implementation detail under those four targets:

   ```text
   tests/checker_frontend.rs
   tests/checker_lowering.rs
   tests/checker_review.rs
   tests/checker_package.rs
   tests/s3_iam_reir_demo_e2e.rs      ignored runtime demo plus fast REIR preflight/scenarios
   tests/file_upload_benchmark_e2e.rs ignored release/demo benchmark
   ```

5. Only then reduce checker internals further. Checker changes are higher risk
   because they carry the language invariants.

## Non-Goals

- Do not introduce compatibility aliases while RSScript is pre-adoption.
- Do not add package-manager behavior that depends on unimplemented language
  semantics.
- Do not make Rust lowering responsible for accepting code the frontend cannot
  explain.
- Do not classify unknown review regions as low risk.
