# Feature request: cross-module enum-variant / constant usage

**Status:** IMPLEMENTED (option 1, qualified value access) · **Driver:** tinygrad-rsmc module de-prefixing
**Repro built on:** current `rss` (post module-isolation + use-aliasing)

> **Resolution.** Qualified value access is implemented: `module.CONST` resolves
> to the module-mangled constant and `module.Variant` resolves through the
> variant's sum type, in value position, with no per-symbol `use`. Acceptance
> criteria 1 is met. Not done: glob `use module.*` (option 2), and full
> cross-module same-named-variant disambiguation (criterion 3) — variant names
> are still global (resolved via their sum type), so two modules declaring the
> same variant name still collide; that needs variant namespacing and is left as
> a follow-up. Qualified variants in *pattern* position are also not yet
> supported (value position only).

## Summary

Make enum/sum **variants** and **constants** defined in one `module` usable from
another without one `use` line per symbol. Today only per-symbol
`use module.NAME` + bare reference works; **qualified value access fails** and
**glob imports are unsupported**. This blocks de-prefixing enum/const-heavy files.

## Motivation / impact

The tinygrad-rsmc port has a single `Ops` sum type with ~100 variants
(`ADD`, `MUL`, `GLOBAL`, `INDEX`, …) referenced across ~100 files, plus shared
constants. To put `Ops` in `module ops`, every consumer must currently add a
separate `use ops.ADD`, `use ops.MUL`, … — thousands of `use` lines — and two
modules that both define a same-named variant can't be used in one file at all
(no qualification, no aliasing for values). That makes module de-prefixing of
these files impractical, which is the last refactor still gated.

## Current behavior (verified)

```rss
// ops.rss
module ops
sum Ops { ADD, MUL, OTHER }
const MAX_OPS: Int = 64
```

```rss
// user.rss
module user
use ops.Ops
use ops.ADD
use ops.MAX_OPS

fn a(o: read Ops) -> Int { return 1 }       // OK  (type via use)
fn b() -> fresh Ops { return ADD }          // OK  (variant via use + bare)
fn d() -> Int { return MAX_OPS }            // OK  (const via use + bare)

fn c() -> fresh Ops { return ops.MUL }      // error[RS0026]: unknown value binding `ops`
fn e() -> Int { return ops.MAX_OPS }        // error[RS0026]: unknown value binding `ops`
```

```rss
use ops.*                                    // error[RS0015]: unsupported RSScript syntax
```

So: qualified *calls* (`ops.fn()`) resolve, but qualified *values*
(`ops.MUL`, `ops.MAX_OPS`) do not, and there is no glob form.

## Proposed behavior — either option unblocks this

1. **Qualified value access:** resolve `module.Variant` and `module.CONST` in
   value position the same way `module.fn()` already resolves in call position.
   (Preferred — also gives per-use disambiguation for same-named variants across
   modules, which `use` alone cannot.)
2. **Glob import:** support `use module.*` to bring every public symbol of a
   module into scope at once. (Simpler for the common "import the whole ops
   enum" case; does not solve cross-module name collisions.)

Ideally both; #1 is the more general fix.

## Acceptance criteria

- `fn c() -> fresh Ops { return ops.MUL }` and `fn e() -> Int { return ops.MAX_OPS }`
  check clean.
- (If #2) `use ops.*` brings `Ops`, its variants, and `MAX_OPS` into scope.
- A file can reference two same-named variants from different modules — at least
  one via qualification (#1).

## Notes

- Pairs with the portman rss-adapter `module` awareness already shipped, which
  reconstructs the flat owner-qualified identity for de-prefixed functions.
- Scope is enum variants and constants specifically; types and functions already
  work via `use` / qualified calls.
