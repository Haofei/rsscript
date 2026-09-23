# Architecture decision records

An ADR is required whenever a change modifies an RSScript compatibility or
security contract: language semantics, MIR, bytecode/Artifact formats, the
Provider ABI, and the reviewed default SDK façade. `scripts/check-contract-adr.sh`
enforces it in CI by requiring a `docs/architecture/adr/NNNN-short-title.md`
addition or update whenever a contract-owning crate changes. A change confined
to a crate's `tests/` directory exercises a contract without changing it and
does not trip the gate; the one exception is the Artifact/Provider
compatibility corpus (`crates/rsscript-sdk/tests/compatibility_corpus.rs` and
`tests/corpus/compatibility/`), whose expectations are the contract's
checked-in record.

Use [`template.md`](template.md) and name records `NNNN-short-title.md` in
monotonic numeric order. An ADR records the decision and migration boundary; it
does not replace the language specification or an implementation test.
Documentation-only clarifications do not need an ADR unless they change the
contract itself.

## Live records

Records from 0226 onward are the current product contracts: the post-migration
contract set, the single executable contract, verified typed executable facts,
the generation oracle, the JIT as a product-owned tier, function values as a
wire type, and `u64` nanoseconds in the native engine telemetry summary. Read
these before changing a contract.

## Archive

[`archive/`](archive/) holds records 0001 through 0225. They were written one
per pass during the August 2026 compiler migration, mostly to record which crate
owns a diagnostic or a lowering step. They remain binding where the code still
matches them, but several describe surfaces that were later archived (Rust AOT,
self-hosting, the legacy executable IR) and none is linked from a current
document. New records never go in the archive; the CI gate matches only the
top-level directory.
