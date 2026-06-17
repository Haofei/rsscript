# Declarative graph-rewrite for the tinygrad port

## Context / problem

The ML port (tinygrad-in-RSScript) reimplements tinygrad's **declarative**
scheduler/codegen as **bespoke imperative recursion**. tinygrad expresses every
rewrite as data — `PatternMatcher([(UPat(...), lambda node: ...)])` applied by one
generic `graph_rewrite(sink, pm, ...)` driver with built-in fixpoint +
memoization. The port hand-rolls each pass (`indexing_rangeify_rewrite_node`,
`graph_rewrite_memo_d`, …) with manual `olds/news` memo arrays.

That paraphrase-of-a-declarative-engine is the root cause of the port's worst
bugs. Canonical example (chained-reduce): tinygrad's rule is
`(UPat(BUFFERIZE), bufferize_to_store)` — it matches a *bare* BUFFERIZE node and
writes using **that node's own ranges**. The port re-expressed it as "match
`INDEX(STAGE)` adjacency, use the **consumer's** ranges," which (a) conflates
write-ranges with read-ranges and (b) silently breaks when another bespoke pass
(`flatten_bufferize`) perturbs the graph shape. A real PatternMatcher makes the
scheduler a near-mechanical **transliteration** of `rangeify.py` — rules match the
same node shapes tinygrad matches, so this whole class of divergence can't arise.
Paraphrase drifts; transliteration doesn't.

## What RSScript already has (premises that are NOT gaps)

Grounded against the implementation — the original ask over-stated the gap:
- **Closures are first-class runtime values**: `VmValue::Closure(Rc<VmClosure>)`,
  `MakeClosure`/`CallClosure` in the reg-VM; closures capture (incl. owned)
  values; `List.fold`'s `|acc, x|` is one in use.
- **An `Fn(...)` type exists** in the checker (e.g. `noescape Fn()`), with
  function-value desugaring (`syntax/function_value_desugar.rs`).
- **Tuple destructuring is done**: `let (a, b, c) = expr`
  (`syntax/desugar.rs::expand_tuple_destructuring`). No work needed.

## The actual keystone gap: escaping / storable closures

A `PatternMatcher` is a **stored list of closures that outlives its definition
site** and is called repeatedly by the rewrite driver. RSScript today restricts
closures to **`noescape`** — they may be *forwarded down* into a call but not
*stored up* (the diagnostic: "Forwarding a local closure is only allowed when the
target parameter is `noescape Fn()`"). This is a deliberate review-first
restriction (an escaping closure that captures is a retention/aliasing concern).

So the precise need is **owned, escaping closures storable in a collection**
(`List<Fn(UOp) -> UOp>` of rules). The VM already represents them (a `Closure` is
an `Rc`, trivially storable); the gap is the **checker's escape rule** plus a
**storable `Fn` value type** in signatures. Relaxing this is the one real language
decision — and it must clear RSScript's admission bar: an escaping+capturing
closure has to *phrase as a reviewer question* (what it captured, whether it can
retain), e.g. via an explicit `own`/move capture annotation and the `Fn` type
surfacing in the signature, so nothing is implicit.

## Layered plan (priority order)

1. **L1 — language (keystone): owned/escaping storable closures.** Allow a
   closure that captures owned values to escape into a value of a first-class
   `Fn(args) -> ret` type and be stored in containers. Make the capture + escape
   explicit in the signature (review-first). Everything else rides on this.
2. **L2 — library/runtime: PatternMatcher + graph_rewrite.** With L1, build a
   `UOp`/`UPat` type and a native `graph_rewrite(sink, rules, bottom_up, name)`
   driver (fixpoint + memoization) plus a gated `toposort(gate: Fn(UOp)->Bool)` —
   as a runtime/library facility (the Tensor-kernel pattern), NOT more language
   surface. The port then transliterates `rangeify.py` rule-for-rule. Also
   retroactively simplifies divmod/symbolic (already faking a fixpoint via
   `graph_rewrite_memo_d`) and codegen.
3. **L3 — ergonomic sugar (independent): iteration.** No `for`-in / comprehension
   exists (the `for` keyword is `protocol … for Type` impls). `while i <
   List.len { List.get(...) }` is ~3× the Python and a transcription-error source.
   `for x in xs` / `map`/`filter` is a separate, independently-weighable nicety.
- **Done already:** tuple destructuring (`let (a,b,c) = …`).

## The decision

The win is declarative *fidelity* — transliteration over paraphrase eliminates a
whole bug class, and it's the highest-leverage thing the port could get. The only
deep change is **L1 (escaping storable closures)**; L2 is then a library, L3 is
optional sugar. L1 is exactly the kind of expressivity RSScript has deliberately
withheld, so the gate is: can an escaping+capturing closure be made *explicit
enough in the signature* to satisfy the review-first contract? If yes, this is the
next language-roadmap item. If the answer is "only with implicit retention rules,"
it stays out and the port keeps a (smaller, well-tested) hand-rolled engine.
