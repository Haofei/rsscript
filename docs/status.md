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
`rsscript-work-budget` crates and re-exported through the compatibility façade;
the runtime still contains concrete host services. The current roadmap prioritizes
those boundaries over new language, JIT, self-hosting, or package-system scope.
