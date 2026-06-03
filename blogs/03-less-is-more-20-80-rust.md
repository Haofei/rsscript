# Less is more: the 20% of Rust that does 80% of application code

The [first post](01-100k-lines-of-ai-rust.md) in this series was a catalog of the patterns that hurt when humans review AI-generated Rust. The [second post](02-ai-writes-library-code.md) argued the patterns are downstream of one bigger thing: AI writes Rust in library voice, by default, every time, because the public Rust it learned from is library-shaped and because it has no view of who will call its functions.

This post is about what to do about it.

The short version: most of what makes Rust expensive to review is the part you don't actually need for application code. If you take Rust, keep the parts an application engineer reaches for ninety percent of the time, and cut the rest, you get a language that is roughly as capable as Rust for the kind of code most people actually write — and dramatically cheaper to read, both for humans and for AI to author. I've been calling this thing RSScript. The point of this post isn't to sell the implementation; it's to argue that the *shape* of such a language is the right answer to the problem the first two posts described.

## The 20/80 cut

Here is, roughly, what stays and what goes when you draw the line at "application-level systems code."

**Stays:**

- Classes, structs, resources, simple control flow, and namespaced calls
- `Option<T>` and `Result<T, E>` with the `?` operator
- Named arguments at call sites
- Memory safety, with managed code using reference-counted handles by default and `local` regions lowering to ordinary Rust ownership
- Generics, but bounded parametric only — no GATs, no higher-rank trait bounds, no associated type families
- Limited generic bounds for review-relevant categories; full Rust-style traits are not part of the v0.5 surface
- Scoped resources via `with` (like `using` in Python, `defer` in Swift, RAII in C++)
- Compilation through Rust to native code (you keep rustc, Cargo, the crate ecosystem)

**Goes:**

- Lifetime annotations in the source language (`'a`, `'static`, `'de`, all of it)
- Higher-rank trait bounds (`for<'a> Fn(&'a T) -> ...`)
- Generic associated types
- Pin, Unpin, the entire pinning protocol
- Complex async type machinery (`Pin<Box<dyn Future<Output = ...> + Send + 'a>>`)
- Manual `Send`/`Sync` constraints on every signature
- `impl Into<T>` parameters
- Macro-heavy DSLs
- User-defined operator overloading
- Specialization, negative bounds, the unstable trait system features

You can't write the Rust compiler in what's left. You also can't write a kernel, an embedded firmware target, or a serde-style zero-copy deserializer. That is fine, because you weren't writing those things in your application service anyway. What you *were* writing — request handlers, agent runtimes, data processing pipelines, config loaders, internal CLI tools, glue code, business logic — is precisely the code that lives entirely inside the kept twenty percent.

## The shape of a signature

Concrete example. Here's a cache `put` written in idiomatic AI-flavored Rust:

```rust
pub fn put(
    cache: Arc<RwLock<HashMap<String, Arc<Image>>>>,
    key: impl Into<String>,
    value: Arc<Image>,
) -> Result<
    Option<Arc<Image>>,
    PoisonError<RwLockWriteGuard<'_, HashMap<String, Arc<Image>>>>,
> {
    let mut guard = cache.write()?;
    Ok(guard.insert(key.into(), value))
}
```

A reviewer asks one question: *does this insert a value, and does it retain the key and value afterward?* That's the load-bearing content. Everything else — the four `Arc`s, the `RwLock`, the `PoisonError<RwLockWriteGuard<'_, ...>>`, the `impl Into<String>` — is there to satisfy the type system, not to inform the reviewer.

Here's the same function in a language that drops the noise and pushes the retention information into the signature:

```rust
fn put(
    cache: mut Cache<String, Image>,
    key: read String,
    value: read Image,
) -> Option<Image>
    effects(retains(key), retains(value))
```

Four lines instead of nine. The runtime handle machinery is gone from the source — in the current single-isolate runtime that means `Rc`/`RefCell`-like internals, not something the application author has to think about. Runtime borrow conflicts become RSScript diagnostics instead of lock or guard types in the application signature. The `impl Into<String>` is gone because named arguments at the call site already give callers the flexibility they wanted from it.

And the load-bearing content — *this function mutates the cache and retains the key and value* — is now visible in the signature, not buried in the body. The reviewer reads four lines and is done.

The trick is not that the second version is cleverer. The trick is that the second version *doesn't have a lot of the language features the first version uses*. The expressive ceiling is lower. For the cases where lowering the ceiling costs you nothing (most application code), you gain readability for free.

## Resources you can see being released

The same move applies to the other thing application code does constantly: acquire a resource, use it, release it. In Rust that's RAII — correct, invisible, and a frequent source of "wait, when does this lock actually drop?" review questions, because the release is implicit in a scope you have to reconstruct in your head. RSScript makes the scope a syntactic block:

```rust
with File.open_write(path: read path)? as file {
    File.write(file: mut file, data: read text)?
}
```

The file is open exactly inside the braces and closed at the closing brace, including on the early return that `?` might trigger. A reviewer doesn't trace lifetimes to find the release point; the release point is *the brace they're already looking at*. It's the same idea as `using` in Python, `defer` in Swift, RAII in C++ — but lifted into visible structure instead of inferred from scope, because "when is this released" is a review question and review questions belong in the syntax. Pooled resources work the same way: `with ResourcePool.borrow(pool: mut pool) as conn { ... }` borrows for the block and returns the connection at the brace. The pattern is uniform, and uniform patterns are cheap to review because you learn the shape once.

## Writer cost stays small, reader gain is large

A common reaction to this design is: "but now the function author has to think about retention, that's extra cognitive load." This concern doesn't survive contact with the way the cognitive load distributes across writers and readers.

The decision about retention is made *once*, at the function definition site. It's read by every caller, every reviewer, every future maintainer, every AI agent that has to understand the function to use it. The cost is paid once; the value is delivered N times.

The same logic applies to `read` / `mut` / `take` on parameters. The function author makes one decision per parameter. The reader of every call site reads `read x` and immediately knows `x` is inspected, not modified. They don't have to descend into the function body to find out. They don't have to remember whether this particular method takes its argument by reference or by value. The naming is the documentation.

This is the central design move and it goes against the grain of most language design from the last twenty years. Most languages optimize for writer ergonomics — terse syntax, type inference, implicit conversions — because writers are the ones who feel the pain at authorship time. But code is read many more times than it's written, and AI tilts that ratio further. **It is a strictly better trade to add a small writer-side cost for a large reader-side gain when most of your code is going to be read by humans reviewing AI's output.** That's the bet.

## Managed by default, fast anyway

The other concern people raise: "if you switch from borrow checking to reference counting, isn't that slow?"

Yes, relative to native Rust. No, relative to anything else.

Reference counting has a per-access cost that borrow-checked code doesn't pay. That cost is real. It's also relative — about an atomic increment per heap-object access. Compare that to what Python does for the same operation: dispatch through `__getattribute__`, allocate a fresh boxed integer for each arithmetic op, traverse a class MRO, fight the GIL. The per-access cost of an atomic refcount on shared objects is *trivial* next to the per-access cost of running on an interpreter. Primitives stay on the stack, hot loops compile to monomorphic Rust through LLVM, there's no GIL, there's no per-object dict header. Managed-only code in this style should be much closer to compiled Rust than interpreted Python for many workloads.

That is the design target, not a benchmark claim yet. The current prototype already lowers through Rust and keeps primitives in ordinary Rust forms, but the honest performance evidence still has to come from self-hosted validation and benchmarks.

When you need to beat managed overhead — when you actually do have a hot inner loop where shared handles are the bottleneck — you reach for `local`. Local values are ordinary Rust-owned values: exclusive at the RSScript level, checked against silent retention by managed objects, and still free to contain heap-backed buffers when that is the right representation. Hot paths get a path toward hand-tuned Rust characteristics without making that the default everywhere.

The performance story, in one sentence: **default managed code should be much closer to compiled Rust than to interpreted Python; opt-in `local` gives hot paths a Rust-owned representation**. Most code never needs the second mode. The 20/80 cut applies to performance too: 20% of the language complexity gets you 80% of the performance that matters in practice.

## Constraint is the product

The earlier posts argued that AI writes Rust in library voice because of structural forces that won't change. A reasonable response is: well, can we add lints to Rust that force application voice?

The answer is no, and the reason is worth being precise about. Lints can flag specific patterns, but they can't change the *surface area* of the language. As long as `Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>` is a thing that exists in the language AI is generating, AI will generate it, because that pattern exists thousands of times in its training data and gradient descent will keep finding it. A lint that says "don't do that" is one weak signal against a massive prior.

A smaller language fixes this from the other end. **In a language that doesn't have `Pin`, AI cannot generate `Pin<Box<dyn Future<...>>>` patterns.** It cannot generate `Send + Sync + 'static` chains, because those constraints don't exist in the language. It cannot use `impl Into<T>` to expand parameter flexibility, because there is no `impl Into<T>`. It cannot stack `Arc<RwLock<...>>` four deep, because the language doesn't expose `Arc` and `RwLock` as construction primitives — managed values just *work* the way you'd expect.

This is the load-bearing argument for the whole project. **Constraint is the product.** A smaller language doesn't just produce smaller signatures (though it does that). It eliminates entire categories of library-voice expression that AI would otherwise reach for. The option space shrinks, and what's left is mostly application voice.

A pleasing corollary: because the language is small, the spec fits inside an LLM's context window. Twenty thousand tokens of language reference and examples is enough to put the full language in front of any frontier model. The model doesn't need to be fine-tuned to write the language. You give it the spec at the top of the prompt and it generates application-voice code from the first token, because the spec is the only RSScript it's ever seen — there's no library-voice prior in its weights to fight against.

## The escape hatch is explicit

Application register doesn't mean "no advanced features ever." Some real application code does need a high-rank trait bound, a complex async surface, an unsafe block for FFI, a zero-copy parser. The honest answer to those cases is *don't pretend the small language covers them*. The honest answer is an escape hatch.

In RSScript that's `features: native`. A file or package that needs to cross into full Rust declares the boundary in RSScript, then binds bodyless `native fn` contracts to explicit Rust wrapper functions through package metadata. The review tooling treats that as a higher-risk region that requires more attention. The escape hatch isn't a shameful retreat from the design — it's part of the design. It exists *so that* the main language can stay small without lying about coverage.

The alternative — trying to make the small language do everything — is what kills these projects. You start with a clean simplification, then a user asks for one more feature, then another, and four years later you've reproduced the complexity you set out to avoid. The escape hatch is what lets you say *no* to that pressure: "if you need the full power, drop into native; the language stays focused on the application register."

## What this is and isn't

A few clarifications, since this kind of project draws the same misreadings every time:

**This is not a Rust replacement.** Rust remains the right tool for everything that needs its full expressivity — kernels, compilers, drivers, embedded firmware, library code that has to be maximally general. The argument here is that *most code is not those things*, and most code currently being written in Rust (and being generated as Rust by AI) would be better served by a smaller surface that's easier to review.

**This is not Rust with a GC.** Reference counting is not garbage collection. There's no tracing collector, no pause times, no allocator opacity. Destruction is deterministic. Memory is freed when the last reference goes away. This is the same model Swift and Objective-C use, not the model Java and Go use.

**This is not a beginner language.** The audience isn't people who can't handle Rust. The audience is people who *can* handle Rust, are writing application code in it, and don't want to pay for advanced expressivity they aren't using — especially when reviewing code an AI generated.

**This is not a research project.** I am not trying to invent new type theory. The design is conservative: take Rust, cut the things application code doesn't need, use refcounted managed handles by default, preserve a local ownership escape hatch, push retention into signatures, lower to Rust. Every individual move has prior art going back decades. The contribution is putting them together for this specific use case at this specific moment.

## Where this matters

The case for this kind of language is strongest in environments where AI is writing a meaningful fraction of the code and humans are reviewing it. If you're a solo developer writing 100 lines of Rust a week by hand, you don't have a problem this solves. If your team is shipping thousands of lines of AI-generated Rust per week and burning out the senior reviewers, you do.

That second category is going to be most teams, in two years. The bottleneck is shifting from generation capacity to review capacity, and the languages that win this decade are the ones designed for that ratio. RSScript is my bet on what one such language looks like for the systems-ish slice of that world. It might not be the one that wins, but the shape — small surface, application register, refcount default, lower to a serious backend, designed for in-context use by AI — is right. If it isn't RSScript, it'll be something with the same outline.

For now, you don't need to use it. You don't need to care about it. The argument I've made across three posts stands on its own: AI-generated systems code is in the wrong register for review, the wrong register has structural causes that won't go away, and the way out is to change the surface AI generates against, not to lint it harder. The 20/80 cut is the cheapest way to change the surface. Constraint is the product. Less is more.

---

*Next: [Why no one built this before](04-why-no-one-built-this-before.md) — a tour through prior attempts (Hylo, Roc, Vale, Mojo, Nim, Carbon) and what's different about doing it now.*
