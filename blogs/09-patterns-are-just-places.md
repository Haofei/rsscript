# Patterns are just places

Another design note from the workbench, in the same register as [the last one](08-the-feature-i-didnt-build.md). This one is about pattern matching, and it's a small story about expecting a hard problem and finding that the language had already solved it — which, when it happens, is the strongest signal you get that a design is coherent rather than just a pile of features that happen to compile together.

It starts the way these design notes always start: with a real program asking for something the language didn't have.

## The interpreter wanted more

The next real program I'm building in RSScript is an interpreter — a tree-walker over RSScript's own syntax, which is the most match-heavy kind of program there is. An interpreter is, structurally, one enormous pattern match over an abstract syntax tree, repeated:

```rust
match expr {
    Call { callee, args } => ...
    Binary { left, op, right } => ...
    Match { scrutinee, arms } => ...
    Block { statements } => ...
}
```

That code wants real pattern matching: destructure a sum variant into its named fields, reach into nested structure, ignore the parts you don't care about, maybe guard an arm with a condition. RSScript, at the point I started, had almost none of it. A `match` arm could name a single binding for a variant's payload — `Ok(value)`, `Some(x)`, `Err(e)` — match literals, and match `_`. That's it. No struct destructuring, no nesting, no multiple fields, no guards. Writing the interpreter against that meant matching a variant to get one binding, then manually pulling fields out of it line by line, which is exactly the kind of ceremony pattern matching exists to delete.

So I needed to make pattern matching stronger: struct patterns, nested patterns, destructuring a variant's payload, an optional guard, `_` to ignore. None of that list is exotic — it's table stakes in any language with sum types. The reason it was worth a blog post is not the feature list. It's what happened when I sat down to figure out the *semantics*, and braced for the part I was sure would be hard.

## The hard problem I was expecting

Here is why I expected pain. In most languages, a pattern is a pattern — you destructure a value and you get its parts, and the only question is whether you bound them by reference or by move, which the compiler mostly figures out for you. RSScript doesn't have that luxury, because RSScript has an ownership and effect model that no other language's pattern matching maps onto cleanly.

When you write `match expr { Call { callee, args } => ... }`, what *are* `callee` and `args`? Are they shared views borrowed from `expr`, or are they moved out of it? If `expr` is a managed value — reference-counted, possibly shared with other parts of the program — can you move a field out of it at all? If `expr` is a `local`, exclusive value, can you move some fields out and leave others? What if you want to read one field and mutate another in the same arm? What stops a guard from mutating the thing being matched halfway through deciding whether the arm applies?

Every one of those is a real question with a wrong answer that compiles and does something subtly unsound. I'd watched Rust spend years getting "match ergonomics" — the rules for when a pattern binds by reference versus by value — to a place that's correct but genuinely intricate, and Rust doesn't even have RSScript's `read`/`mut`/`take` distinction or its managed/local split layered on top. I sat down expecting to design a whole sub-system of binding-mode rules, and to get a third of them wrong.

## The thing I missed for an afternoon

Then I wrote down what a destructuring pattern actually *is*, operationally, and the problem evaporated.

`Call { callee, args }` is not a new kind of value-handling. It's a name for two **places** — `expr.callee` and `expr.args` — the same places you'd get by writing `expr.callee` and `expr.args` by hand. A pattern is sugar for a set of named place projections of the thing you're matching. And RSScript already has a complete, enforced theory of places: which places conflict with which, what you're allowed to do to a place under each effect, what happens when you move out of one field and read another. It's the machinery that makes field-level mutation and move checking work in ordinary code. The whole sub-system I was bracing to design *already exists*, and a pattern is just a new way to spell the thing it already governs.

Once I saw patterns as places, every boundary case I'd been dreading answered itself with a rule the language already had:

```text
the scrutinee's effect is the default binding mode, and it can only narrow.
```

You can't pull a stronger capability out of a pattern than you brought into it. Match a `read` value and the bindings are `read` views; you can't get a `mut` binding out of something you only have read access to — that's the same read-view-mutation error you'd get writing it by hand. Match a `mut` value and you can bind fields `read` or `mut`. Match an owned `local` value and you can `take` fields out — but only because you could `take` them by hand, and the move requires `features: local`, the exact rule that [corrected me in the last post](08-the-feature-i-didnt-build.md).

## The boundaries, and the rule that decides each one

I went through the cases I'd been afraid of, and not one of them needed a new rule.

**The interpreter's case — read everything.** `match read expr { Call { callee, args } => ... }`. The scrutinee is a `read` view of a managed AST node, so `callee` and `args` are `read` views into it, valid for the arm. You can pass them along as `read`; you can't mutate them; you can't move them, because you don't own the node. This is the common case, it's the clean one, and it's exactly what the interpreter wanted. Nothing new.

**Splitting one value into different effects.** `match mut node { Call { callee: read c, args: mut a } => ... }` — read one field, mutate another, from the same value. In most languages this is the fiddly case. In RSScript it's decided by one question the language already answers: are `callee` and `args` disjoint places? They are — different fields — so binding one `read` and the other `mut` is fine, the same way touching two different fields of a struct in one expression is fine. If the two bindings *overlapped* and one were `mut`, you'd get the existing field-partial-access conflict. The pattern didn't introduce a new rule; it inherited the disjointness rule.

**Moving fields out.** `match take node { Binary { left, right } => ... }` — under `features: local`, `left` and `right` are moved out of `node`, which is consumed. Two disjoint moves, both fine; the language already knows you can move out of disjoint places. With one exception it also already knows: you can't move a `handle` or `weak` field out, because moving a retained handle is forbidden everywhere, not just in patterns. And if the value were managed rather than `local` — reference-counted, shared — you couldn't move a field out at all, because you can't tear a piece off an object someone else might be holding. Managed scrutinees get `read` bindings, full stop. Every one of those is a rule that predates pattern matching by months.

**Guards.** The one genuinely new rule, and it's small: a guard may only *read* the bindings. `Call { callee, args } if is_empty(args) =>` is fine; a guard that mutated the value mid-match would be hidden control flow — the thing deciding whether the arm applies is also changing the thing being matched — and "no hidden control flow" is a constitutional line. So guards are read-only over their bindings. That's the entire new semantics the feature needed: one sentence.

## Why this is the interesting result

The temptation, writing this up, is to make it sound like cleverness. It wasn't. I didn't design an elegant pattern system; I discovered that I didn't have to, because the elegant part was already done and pattern matching was just a new surface over it. The richer patterns themselves — struct, nested, the payload destructuring, the guard, the `_` — aren't built yet; this is the design, not a shipped thing, and the work that remains is real, mostly in extending the exhaustiveness checker to reason about nested and struct patterns instead of flat variants. But the *ownership* design, the part I was sure would be a multi-week subsystem, turned out to be zero new rules and one sentence about guards.

That outcome is the actual point, and it's a property of the language, not of me. [Post 4](04-why-no-one-built-this-before.md) made a claim in passing that I didn't have evidence for at the time: that a good language is made of *coordinated projections of one underlying model*, so that features compose instead of each dragging in its own special cases. This is what that claim looks like when it's true. Pattern matching and field-level mutation and move checking are not three features with three rule-sets that have to be reconciled at their seams. They are three surfaces over one theory of places, and when you add the third surface, the theory already covers it. The seam isn't there to get wrong.

The inverse — a feature you bolt on that needs its own ownership rules, that interacts with the existing ones in ways no one fully designed — is how languages accumulate the dark corners where the soundness bugs live. The fact that pattern matching *didn't* do that is a small, concrete piece of evidence that the model underneath is actually one model, and not a collection of features wearing a trench coat. You don't get to know that from the spec. You find out the first time you extend the language and the extension has nowhere new to break.

I'll take "the hard part was already solved" as a result. It doesn't happen often, and when it does, it's the language telling you the foundation is sound.

---

*Next: [When you are the entire ecosystem](10-when-you-are-the-entire-ecosystem.md) — with zero users, nobody catches your mistakes, so the toolchain has to catch them itself. Including the one where I taught every model a syntax that doesn't exist.*
