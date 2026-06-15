# Feature request: glob import `use module.*`

**Status:** requested · **Driver:** tinygrad-rsmc module de-prefixing
**Repro built on:** `rss` @ `6c6c57a`

## Summary

Support `use module.*` to bring every public symbol of a module (its types, sum
variants, constants, and functions) into the current file's scope at once,
instead of one `use module.NAME` line per symbol.

## Motivation / impact

This is the deciding ergonomic feature for de-prefixing tinygrad-rsmc into real
modules. The port is currently 100% flat-prefixed — 0 of 123 files use a `module`
declaration; the cross-file call surface is `node_*` (4745 refs), `tensor_*`
(2703), `mk_*` (1731), `uop_*` (1451). The single `Ops` sum type has ~100
variants (`ADD`, `MUL`, `GLOBAL`, `INDEX`, …) referenced across ~100 files.

To move `Ops` into `module ops`, each consumer file must today list every variant
it touches as a separate `use ops.ADD`, `use ops.MUL`, … — thousands of `use`
lines across the codebase. Qualified value access (`ops.ADD`) would be the other
option but it is currently broken through `rss check` (see
`module-value-access.md`) and is noisy regardless. `use ops.*` collapses the
import surface of an enum-heavy file to a single line and makes de-prefixing
practical.

## Current behavior (verified via `rss check`)

```rss
// app.rss
module app
use ops.*

fn pick() -> fresh Ops { return ADD }
```

```
error[RS0015]: unsupported RSScript syntax.   // use ops.*
error[RS0024]: unknown type `Ops`.            // cascades — glob never imported Ops
```

The per-symbol form works (`use ops.Ops` / `use ops.ADD` + bare reference checks
clean), but there is no glob form.

## Proposed behavior

`use module.*` imports all public symbols of `module` into the current file's
scope: types and type aliases (usable bare in type position), sum variants
(usable bare in value and pattern position), constants, and functions. Equivalent
to expanding to one `use module.NAME` per public symbol.

## Acceptance criteria

- `use ops.*` brings `Ops`, its variants (`ADD`, `MUL`, …), `MAX_OPS`, and `ops`'
  functions into scope; the snippet above checks clean.
- A bare variant from a glob-imported module works in both value position
  (`return ADD`) and `match` pattern position (`ADD => { … }`).
- Verified at the **package level via `rss check`**, not only through the lowering
  helper (see the testing note in `module-value-access.md`).

## Notes

- Does not by itself solve cross-module same-named-variant collisions (two modules
  each exporting `ADD`); that still needs qualification / variant namespacing.
  For tinygrad-rsmc this is moot — a single `Ops` enum, no collisions.
- Pairs with `module-qualified-variant-pattern.md` (qualified variants in match
  patterns) and the `module-value-access.md` checker-pass fix.
