# Feature request: `read module.fn(...)` should parse as read-of-call

**Status:** requested · **Driver:** tinygrad-rsmc module de-prefixing
**Repro built on:** `rss` @ `78ad2d9`

## Summary

In argument position, `read module.fn(args)` misparses: the `read` binds to the
module path's first segment (`read module`) and the rest is taken as a method
call on it, so the argument is seen as a non-`read` value. A bare `read fn(args)`
(unqualified call) parses correctly. The qualified form should parse the same way
— `read` applied to the whole qualified-call expression.

## Motivation / impact

Module de-prefixing turns flat calls like `linearizer_order_names(...)` into
qualified `linearizer.order_names(...)`. Anywhere such a call sat under `read`
(common for `read`-borrowed string/list arguments), the conversion breaks:

```rss
// before (flat): parses fine
showstr(label: read "...", v: read linearizer_order_names(cache: read c, order: read o))

// after (qualified): error
showstr(label: read "...", v: read linearizer.order_names(cache: read c, order: read o))
```

## Current behavior (verified via `rss check`)

```rss
fn sink(v: read String) -> Unit { return Unit }

fn ok()   -> Unit { return sink(v: read flat_call()) }        // OK
fn bad()  -> Unit { return sink(v: read m.order_names()) }    // error[RS0202]: argument `v` ... missing `read`
fn work() -> Unit { return sink(v: read (m.order_names())) }  // OK (parenthesized)
```

So `read flat_call()` works, `read (m.call())` works, but `read m.call()` does
not — `read` greedily binds to `m` and `.order_names()` becomes a method call on
the read of `m`.

## Proposed behavior

Parse `read <postfix-expression>` so that the qualified call
`module.fn(args)` is the operand of `read`, identical to the unqualified and
parenthesized forms. (Equivalently: `read` should bind looser than the dotted
call/path, not capture only the leading identifier.)

## Acceptance criteria

- `sink(v: read m.order_names())` checks clean, same as
  `sink(v: read (m.order_names()))` and `sink(v: read flat_call())`.

## Workaround in use

Parenthesize: `read (module.fn(args))`. Applied at the two affected sites in
`main.rss` during the linearizer module conversion; would prefer the parser fix
so de-prefixing stays a pure rename.

## Notes

- Pairs with the module de-prefixing refactor (qualified calls need no `use`).
- Low risk: purely a parse-precedence issue; the parenthesized and unqualified
  forms already type-check identically.
