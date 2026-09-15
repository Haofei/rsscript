# VM runtime dependency inventory

`rsscript-vm` is the execution engine. Its normal dependency section is kept
small and reviewed so deterministic library implementations do not silently
become part of the VM trusted computing base.

## Approved runtime dependencies

| Dependency | Owner | Reason | Removal condition |
| --- | --- | --- | --- |
| `rsscript-abi-model`, `rsscript-bytecode`, `rsscript-diagnostics`, `rsscript-core-types`, `rsscript-provider-api` | Core execution boundary | Versioned ABI, verified program, diagnostics, operation controls, Provider dispatch, and the total text utilities shared with the frontend (`rsscript_core_types::text`). | Stable Core contracts; not scheduled for removal. |
| `base64`, `chrono`, `flate2`, `hex`, `hmac`, `percent-encoding`, `regex`, `serde_json`, `serde_yaml_ng`, `sha2`, `sha3` | Deterministic builtin boundary, owned by the `corelib` module | Encoding, regex, date, hash/HMAC, gzip, JSON/YAML structured-data representation, and generic collection algorithms. These were the dependencies of the separate `rsscript-corelib` crate, which is now `crates/rsscript-vm/src/corelib.rs`. The legacy JSON adapter is re-exported only through `structured_data`. | Remains the one-way pure-library boundary; P06.2/P06.4 remove the legacy dynamic representation itself. |
| `serde` | VM model serialization | Serialization derives for the VM's verified-program and value-model support types. | Re-evaluate only if the VM model’s serialization contract changes. |
| `vm-jit` (optional) | Adaptive native backend | Cranelift translation, executable-memory management, OSR, and deoptimization for explicitly trusted in-process execution. It is absent from the default VM closure and cannot be selected by an Artifact. | Required VM tier under active development. Individual optimizations stay only while controlled workloads show a material end-to-end win; the engine itself is not a removal candidate. Native/interpreter differential gates must pass. |

## The `corelib` module boundary

`rsscript-corelib` was merged into the VM as `crates/rsscript-vm/src/corelib.rs`.
It keeps the contract it had as a crate, but the contract is now enforced on
source rather than by Cargo, so it is worth stating precisely. Two rules, both
checked by `deterministic_core_library_is_pure_and_the_vm_only_adapts_its_results`:

1. The `corelib` module is a one-way pure library. It must not name `crate::`,
   `super::`, `VmValue`, `EvalError`, `reg_vm`, or any Core contract crate. The
   VM adapts its value representation at the boundary and keeps accounting.
2. VM source files **must not directly name algorithm crates** for encoding,
   regex, time/date, compression, hashing, HMAC, or YAML. `src/corelib.rs` is the
   only file allowed to, and the check runs per file, so a new algorithm
   dependency cannot appear anywhere else in the VM. Deterministic code
   independent of VM values belongs in the module; host-visible services remain
   Provider calls.

Note the tradeoff this merge made: while `rsscript-corelib` was a separate
package, "the pure library cannot see a VM type" was a fact about the dependency
graph that no edit could violate. It is now a lint over source text. The
per-file check is tighter than the old per-package manifest check for rule 2,
but rule 1 is weaker than it was. Reversing the merge restores it.

The native JIT is an opt-in VM feature and remains outside the default Core
dependency closure. Selecting it is a host build and execution decision; source
code and Artifacts cannot request it. Bounded or isolated execution continues to
use the reference interpreter until native execution implements identical
deterministic budget accounting, which is an active roadmap goal.

This inventory describes dependencies only. It does not make the in-process VM
a security isolation boundary; see the threat model for trust and runner
requirements.
