# The feature I didn't build

The language has accumulated enough mechanisms to make one constraint clear: a feature is only useful when its maintenance cost serves the product's central proof. This is a design note from the workbench about a decision to *not* build something.

The series so far has been mostly about what RSScript is. This is about how a feature gets rejected, which tells you more about a language's character than another list of what it includes. [Post 5](05-explicit-is-a-budget.md) argued that ceremony you don't need is a cost, not a virtue. This is the same discipline pointed at my own roadmap: a feature I wanted, talked myself into, designed two mechanisms for, and then didn't build — and why not building it was the right call.

## The itch

RSScript's standard library has two functions that do almost the same thing:

```rust
List.fold<T, U>(list, initial, folder: Fn(U, T) -> U) -> U
List.try_fold<T, U, E>(list, initial, folder: Fn(U, T) -> Result<U, E>) -> Result<U, E>
```

`fold` for when your step can't fail; `try_fold` for when it can. Two functions, identical in shape, differing only in whether the callback — and therefore the whole operation — can produce an error. `Map` has the same pair. And if you wanted the same convenience for `map`, `filter`, and `each`, you'd need a fallible twin of each, on each collection type. A reader of post 5 sees this and expects me to be annoyed by the duplication, and to fix it. I was, and I tried.

The instinct, when you see duplication in a language's standard library, is to reach for a mechanism that removes it. I found four.

## The four options

**Effect polymorphism.** One `map`, and the compiler figures out whether it's fallible by looking at whether the callback you passed returns a `Result`. This is roughly the effect-polymorphism approach — the compiler infers fallibility from the callback — and it's elegant: no `try_` twins at all. I sketched it for about an hour before the problem surfaced. With effect polymorphism, you read `List.map(items, f)` at a call site and *cannot tell whether it can fail*. To know, you have to go find the type of `f`. That is precisely the thing RSScript's constitution forbids — review-critical behavior, here *can this line fail*, has to be visible in the signature at the call, not inferred from somewhere else. Killed by Article III.

**Make everything return `Result`.** No fallible/infallible split because there's no infallible version; `map` always returns `Result<List<U>, E>` and the infallible case just always returns `Ok`. The API gets smaller. And every single piece of ordinary, can't-fail code gets dragged into the error monad, writing `?` and `Ok(...)` around operations that never fail. That's not a simplification; it's pollution. Killed on sight.

**A pipeline.** Move the combinators off the individual collection types and onto a single `Pipeline` value: `xs.pipeline().filter(...).map(...).collect()`. The combinators live in one place instead of being copied onto `List`, `Map`, `Set`, and everything else.

**A compiler-owned generator.** Keep the explicit `try_map` at the call site, but don't make the stdlib author hand-write it. Declare, somewhere, "generate the fallible variants of these combinators," and the compiler emits `List.try_map`, `List.try_filter`, and so on. The user still writes the explicit `try_`; the author doesn't write the boilerplate.

## The principle that did the cutting

Options three and four felt right and one and two felt wrong, and it took me a while to articulate why in a way I could defend rather than just feel. Here's the sentence I landed on, and it's the whole post in one line:

> Don't let *users* save keystrokes through inference. Let the *compiler* save boilerplate for stdlib *authors*.

Effect polymorphism (option one) saves the user keystrokes — `map` instead of `try_map` — by making the fallibility inferred, which moves a review-critical fact off the page. That's the wrong trade for a review-first language, every time. The generator (option four) saves the *author* the boilerplate of writing `try_map`, while the *user* still types `try_map` and reads it at the call site. Same duplication eliminated, opposite surface. One hides a fact from the reader to spare the writer; the other spares the writer while keeping every fact in front of the reader. Once I had that sentence, options one and two were dead and I was choosing between the pipeline and the generator.

## The realization: they aren't peers

I'd been treating the pipeline and the generator as two features I might build, maybe both. Then I noticed they attack *different* duplications.

There are two axes of repetition here. One is fallible-versus-infallible: `map` and `try_map`, a factor of two. The other is collection-times-combinator: `List.map`, `Map.map_values`, `Set.map`, the same handful of combinators copied across every container, a factor of N. The generator attacks the small axis. The pipeline attacks the big one — put the combinators on `Pipeline` once and you write them a single time instead of once per collection.

The big axis is the expensive one. And once you collapse it — combinators living on `Pipeline` alone — the total surface that's left is small enough to hand-write. A handful of combinators, times two for their fallible forms, is a couple dozen functions, written once, ever. That is not enough boilerplate to justify a whole compiler-owned generation mechanism, with all the questions it drags in about how generated methods show up in the review map and what risk they carry. The pipeline doesn't *complement* the generator. **The pipeline dissolves the problem the generator was for.** I'd been about to build two things to solve one problem, and one of them made the other unnecessary.

## The twist: the compiler corrected my design

So: an eager pipeline. I started speccing it, and I reached for the efficient design. Each stage would `take` the pipeline — consume it, mutate its buffer in place, hand back a fresh one. No wasted copies. I wrote the signatures with `take` and ran them through the checker to make sure they held together.

They didn't. `take` requires `features: local`.

I knew that rule; I'd just forgotten it applied here. `take` is RSScript's move — it consumes a value — and moving values around is a capability you opt into with `features: local`, the performance mode. Which means a `take`-based pipeline would be usable *only* from `local` code. The entire point of the pipeline is to be the comfortable, standard way ordinary managed application code transforms a collection — the eighty percent that never touches `local`. I had designed the convenience feature in a way that excluded the exact people it was for. The checker told me, by refusing to compile two functions, that my efficient instinct was the wrong *default*.

The fix was to make every stage take `read` and return `fresh` — read the incoming pipeline, produce a new one, let the discarded intermediate get reference-counted away. No `local` required, works from plain managed code, and — I checked — it matches what the standard library's existing `List.map` already does. My "more efficient" instinct had been quietly fighting the language's own convention. I tested the corrected form: a chain of `read`-and-`fresh` stages type-checks cleanly with no `features: local`, and the chaining I'd worried might need new syntax already worked. The mutate-in-place version isn't wrong; it's just the *`local` fast path*, a thing you reach for when you've measured, not the default the feature ships as. The managed/local split from [post 3](03-less-is-more-20-80-rust.md) reappeared exactly where I wasn't looking for it.

There's a small lesson in here that I keep relearning: the discipline that makes the language good for its users is the same discipline correcting me when I design it. I wanted the fast thing. The language wanted the readable thing by default and the fast thing on request. The language was right.

## Keeping fallibility visible in a chain

One real worry remained, and it's the one that almost sent me back to option one. In a chain, where does the fallibility *show*? `xs.pipeline().map(...).try_map(...).collect()` — if `collect` can now fail because there's a `try_map` buried in the middle, then reading `collect` tells you nothing, and you're back to scanning the chain to find out if it can fail. That's option one's flaw wearing a different coat.

The fix is to put the fallibility in the *type*. A plain pipeline is a `Pipeline<T>`. The moment you call `try_map`, the value's type becomes a `FalliblePipeline<T, E>` — and a `FalliblePipeline` has a different terminal: its `collect` returns `Result`. So the second you introduce a fallible stage, the type changes, and the type forces the chain to end in a collect that returns `Result`, which forces a `?` at the call site. You cannot hide the fallibility, because it propagates through the type and changes how the line has to end. No inference, nothing to scan — the fact is carried in the open, by the type, all the way to the call. That's the same move as `try_fold` being a different name than `fold`: the fallibility is a visible, named thing, not a deduced one.

## The decision

So the answer came out as: an eager pipeline, `read`-and-`fresh` stages, fallibility carried in the type. And, just as importantly, *not* the generator, *not* effect polymorphism, and a hard line on scope — if you need the performance of fused, lazy, allocation-free iteration, that's a different tool, a `Stream` API, with its own explicit boundary. The pipeline doesn't carry the performance burden, the way `local` carries it for ownership. One mechanism for the readable standard path, a different one for when you've measured and need speed, and the reviewer always knows which one they're looking at. The managed/local philosophy, again, one level up.

The part I want to end on is the part that's easy to miss because it isn't a feature: I decided not to build the generator. For a couple of days it was the clever thing I was going to build — compiler-owned code generation for fallible combinators, a real mechanism, genuinely interesting to design. And the right call was to not build it, because once the pipeline collapsed the big axis of duplication, the boilerplate it would have saved was a couple dozen functions you write once. Article II of the constitution — features are admitted by subtraction-bias, a capability is removed or declined unless it earns its complexity — is what let me say no to my own clever idea with a reason instead of a mood. Without that article written down, I'd probably have built it, because it was fun and it felt principled, and four years later the someone-maintaining-this would be staring at a code-generation mechanism that exists to save twenty functions.

The most useful output of that design session was a feature I didn't build. That's not a failure of the session; it's the point of having a constitution. A language's character is set as much by the clever things it declines as by the things it ships, and the discipline to decline is the whole bet of this project, applied, for once, to me.

---

*Next: [Patterns are just places](09-patterns-are-just-places.md) — the interpreter asked for stronger pattern matching, I braced for a hard ownership problem, and the language had already solved it.*
