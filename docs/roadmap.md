# RSScript roadmap

This roadmap implements the product boundary in [product.md](product.md). The
language specification and tests remain authoritative for existing behavior.

## Priority 0: make current claims true

1. Remove active policy/capability examples and obsolete product descriptions.
2. Replace catalog-size assertions with uniqueness, completeness, signature,
   determinism, and orphan-entry checks.
3. Split neutral package analysis from review, provider, native, and build
   metadata, with checked-in schemas.
4. Enforce dependency direction using Cargo metadata.

## Priority 1: establish Core contracts

1. Move the real syntax and semantic models into dependency-boundary crates.
2. Make one validated typed HIR the source for VM, AOT, LSP, package analysis,
   and optional review.
3. Define a provider ABI with versioned semantic signature hashes, load-time
   validation, cancellation behavior, and resource cleanup contracts.
4. Define one provider-independent executable IR.
5. Split runtime-core from concrete filesystem, environment, process, network,
   time, entropy, logging, CLI, and OS-handle providers.

## Priority 2: verified execution and embedding

1. Add deterministic experimental bytecode and a bounded decoder.
2. Add structural verification and require `VerifiedBytecode` at VM entry.
3. Stabilize the embedding façade around compiler, compiled package, runtime,
   provider registry, run limits, diagnostics, and execution reports.
4. Add build and inspect commands for imports, call graphs, resources, async
   structure, analysis, and bytecode.
5. Ship one provider-replacement demo and end-to-end conformance workload.

## Frozen scope

Until the priorities above are complete, do not expand language syntax, public
intrinsics, JIT tiers or speculation, the C backend, full self-host bootstrap,
package publishing, native plugin surface, or language-level policy. Correctness,
security boundary, maintenance, and measured-regression fixes remain allowed.

Rust AOT and Cranelift JIT stay Experimental. REIR stays an Integration.
Self-hosting stays Research. Promotion follows the criteria in
[feature-matrix.md](feature-matrix.md).
