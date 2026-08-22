## Change

Describe the user-visible outcome and the boundary changed.

## Verification

- [ ] Relevant focused tests pass.
- [ ] `cargo run --locked -p rsscript-xtask -- validate-ci` passes.
- [ ] Core formatting, lint, and test gates pass when Core code changed.
- [ ] Failure, cancellation, cleanup, and budget paths are covered when applicable.
- [ ] Serialized/public contract changes include an ADR and compatibility fixtures.

## Trust boundary

State whether this changes Artifact verification, Provider authority, VM/JIT
execution, runner isolation, secret-bearing output, or release provenance. Write
`None` when no trust boundary changes.
