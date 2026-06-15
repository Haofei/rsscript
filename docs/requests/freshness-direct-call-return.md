# Feature request: freshness propagation through a direct `return fresh_call()`

**Status:** requested · **Driver:** tinygrad-rsmc clean `rss check`
**Repro built on:** current `rss` (post literal/enum/clean-local freshness fixes)

## Summary

Treat `return <call-to-fresh-fn>(...)` as fresh when the callee is a known
fresh-returning function — the same way the just-landed "clean local" rule treats
`let s = <fresh-call>(); return s`. This is the lone remaining freshness gap.

## Motivation / impact

After the literal-return, enum-variant, and clean-local-propagation fixes,
`rss check tinygrad-rss` went from **483 → 1** `RS0602` warning. The single
remaining one is a function that simply forwards another fresh function's result
directly in a `return`, without binding it to a local first. Closing this gets the
port to a **0-warning** check.

## Current behavior (verified)

The remaining warning in the port:

```
warning[RS0602]: freshness of return value in `tensor_max_pool2d_resolved`
  --> tinygrad-rss/src/tensor.rss:4867
     return tensor_max_pool2d(cache: mut cache, x: x, kh: kh, ... )
```

Minimal form:

```rss
fn make() -> fresh String { return "x" }

fn via_local() -> fresh String {
    let s = make()
    return s            // OK now (clean-local rule)
}

fn direct() -> fresh String {
    return make()       // warning[RS0602]: freshness unknown
}
```

The via-local spelling is already accepted; the equivalent direct spelling is not.

## Proposed behavior

In the freshness check, a `return E` where `E` is a call to a function whose
return is known-fresh (declared/inferred `fresh`) is itself fresh — identical to
binding `E` to a clean local and returning that local.

## Acceptance criteria

- `fn direct() -> fresh String { return make() }` checks clean.
- `rss check tinygrad-rss` reports **0 warnings** (currently 1).

## Notes

- Low priority / small: it's a single warning, no behavioral effect, and a natural
  extension of the clean-local propagation rule already implemented.
