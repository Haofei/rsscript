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

   `src/rust_lower.rs` owns generated Rust, source maps, backend verification,
   rustc remapping, runtime diagnostic parsing, and runtime hook selection.
   Lowering consumes checked RSScript semantics; it should not be the first
   place that discovers language errors.

5. Package Tooling

   `src/package.rs` owns package manifests, `.rssi` public contracts, dependency
   graphs, semantic locks, package review, publish dry-runs, vendoring, and
   package metadata. It consumes syntax/check/review/lowering APIs.

6. Runtime

   `runtime/` owns the reference runtime ABI: managed handles, weak handles,
   resources, diagnostics, and core native hooks used by lowered Rust.

7. CLI

   `src/main.rs` is only the process entrypoint. `src/cli.rs` owns command-line
   parsing and command dispatch, and should remain an application shell around
   the library APIs.

## Current Hotspots

These files are intentionally tracked as refactoring targets:

```text
tests/checker.rs       integration harness and many fixture assertions
src/package.rs         package domain model, graph, review, lock, publish, vendor
src/rust_lower.rs      lowering, backend checks, source maps, remapping, intrinsics
src/analyzer.rs        frontend orchestration
src/checks/*.rs        large semantic checker implementations
```

## Refactoring Order

1. Keep `src/main.rs` thin and move CLI application code under `src/cli.rs`.
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
   src/package/format.rs      JSON/TOML/human output formatting
   src/package/source_set.rs  rsspkg.toml loading and source/interface selection
   src/package/native.rs      native binding metadata and native Rust risk checks
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

4. Split integration tests by semantic area:

   ```text
   tests/frontend.rs
   tests/lowering.rs
   tests/review.rs
   tests/package.rs
   tests/selfhost.rs
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
