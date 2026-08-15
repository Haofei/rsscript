# ADR 0225: Own captured package-review inputs outside the compiler

- Status: Accepted
- Date: 2026-08-15

## Problem

The optional package-review compatibility path kept its typed manifest and
bounded source-set loader in `rsscript-compiler/src/package`. Although normal
compiler entry points were already in-memory and provider-neutral, this made
the compiler the physical owner of project/review input representation.

## Decision

`rsscript-package-review` owns the captured package manifest model,
feature-selected source-set model, bounded loader, and source-level contract
extractor. The latter calls `CompilationSession` and syntax directly rather
than routing semantic facts through compiler-local helpers. It also owns the
neutral resource/task execution-fact collector used by package analysis. The crate depends
on `rsscript-project` for confined/no-follow capture and on
`rsscript-package-model` for versioned file-kind identity; it has no compiler
dependency.

The await collector receives a read-only runtime-intrinsic identity table
generated from the shared catalog. That removes the last production use of the
compiler-local `runtime_abi` module without coupling review evidence to the VM.
The dependency resolver likewise consumes project-captured manifest graphs and
uses bounded project reads for diagnostic spans; it no longer lives in the
compiler package module.

The same boundary now owns manifest review-policy evaluation and diagnostics.
Policy diagnostics use project-owned bounded manifest reads, and signature
limits use review-owned contract extraction. Compiler-native helper functions
no longer own or expose that review policy logic.

Neutral package analysis also belongs to this boundary. It builds
`rsscript.package_analysis.v1` directly from captured sources and
`CompilationSession` facts, and reads the catalog digest from the catalog
crate rather than a compiler implementation helper. The compiler retains only
the authorization wrapper needed by its legacy compatibility entry point.

Native binding descriptor parsing, binding-interface projection, and binding
diagnostics are likewise review-owned. This removes native binding manifest
formatting and source-contract validation from compiler package code while
leaving native Rust build/review execution in its explicit compatibility
boundary.

The full package review evidence engine is now review-owned too. The compiler
does not re-export its implementation: it supplies only the legacy native Rust
inspection callback plus captured-snapshot path remapping. That prevents native
wrapper support from pulling parser, semantic evidence, policy, or package
review presentation back into compiler-owned source modules.

The compatibility package diff now consumes the review-owned engine directly.
It receives the same explicit native inspection callback, so manifest/interface
comparison and review evidence no longer live in a compiler package module.

Package lock construction, comparison, bounded parsing, and content hashing
are review-owned as well. The one native-sensitive operation is an explicit
rooted-path resolver callback; compiler compatibility retains only that adapter
and snapshot remapping rather than lock semantics or hashing implementation.

Package graph construction and graph-level review validation are also
review-owned. They consume the project-captured manifest graph and receive the
legacy native Rust inspection only as the same explicit callback. Compiler
compatibility now only authorizes the captured input and remaps snapshot paths
for its legacy public result.

The compiler's opt-in `package` compatibility feature has a private forwarding
module during the staged migration so existing authorization, native, lock, and
review callers retain their established behavior. The reviewed compiler default
closure remains unchanged and does not select this crate.

## Consequences

This is a staged physical S05.3 migration. The remaining package check and
final public compatibility composition must move before the forwarding module
can be removed. Architecture tests assert that captured review implementation
files cannot return under `rsscript-compiler`, while compiler compatibility
continues to consume project-owned bounded capture APIs.
