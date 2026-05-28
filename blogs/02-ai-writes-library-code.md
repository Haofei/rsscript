# Why AI writes Rust like library code

In the [previous post](01-100k-lines-of-ai-rust.md) I listed the patterns that kept hurting when I reviewed 100k+ lines of AI-generated Rust. `Arc<Mutex<HashMap<...>>>` stacked four deep. Eight trait bounds where one would do. `Pin<Box<dyn Future<...>>>` blocking the view of what a function actually does. Every function `pub`. Every parameter `impl Into<T>`.

What I want to argue in this post is that those patterns are not random noise. They are a *consistent literary style*, and the style has a specific shape: **AI writes Rust as if it were writing a library, even when the code is unambiguously application code with one caller in the same repository.** The technical complexity is downstream of the wrong register. If you can see why AI is stuck in library voice, the surface patterns stop looking like a string of unrelated mistakes and start looking like a single failure mode that you can actually fix.

## What "library voice" means

Library code and application code are different literary registers, the way a research paper and a personal letter are different registers. They share grammar and vocabulary, but the choices made within them diverge sharply, because the *audience and the unknowns* are different.

A library is written for unknown future callers. The author cannot know who will call the function, with what types, in what context. So library authors:

- Constrain inputs with trait bounds that handle the widest plausible set of callers
- Use `impl Trait` parameters to allow type conversions at the call site
- Mark items `pub` selectively, because what's public is forever
- Define error types that union every failure mode any caller might want to discriminate
- Write doc comments that imagine someone reading them on docs.rs in two years
- Treat parameters defensively, because the author can't audit the call sites

Application code is written for known immediate callers. The author *does* know who will call the function — usually it's the next function over, sometimes a handful of other modules, all in the same repo, all written by the same team. So application authors:

- Use concrete types matched to the actual callers, not generics for hypothetical ones
- Skip flexibility (`impl Into<T>`) the caller doesn't need
- Mark items `pub` only when something outside the module truly needs them
- Define error types narrowly, matched to what *this* code path can actually return
- Skip the doc comments on internal helpers because the next reader has the whole file in their head
- Trust their callers, because they wrote them

When a senior engineer writes application code, they consciously or unconsciously stay in the application register. The result is shorter signatures, narrower bounds, concrete types, and code that reviews quickly because the local context contains everything the reviewer needs.

AI doesn't do this. AI writes in library register regardless of context. *Why?*

## 1. The training data is overwhelmingly libraries

Look at the Rust that exists in public on GitHub. The high-quality, high-star, well-documented Rust — the Rust that ends up over-weighted in any reasonable training run — is almost entirely:

- Crates published to crates.io (libraries by definition)
- The Rust standard library (a library)
- The Rust compiler and toolchain (internal library code, written defensively because everyone depends on it)
- Tutorial code that *teaches* idiomatic library design
- Famous Rust projects (servo, deno, foundationdb) that are themselves giant libraries or have library cores

What's missing? Most production application Rust. The internal Rust at companies that use it heavily — Discord, 1Password, Cloudflare, Microsoft, Mozilla, Amazon — sits behind closed source control. The Rust that ships consumer products, internal services, ETL pipelines, and CLI tools is not public, except for the few cases where companies deliberately open source it (and those tend to be the *library-shaped* parts).

The result is a training set that systematically over-represents library voice. The model learns "this is what good Rust looks like" by averaging over thousands of examples, and the average example is from a crate written for unknown callers. The model has no way to know that application Rust would be 70% of the actual demand if anyone could see it.

This is structural, not fixable by better RLHF, not fixable by adding more data, not fixable by waiting for the next model. The public Rust corpus is library-shaped because the economics of open source make it library-shaped. Until that changes — which it will not — AI's Rust prior will be a library prior.

## 2. AI does not know who its caller is

This is the deeper version of the same problem. When a senior engineer writes a function, they have a mental model of who will call it. Often it's *the function they just wrote two minutes ago*. They know the call site uses a `String`, not "something that converts to a String." They know the caller already has a `&Database`, not "anything that can produce a reference to a Database." So they write narrowly.

AI doesn't have this. When AI generates a function, it has no privileged view of the caller. Even when the caller appears two lines later in the same prompt, the model is, in some real sense, generating each function independently — predicting tokens conditional on what makes a good standalone function, not conditional on what the specific caller needs. So it defaults to *maximum flexibility for unknown future callers*, which is exactly the library register.

In other words: **AI is permanently writing for an unknown caller, even when the caller is right there.** It cannot project itself into the position of someone who knows the program's whole context. The library voice is what falls out of that limitation.

This is also why prompt engineering helps but doesn't fully fix the problem. You can tell the model "you are writing internal application code with one known caller, don't add defensive bounds" — and the model will *initially* comply. After a few hundred lines, the model drifts back to library voice, because the bulk of its training is pulling it that way and the prompt instruction is a small counterweight against a massive prior.

## 3. RLHF rewards code that looks thorough

The third force pushing AI toward library voice is the alignment process itself. During reinforcement learning from human feedback, annotators rate responses. The annotators are trying to identify "high-quality code." But quality is hard to judge in the abstract, so annotators rely on visible signals: are the types specific? Are the error cases handled? Are the trait bounds present? Is the function documented?

Code that has *all the marks of professionalism* — explicit bounds, comprehensive error types, thoughtful generics, doc comments — looks more competent than code that doesn't, even when the bare version is genuinely the right level of abstraction for its context. So the rated-higher responses tend to be the more library-voice responses. The model learns: when in doubt, add more types and more bounds.

This is the most pernicious of the three forces because it's invisible. Nobody intends for RLHF to push toward over-engineering. It just falls out of "we evaluate code on local correctness signals." Each individual rating is reasonable. The aggregate creates the bias.

## 4. Library voice is a literary style, not just a type-level habit

I want to dwell on this because it's the piece that surprised me most. When you look at AI-generated Rust at a distance — squinting past the specific types — the code *reads* like library code in ways that aren't just about the types:

- It defaults to `pub fn` even for items that obviously should be `fn`
- It writes doc comments on internal helpers, in the voice of someone explaining a public API
- It chooses parameter orderings that anticipate multiple use cases ("I'll put the most general thing first")
- It defines error variants for failure modes that can't happen in *this* code path
- It returns `Option<Result<...>>` because it imagines a caller that wants to distinguish "not present" from "errored"
- It exposes builder patterns and configuration structs that no caller actually configures

This is a *voice*, in the literary sense — a consistent set of stylistic choices that signal "this is for external consumption." A reviewer reading this code feels it before they can articulate why. The code *sounds wrong for its context*, even when nothing in particular is technically incorrect.

This matters because if the problem were just "AI uses too many trait bounds," you could fix it with a linter. The voice problem is deeper. You can't lint your way out of code that's consistently authored for the wrong audience. You can teach a linter to flag `impl Into<T>` parameters, but you can't teach it to recognize that the entire register of the code is wrong for an internal helper module. Voice is everywhere at once.

## 5. Library voice is also defensive voice

There's one more force worth naming. Library authors write defensively because they have to. A library that crashes on bad input is a worse library than one that returns a clean error. A library that assumes its callers are well-behaved is a worse library than one that validates. So library Rust accumulates defensive bounds (`Send + Sync + 'static`), defensive error types (`Box<dyn Error + Send + Sync + 'static>`), defensive parameter constraints (`impl Into<T>`).

Application code mostly doesn't need this. The application has fewer callers and the callers are well-known. The error types can be narrow. The bounds can be tight. The cost of being defensive is *paying for safety you don't need*, in the form of signature noise and reviewer time.

AI doesn't make this calculation. It can't, because it doesn't know it's writing application code. So every function it writes is defensive. Across a 400-line module, defensive bounds and defensive error types and defensive parameter constraints add up to roughly the cost of an extra review pass — for no benefit, because there were never any unknown callers to defend against.

## The voice problem and what to do about it

Now here is the unhappy truth: you cannot fix the voice problem with better Rust, better prompts, or better models. The voice problem is *structural*. It's downstream of three things that are unlikely to change:

1. The public corpus is library-shaped because of open-source economics.
2. AI cannot know its caller because that knowledge isn't in the prompt-time context.
3. RLHF rewards visibly-thorough code because raters need visible signals.

So if you want AI-generated systems code that reads like application code, you have three options:

**Option A: Heavy prompt engineering plus aggressive review.** You write long system prompts that try to force application register. You accept that the model will drift after a few hundred lines and you re-engage. You do constant code review to push back against library voice. This works, but it consumes the human attention that AI was supposed to save. It's the worst of both worlds — AI generates fast, but the review cost is unchanged.

**Option B: Better post-hoc tooling.** Linters, refactoring assistants, "simplify this signature" passes. This shifts some work from review to automated fix-up. But it doesn't address the voice issue: a function that's *systematically* over-engineered cannot be locally simplified into a well-designed function. The structure has to be right at generation time.

**Option C: Change the surface AI generates against.** If the language the AI is generating doesn't *have* the library-voice constructs — no `Pin<Box<dyn Future<...>>>` because there's no exposed `Pin`; no `Send + Sync + 'static` chains because the concurrency model doesn't need them; no `impl Into<T>` because named arguments make it unnecessary; no eight-deep `Arc<Mutex<...>>` because managed values are the default — then the model *cannot write library voice in that language*. The voice is forced toward application register by what the language allows.

This is the path I ended up on. The next post is about it.

The argument so far is independent of any specific solution. It's that **AI writes Rust like library code because of structural forces that won't go away on their own, and the patterns I documented in the first post are downstream of one bigger thing: AI is in the wrong register, not making a string of unrelated mistakes.** If you take only one thing from this post, take that.

Once you see it, you start to see it everywhere. Every time AI generates a `pub fn` that should be private, every time it adds a `'static` that isn't needed, every time it returns `Box<dyn Error + Send + Sync + 'static>` instead of a concrete error — it's the same underlying thing. AI doesn't know what code it's writing, so it writes library code. By default. Every time.

---

*Next: [Less is more — the 20% of Rust that does 80% of application code](03-less-is-more-20-80-rust.md) — what a Rust-shaped language designed for application voice actually looks like.*
