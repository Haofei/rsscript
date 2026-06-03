# Why no one built this before

If you've been following this series, the proposition is roughly: take Rust, cut the parts application code doesn't use, switch to reference counting, push retention information into signatures, lower to Rust for the backend, design the surface to fit inside an LLM's context window. ([Post 1](01-100k-lines-of-ai-rust.md), [Post 2](02-ai-writes-library-code.md), [Post 3](03-less-is-more-20-80-rust.md) made the case for each piece.)

A reasonable reaction at this point is: that sounds obvious in hindsight. Why hasn't someone already built it?

The honest answer is that several people have built things in this direction, and none of them caught fire. This post is a tour through the prior art — Hylo, Roc, Vale, Mojo, Nim, Pony, Carbon, and others — and an argument about what specifically is different about doing this now. I want to take the question seriously, because "why hasn't this been done" is a real question with a real answer, not a marketing setup.

## The shelf of "simpler Rust" attempts

A non-exhaustive list, roughly ordered by how close each gets to the same problem:

**Hylo (formerly Val)** is probably the closest. It's a research language out of Dave Abrahams' work on what he calls mutable value semantics — a model in which values are owned exclusively and mutation is local. Like RSScript, Hylo wants to be a systems-shaped language without Rust's lifetime ceremony. Unlike RSScript, Hylo has its own backend (via LLVM), targets full systems use, and is structured as a research project. It moves slowly and produces papers; it has not produced an ecosystem.

**Vale** is one person's project to build a region-based memory language with what the author calls "generational references" — refcount-like in spirit but with optimization tricks to avoid the per-access cost. It has no lifetimes, no GC, and aims at Rust-class performance. The project is alive but small, and it carries the entire weight of building its own backend, stdlib, and tooling.

**Roc** is a functional language with strict typing and refcounting under the hood. It comes out of the Elm community's design instincts and targets a similar "fast, safe, simple" niche. The differentiator is that it's deeply functional — closures, immutability, no methods — which is a great fit for some workloads and a bad fit for the systems-shaped application code where Rust is currently popular. It has not crossed into the audience that's currently writing Rust.

**Mojo** is the most ambitious of the recent crop. It's positioned as a Python superset with Rust-like ownership, primarily for ML workloads. It has Chris Lattner behind it and Modular as a commercial backer. It's a different shape of language than RSScript — it inherits Python syntax, targets ML hardware, and is part of a commercial product strategy. The interesting overlap is that Mojo also recognizes the "compile-to-fast-thing while staying ergonomic" pattern.

**Nim** has existed for over a decade and quietly does much of what people want from "simpler Rust": multiple memory models (ARC, ORC, plain refcount), Python-ish syntax, compilation to C, native performance. It's mature and used in production by a small but committed community. It has not broken out into mainstream systems use, and the reasons are partly ecosystem (smaller crate equivalent) and partly culture (Nim has its own community style that doesn't overlap much with the Rust crowd).

**Pony** is an actor-based language with capability-secure reference types. It solves a specific problem (data-race freedom without GC) elegantly and academically. Industrial uptake has been minimal — it asks too much new thinking from the user to be a casual jump.

**Carbon** is Google's stated successor to C++. Big ambitions, careful design, slow progress. It's solving a different problem (C++ migration) than the application-Rust problem this series describes. Not really a direct comparison, but it lives on the same shelf.

**Crystal** is "Ruby that compiles." Static types, native performance, refcounted-ish GC. It hit a small audience and stalled. **Gleam** runs on the Erlang BEAM with strong types and refreshingly simple syntax — it's growing but lives in the BEAM niche. **Lobster** is a game-focused language with a clever refcount-elision compiler. **Zig** is a C alternative that competes for systems mindshare with a different value proposition (manual memory, explicit allocation, no hidden control flow).

So the shelf is not empty. People have been trying.

## What every prior attempt is missing

Looking across the list, each project has at least one of the following gaps. Most have more than one.

**Most of them built their own backend.** Hylo, Vale, Roc, Mojo, Nim, Pony, Crystal, Gleam, Lobster, Zig — every project on the list either compiles directly to LLVM, transpiles to C, or runs on a VM. That means each one carries the entire weight of *also* shipping a code generator, optimizer, platform support layer, allocator, debugger, and so on. Building a competitive backend is the work of a major team over a decade. The Rust backend is the work of hundreds of engineers over fifteen years. A new project trying to do the same is, in practice, signing up to permanently lag Rust on raw machine output.

This is the single most-underrated lesson. **You don't need to build a backend if Rust already exists and you can lower to it.** A language that lowers to Rust source inherits, for free, every optimization the Rust ecosystem has accumulated — and continues to inherit them as Rust improves. You give up nothing on the backend axis. You spend zero engineering effort on platforms, debuggers, allocators, codegen. The entire effort goes into the front end, which is where the *value* of a smaller language lives anyway.

This is so cheap that it should be the obvious move. It isn't, because language designers are trained to think a "real" language has its own backend. Building on top of someone else's compiler feels like cheating. It also feels small, because there's less to build. But the value isn't in the backend; the value is in the front end. RSScript is structured around accepting that asymmetry.

And it matters that the backend is *Rust* specifically, not just "some lower-level language." Lowering to C would inherit a backend but not its safety — you'd be one generation bug away from a use-after-free, and your shiny review-first front end would sit on top of an unsound foundation. Lowering to Rust means the generated code goes through the borrow checker, so the lowering itself has a second, independent verifier: if my front end emits something that violates ownership, rustc rejects it, and that's a bug in my compiler I find immediately instead of a memory-safety hole I ship. The managed model maps cleanly onto reference-counted Rust types; `local` maps onto ordinary Rust ownership; the `with` blocks map onto RAII. The semantic distance between RSScript and Rust is small enough that the lowering is mostly mechanical and the mapping is auditable — which is the whole point, because [the review-first promise](03-less-is-more-20-80-rust.md) would be hollow if the thing you actually run were a black box. You can read the generated Rust when you need to. You rarely need to. But "rarely need to, can when you must" is only possible because the target is a safe, high-level-enough language, not a raw codegen layer.

**Most of them predate the AI review crunch.** Hylo, Vale, Roc were designed when the bottleneck was *humans writing code*. Their pitch was "Rust is too hard to write, here's something easier." That's true, but it has always been true, and people who learn Rust mostly stop finding it hard after a few months. So the value of "easier to write" has a hard ceiling: it competes with the user's willingness to spend a quarter learning Rust.

What changed in 2024-2025 is that the bottleneck moved from writing to *reviewing*. AI generates code at superhuman rates; the review capacity stays human. The value proposition for a smaller language changes from "easier to write" (which has a fixed ceiling) to "easier to review" (which scales with how much code AI is generating). That value proposition was not available to projects designed before AI. They were solving the right problem at the wrong time — or rather, the value of solving it was orders of magnitude lower then than it is now.

**Most of them don't put review at the center.** Read the design docs for Hylo, Roc, Vale, Mojo. The vocabulary is about ownership, mutation, safety, performance, expressiveness. The vocabulary is *not* about review. Review is treated as a side effect of good design, not as the primary design objective. So the languages end up with cleaner type systems and simpler semantics but don't push retention, mutation, or resource information into the signature where reviewers actually need it.

A review-first language is a different design target than a "writer-friendly" language. It puts information into signatures that writers would rather leave out (because writing them costs the writer something). It enforces named arguments at call sites (verbose for the writer, clarifying for the reader). It requires effect annotations (extra work for the writer, indispensable for the reader). None of the projects on the shelf made these trades, because none of them were aimed at the review bottleneck. They were aimed at writers.

**None of them treat in-context learning as a design constraint.** This is the most subtle gap. Before 2023, a new language's adoption story required: build community, write tutorials, attract package authors, get into training data, wait for models to be re-trained. The whole cycle takes years. Hylo and Roc and Vale are still inside that cycle, slowly accumulating mindshare.

After 2023, there's a shorter path: design the language so its entire reference can sit inside an LLM's context window. Twenty thousand tokens of spec and examples is enough to put any frontier model into "I know this language" mode. The model doesn't need fine-tuning. The user doesn't need years of accumulated tutorial content. You ship the spec, the user pastes it as a system prompt, and they're generating code on day one.

This is a totally new dimension for language adoption, and as far as I can tell, no language on the shelf was designed with it in mind. They were all designed assuming the long path — community-building, ecosystem, training data, time. The short path requires *deliberately* keeping the language small enough that the entire reference fits in context, and it requires writing the spec in a form that an LLM can use directly. Neither was a known design constraint when most of these projects started.

## Four conditions that converge in 2025

Putting the four gaps together, the argument is:

1. **Rust as a backend** is a fifteen-year-old, mature, optimization-rich thing that you can now treat as a code generator. Lowering to Rust source means you skip the entire backend axis of language work.

2. **The AI review crunch** is the urgent forcing function. Without it, "simpler Rust" was a pleasant aesthetic preference, but not one that survived the user's willingness to invest in learning Rust. With it, "simpler Rust" becomes the difference between teams that can keep up with AI-generated PRs and teams that can't.

3. **Review-first design** is a fresh axis nobody else has worked. The prior art is uniformly writer-first. Putting retention, mutation, resources, and native boundaries into the signature is a design move that the existing projects could have made but didn't, because it wasn't the problem they were solving.

4. **In-context learning** as a distribution mechanism didn't exist before 2023 and has been demonstrated only in the last 18 months. A language designed *to fit in context* skips the multi-year adoption cycle that every prior attempt was forced through.

Each individual condition has been around for a while. Rust has been a viable backend target for years. AI review pain has been real since GPT-4 dropped. Review-first language design has been technically possible since the 1970s. In-context learning has been a thing since GPT-3.

What's new is the *intersection*. The four conditions only co-exist starting in roughly 2024. The earliest plausible window for someone to have built this is when Rust's backend was mature *and* AI generation was fast enough to break review *and* in-context windows were big enough to hold a full spec. That's a recent intersection.

This is also the most honest answer to "why didn't anyone do it before." It's not that nobody noticed the problem, and it's not that the problem is somehow secretly hard. It's that the problem requires four independent things to be simultaneously true, and they only became simultaneously true very recently. The shelf of prior attempts is full of projects that had two or three of the four; none had all four; and the four are independent enough that you can't get them by tweaking an existing project — you have to start over with all four in mind.

## What this means for the project, and for skeptics

A few implications I think are worth being direct about.

**Hindsight makes this look obvious; it wasn't.** The lower-to-Rust idea seems trivial once you've thought about it. It isn't trivial — every language designer's training points them toward building their own backend. Carbon is doing it. Mojo is doing it. Roc is doing it. They are all leaving the biggest engineering shortcut on the table, because the shortcut feels like cheating. RSScript's biggest "advantage" is just being willing to not write its own code generator.

**There's still a survivorship bias caveat.** People who tried this and failed quietly aren't in the public record. There may be three abandoned GitHub repos of "Rust without lifetimes that lowers to Rust source" that I don't know about. The absence of *successful* prior art is real; the absence of *attempted* prior art is harder to verify. Take "no one built this before" with that grain of salt.

**The window for being early is open but not infinite.** Once the conditions stabilize, others will notice. There will be a "simpler Rust for AI" project from a large tech company within two years. The advantage of being early is in setting the design vocabulary — if RSScript's framing (managed-by-default, review-first, lower-to-Rust, in-context spec) becomes the way people think about this category, that's a long-tail advantage even if a better-resourced project arrives later. If the framing is wrong, the project is dead either way.

**The competition is mostly *not* the projects on the shelf above.** Hylo isn't competing for the same users. Roc isn't either. Mojo is in a different segment. The real competition is *Rust plus better linters* — the world where people just keep writing Rust and tooling makes review tolerable. That's a credible alternative, and it's the one that has to be argued against, not "why is RSScript better than Hylo."

**The serious version of this critique is "Rust will fix this itself."** The Rust language team is aware that Rust is hard to review at scale, and there's interest in profiles or subsets that constrain the language for specific contexts. If Rust ships an official "application profile" with built-in retention and mutation tracking, RSScript loses much of its reason to exist. That's a real risk. The mitigating factor is that Rust changes slowly and conservatively — for good reasons, but it does mean the soonest you'd plausibly see an official application profile is several years out, and it would inherit Rust's existing surface (lifetimes, traits, async) instead of starting from a clean smaller surface. There's a real gap to operate in.

## The straight answer

To put the question and answer cleanly:

*Why hasn't anyone built a smaller, review-first, Rust-shaped language that lowers to Rust?*

Because the four conditions that make it the right thing to build — a mature Rust backend, an AI review crunch, an appetite for review-first design, and in-context distribution — only converged a year or two ago, and language design responds slowly. The prior art was all working under conditions where at least one of the four wasn't true yet, so they made different tradeoffs and ended up in different places.

It's a moment, not a missing insight. RSScript exists because I happened to notice the moment from the position of someone reviewing 100k+ lines of AI-generated Rust and getting tired. Someone else could have noticed it too. Plenty of someones probably will, in the next two years. The thing to do is build the version I think is right and ship it before the moment closes.

If you've read this far, you probably know enough about the project now to decide whether you want to follow along. The MVP is close; once it's stable I'll start self-hosted validation and writing about what I learn. The earliest you'll see usable RSScript in production code (mine, not yours) is sometime in the next few months. If you want to wait for that before forming an opinion, that's the right call — words are cheap, working software is the only honest evidence.

Until then: the problem is real, the moment is real, the moves are visible. Whether RSScript is the right embodiment of those moves is what self-hosted validation has to determine. I'll write about it when there's something real to write about.

---

*That was the first arc of this series — the argument, made before the language was real. The next posts are the second arc: what actually building it taught me, starting with the design decision the argument made unavoidable.*

*Next: [Explicit is a budget, not a virtue](05-explicit-is-a-budget.md).*
*Previous: [1](01-100k-lines-of-ai-rust.md), [2](02-ai-writes-library-code.md), [3](03-less-is-more-20-80-rust.md)*
