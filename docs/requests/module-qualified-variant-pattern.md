# Feature request: qualified sum variant in `match` pattern position

**Status:** IMPLEMENTED · **Driver:** tinygrad-rsmc module de-prefixing
**Repro built on:** `rss` @ `6c6c57a`

> **Resolution.** A module-qualified variant is accepted as a match pattern. The
> pattern parser now reads a dotted head (`ops.ADD`, `a.b.Variant`) — no more
> RS0015 — and `module_isolation` rewrites it to the bare variant (resolved
> through its sum type), identical to qualified value access in expression
> position. Exhaustiveness/typeck treat `ops.ADD` as the bare `ADD` it resolves
> to. Verified through the semantic checker on a merged program:
> `qualified_variant_in_match_pattern_checks_clean`
> (`tests/checker_frontend/misc.rs`). Qualified `module.Type { .. }` payload
> patterns resolve too.

## Summary

Allow a module-qualified sum variant (`module.Variant`) as a pattern in a `match`
arm, mirroring qualified value access in expression position. Today only a bare
variant works in pattern position.

## Motivation / impact

tinygrad-rsmc dispatches on `Ops` variants in `match` arms pervasively
(`node_*`/`uop_*` lowering and rewrite rules are large `match` over the op kind).
Once `Ops` lives in `module ops`, those arms need to name its variants. Without a
qualified pattern form, every consumer file must `use ops.ADD`, `use ops.MUL`, …
for *each* variant it matches on — the same per-symbol-`use` explosion that blocks
de-prefixing. A qualified pattern (`ops.ADD => …`) lets an arm name a foreign
variant without an import line, and is the only way to disambiguate same-named
variants from two modules in one `match`.

## Current behavior (verified via `rss check`)

```rss
module app
use ops.Ops

fn classify(o: read Ops) -> Int {
    match o {
        ops.ADD => { return 1 }
        ops.MUL => { return 2 }
    }
}
```

```
error[RS0015]: unsupported RSScript syntax.   // ops.ADD pattern
error[RS0015]: unsupported RSScript syntax.   // ops.MUL pattern
```

The parser does not accept a dotted path as a pattern. The bare form works with
per-symbol imports:

```rss
use ops.Ops
use ops.ADD
use ops.MUL

fn classify(o: read Ops) -> Int {
    match o {
        ADD => { return 1 }   // OK — checks clean
        MUL => { return 2 }
    }
}
```

## Proposed behavior

Accept `module.Variant` as a match pattern and resolve it the same way the
expression-position rewrite does — through the variant's (module-mangled) sum
type — so no per-variant `use` is required.

## Acceptance criteria

- The first snippet (qualified `ops.ADD` / `ops.MUL` arms, only `use ops.Ops`)
  checks clean.
- Exhaustiveness/typeck treats `ops.ADD` identically to the bare `ADD` it
  resolves to.
- Verified at the **package level via `rss check`** (see the testing note in
  `module-value-access.md`).

## Notes

- Companion to `module-value-access.md` (value position, RS0026) — this is the
  pattern-position counterpart (RS0015, a parser gap). Glob import
  (`module-glob-import.md`) would let bare variants work in patterns without
  per-symbol `use`, addressing the common single-`Ops`-enum case; qualified
  patterns are still needed for cross-module same-name disambiguation.
