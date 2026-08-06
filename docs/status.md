# Status

The platform-neutral language cut is active:

- legacy file headers, generic declaration effects, source implementation-origin
  markers, and source unsafe boundaries are removed;
- retention is a structured declaration field;
- dynamic protocol dispatch uses `Dyn<P>`;
- default core interfaces contain no host service modules;
- external package functions lower through `CallExternal` and are resolved by an
  execution-time registry;
- execution/deployment policy types and legacy API facade are removed;
- neutral package analysis uses `rsscript.package_analysis.v1`; optional review
  output uses the distinct `rsscript.package_review.v1` schema.

The physical dependency cut is not complete: the main crate still contains the
analyzer orchestration, package tooling, VM, AOT lowering, native loading, and
review adapters. The lexer, parser, source AST, syntax desugarings, spans, and
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
