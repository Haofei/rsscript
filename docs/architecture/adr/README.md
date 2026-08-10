# Architecture decision records

An ADR is required whenever a change modifies an RSScript compatibility or
security contract. This includes language semantics, MIR, bytecode/Artifact
formats, Provider ABI, and the reviewed default SDK façade.

Use [`template.md`](template.md) and name records `NNNN-short-title.md` in
monotonic numeric order. An ADR records the decision and migration boundary; it
does not replace the language specification or an implementation test.

The Core CI gate compares the change against its base revision. If it touches a
contract-owning crate, the same change must add or modify an ADR in this
directory. Documentation-only clarifications do not need an ADR unless they
change the contract itself.
