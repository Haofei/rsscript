# Explicit is a budget, not a virtue

The [first four posts](01-100k-lines-of-ai-rust.md) in this series made an argument and stopped. The argument: AI-generated systems code is expensive to review, the cost is structural, and the way out is a smaller language that pushes review-critical facts — mutation, retention, freshness — into the signature where a reviewer can see them. [Post 3](03-less-is-more-20-80-rust.md) called that the central move: *put the load-bearing information in the type, not the body.*

I've spent the months since actually building the thing. This post is the first of a second batch — call it the part where the thesis meets the artifact — and it starts with the objection a careful reader of post 3 should already be forming.

The objection is this. [Post 1](01-100k-lines-of-ai-rust.md) was a catalog of *noisy signatures*: `Arc<RwLock<HashMap<...>>>` stacked four deep, eight trait bounds, `Pin<Box<dyn Future<...>>>`. Post 3 then said the fix is to put *more* information into the signature — effects, retention, freshness. So which is it? If noisy signatures are the disease, how is adding `read` and `mut` and `effects(retains(...))` to every line not just a different wall of noise?

That objection is correct, and resolving it turned out to be the most important design decision in the language. It is the reason RSScript opens with a binding constitution — nine articles that override every later chapter — instead of a feature list. I'll tell you which article, and why it had to be law rather than taste, but first the mistake I had to make to find it.

## The mistake: treating explicitness as a virtue

Here is the thing I had backwards at the start. I thought explicitness was a virtue — that more of it was safer, and the only real question was how much verbosity a user would tolerate before they revolted. I would look at a signature, ask "is there any review-relevant fact not stated here?", and if there was, I'd add a marker for it. The language accreted annotations the way post 1's Rust accreted trait bounds, except I felt virtuous about it, because each annotation was *true* and *review-relevant*.

The trap is that "true and review-relevant" is not a high enough bar. Half the facts about a function are true and arguably review-relevant, and a signature that states all of them is exactly the wall of noise I'd spent post 1 complaining about. I had reinvented the disease and given it a clean conscience.

The framing that got me out was this: explicitness is not the goal. *Reviewability* is the goal, and explicitness is an instrument for it with a curve that goes up and then back down:

```text
too implicit   mutation, retention, ownership are hidden in the body
               -> the reviewer can't see what to verify -> unreviewable

too explicit   every line is keyword ceremony; the one load-bearing
               marker is buried under forced and default ones
               -> the reviewer can't find what to verify -> unreviewable
```

Both ends fail, and they fail *the same property*. Post 1's noise and my hypothetical "annotate everything" language are the two ditches on either side of the same road. The target isn't maximum explicitness; it's maximum **signal-to-noise** in the review-critical information. A marker that adds a token without adding a decision moves you toward the second ditch, not away from the first.

Once I saw it that way, the rule wrote itself.

## Mark the departure, hide the norm

A reviewer scans for what is *unusual*. So the rule is: keep the safe default silent, and make only the departures from it explicit. Then every marker that appears is a signal, and the amount of annotation a function carries is proportional to the risk it actually presents.

The operative test for whether a marker earns its place is sharper than "is it true and relevant." It's: *does this marker answer a reviewer's question that has more than one possible answer?* If the value is forced — there's only one legal thing it could be — it's not a decision, and writing it down is pure cost. Same if it's the common, safe default that a reader would assume anyway.

RSScript already works this way almost everywhere, and you can feel the discipline when you read it:

- `managed` is the default and is silent; `local` (an exclusive, fast value you can move and mutate in place) is the departure and is marked.
- Non-retaining is silent; `effects(retains(key, value))` is marked, because retention is the single most expensive thing to discover by reading the body (post 1, pattern 4).
- A managed return is silent; `fresh` — a value created in the function and handed out clean — is marked.
- Synchronous, safe, allocating-normally code is silent; `async`, `native`, `unsafe`, `noalloc` are marked.

In each case the unmarked state is the safe, common one, and the markers are exactly the things a reviewer is obligated to check. The signature of a boring function is short. The signature of a risky one is long, *because it is risky*. The annotation density is the risk map.

That is the precise inversion of post 1. There, every signature was equally noisy, and the noise told you nothing about where the risk lived — the load-bearing function and the mechanical helper looked identical, so you read all twenty. Here, the noise *is* the map. You can scan a module and your eye goes straight to the three functions carrying `mut`, `take`, `manage`, `effects(retains(...))`, and skip the seventeen that carry nothing, because the seventeen are, provably, the boring ones.

## The one place it cheats, and why I left it there

There is exactly one place RSScript spells out a default instead of a departure, and I want to be honest about it because it's the cleanest illustration of the whole principle — and because pretending it isn't there would be the kind of dishonesty this series is supposed to be against. Every by-reference argument at a call site carries a data-effect keyword — including the common `read` case:

```rust
cache_put(cache: mut cache, key: read key, value: read value)
//                          ^^^^        ^^^^   the default, written at every use
```

By the rule I just stated, `read` *should* be silent. It's the least-privilege baseline; it's what you mean when you mean nothing special. Writing it on every argument is the one bit of wallpaper in the language, and when a call has four `read`s and one `mut`, the `read`s dilute the `mut` that actually matters — the exact dilution effect I just praised the language for avoiding everywhere else.

The argument people get stuck in here is whether `read` is "explicit" or "a default." It's both, on two different axes, and the confusion is just from running the axes together:

```text
meaning axis    among read / mut / take, read is the least-privilege baseline
                -> read is the semantic default

surface axis    is the token written or omitted? read is mandatory; an omitted
                effect is a hard error (the checker emits RS0202), never an
                inferred read
                -> read is syntactically explicit
```

Plot every marker on those two axes and `read` is the single odd cell: the only default that's also written out. Every other default is silent; every departure is written; `read` sits alone in the off-pattern corner.

I kept it there anyway, deliberately, and the reason is itself a review property. If `read` were omittable, a bare `f(x: value)` would be ambiguous between "I checked, this is read-only" and "I forgot to annotate." I wanted absence to be a *detectable error* — RS0202, every time — never a silent assumption. An un-annotated argument can never slip through review looking reviewed, because it doesn't compile. The cost is the wallpaper. The benefit is that mutation and consumption are always a positive, present claim at the call site, legible without chasing the callee's signature into another file or a package interface. It's a genuine trade, it's written into the spec as a deliberate exception with its justification attached, and the alternative — make `read` the omittable default so only `mut`/`take` are written — is recorded as something a future version may do, under one binding constraint: omission must stay a hard error, never become an inferred `read`.

That whole paragraph is the design philosophy in miniature. Not "be as explicit as possible." Rather: spend explicitness where it buys a review decision, and when you spend it somewhere it doesn't, *say so on the record* instead of pretending the cost isn't there.

## Why this had to be a constitution and not a style guide

Here's why I bothered to carve this into law rather than leave it as a preference. A language built for AI review is under permanent pressure to add *one more explicit marker, just to be safe.* The pressure is reasonable every single time — someone hits a case where a hidden fact bit them, and the obvious fix is to make that fact visible everywhere. Each individual request is sound. Each one costs a few tokens. And the sum, four years on, is the wall of noise from post 1 — rebuilt one well-intentioned annotation at a time, except now it's the language's own ceremony instead of Rust's. This is exactly how every "simpler X" project from post 4 slid back into complexity: not by one bad decision, but by a hundred locally-reasonable additions with no rule to stop them.

The budget principle is the rule that lets you say no to that pressure with a reason instead of a mood: *does this marker answer a review question with more than one answer? If not, it's noise, and noise erodes the signal it was meant to protect.* In RSScript it sits in the constitution right next to "constraint is the product" (post 3), because they're the same discipline pointed at two surfaces. Constraint is the product removes *capabilities* that don't earn their complexity. The explicitness budget removes *ceremony* that doesn't earn its tokens. Both are subtraction. Both protect the reader. A feature that can only be made explicit by forcing ceremony at every use is failed by the same test that kills hidden behavior — they're the two ways a feature can tax the person reading the code.

It also, not coincidentally, makes the language cheaper for a model to *generate*. Every forced token a human reads is also a token a model has to emit correctly, and a token of context it has to carry. But that turns out to be a whole article of the constitution on its own, and a whole post — the next one.

## The takeaway

If post 3 was "push the load-bearing facts into the signature," this is the necessary other half: **push only the load-bearing facts, keep the defaults silent, and treat every spelled-out default as a debt you have to justify out loud.** Explicit is not a virtue you can never have too much of. It's a budget. Spend it on decisions. The moment you start spending it on ceremony — even true, relevant, well-meant ceremony — you are rebuilding the exact problem you set out to solve, with a clear conscience, one marker at a time.

---

*Next: [How do you teach a language that doesn't exist yet?](06-teaching-a-language-that-doesnt-exist.md) — what it actually takes to make a model write a language with zero training data, and the layer that comes after persuasion.*
