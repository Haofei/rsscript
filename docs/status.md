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
- package analysis uses the breaking `rsscript.package_analysis.v1` schema.

Runtime/JIT optimization work continues independently of these language and host
boundaries.
