# RSScript roadmap

This roadmap implements the product boundary in [product.md](product.md). The
language specification and tests remain authoritative for existing behavior.

## Completed product-boundary milestones

1. Removed active policy/capability examples and obsolete product descriptions.
2. Replaced catalog-size assertions with uniqueness, completeness, signature,
   determinism, and orphan-entry checks.
3. Split neutral package analysis from review, provider, native, and build
   metadata, with checked-in schemas.
4. Enforced dependency direction using Cargo metadata.
5. Established syntax, structural semantics, Typed HIR, executable IR, provider
   ABI, host-neutral runtime defaults, and concrete leaf providers.
6. Added bounded `rsscript.bytecode.v1`, structural verification, verified-only
   VM construction, a stable embedding façade, build/inspect commands, and the
   provider-replacement demonstration.

## Current priority: conformance and boundary hardening

1. Expand bytecode decoder/verifier fuzzing and schema compatibility fixtures.
2. Deepen resource/cancellation state-machine and provider conformance tests.
3. Continue reducing compatibility-crate API and crate-wide lint exceptions.
4. Measure check latency, cold load/verify, memory, cancellation latency,
   provider overhead, VM throughput, and artifact size on product workloads.
5. Promote a feature only through the maturity matrix; do not add syntax while a
   Core row remains partial.

## Frozen scope

Until the priorities above are complete, do not expand language syntax, public
intrinsics, JIT tiers or speculation, the C backend, full self-host bootstrap,
package publishing, native plugin surface, or language-level policy. Correctness,
security boundary, maintenance, and measured-regression fixes remain allowed.

Rust AOT and Cranelift JIT stay Experimental. REIR stays an Integration.
Self-hosting stays Research. Promotion follows the criteria in
[feature-matrix.md](feature-matrix.md).
