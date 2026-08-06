# Status

The platform-neutral language cut is active:

- legacy file headers, generic declaration effects, source implementation-origin
  markers, and source unsafe boundaries are removed;
- retention is a structured declaration field;
- dynamic protocol dispatch uses `Dyn<P>`;
- default core interfaces contain no host service modules;
- external package functions lower through `CallExternal` and are resolved by an
  execution-time registry;
- provider runtime values and callables are owned by the safe provider API; the
  native ABI adapts that model instead of defining it;
- execution/deployment policy types and legacy API facade are removed;
- neutral package analysis uses `rsscript.package_analysis.v1`; optional review
  output uses the distinct `rsscript.package_review.v1` schema.

The physical dependency cut is not complete: the main crate still contains the
analyzer orchestration, package tooling, VM, AOT lowering, and native loading.
Native plugin loading and guarded child-process tooling are explicit
`native-plugin` / `host-tools` features and are absent from the compiler's
default dependency closure. The compiler now consumes runtime `core` only; VM
compatibility intrinsics for filesystem and process access fail with an explicit
provider-required error instead of reaching the OS. Disabled hosts fail before
build, spawn, or dynamic loading when those integrations are requested.
REIR conversion now lives in the one-way `integrations/rsscript-review-reir`
adapter and is absent from normal compiler dependencies, public compiler APIs,
CLI package output, and package metadata writes. The retired policy-oriented
examples and action have been removed. The LSP now consumes the document-oriented
`rsscript-language-service` API rather than importing the product façade directly;
the remaining transitive compiler dependency will disappear as diagnostics and
package snapshots finish moving into their owning crates. The lexer, parser,
source AST, syntax desugarings, spans, and
bounded parse budget are now owned by the independent `rsscript-syntax` and
`rsscript-work-budget` crates. Structural types, type interning, substitution,
parameter effects, package-wide semantic type facts, Typed HIR, call binding, and
HIR construction are now owned by `rsscript-semantics`. The platform-neutral core
and standard-package interface sources are owned by the data-only
`rsscript-interface-catalog`. These are re-exported through the compatibility
façade while the remaining checks are migrated. `rsscript-lowering` now owns the
provider-neutral `ExecutableIr` gate; the VM and Rust AOT path both receive this
checked representation, and JIT remains downstream of the VM unit. Runtime-core
now compiles without filesystem, environment, process, network, entropy, or
temporary-directory modules and its default feature set is network-free. The
existing VM/AOT compatibility surface still opts into those modules through the
explicit `legacy-host` composition feature while external calls move to provider
packages. The current roadmap prioritizes those boundaries over new language,
JIT, self-hosting, or package-system scope.
