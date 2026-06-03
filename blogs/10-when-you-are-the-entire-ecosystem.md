# When you are the entire ecosystem

One more design note before this series pauses to wait for the milestones it keeps promising. [Post 6](06-teaching-a-language-that-doesnt-exist.md) was about teaching a language with zero training data — the guide you write, the toolchain you build because you inherit none. This post is about the part of that I underestimated, which is what happens to all of it when there is nobody to catch your mistakes. It's also the post with the most actually-shipped code in it, because the discipline I'm about to describe is one I spent a real chunk of this session building, and it caught me being wrong in a way I want to show you, because it's funny and because it's the whole argument.

## The quiet privilege of having users

When you maintain a normal language, or a normal library, there is a thing you get for free that you never notice: other people run your stuff. They read your docs, type the example, get an error, and file a bug. They notice when the documentation says one thing and the compiler does another. The gap between what you wrote down and what your code actually does is *found*, constantly, by a distributed swarm of people who are not you, who have no respect for your intentions, and who will paste your own example back at you with a screenshot of it failing. That swarm is a correctness system. It's most of what keeps a mature project's documentation honest. And it is entirely invisible until you don't have it.

RSScript doesn't have it. There are no users yet; the swarm is a population of one, and it's me, and I am the worst possible auditor of my own work because I know what I *meant*. I read my own example and I see the intended behavior, not the actual one. The drift between what I document and what the compiler does is exactly the kind of thing I am constitutionally unable to notice by rereading, because rereading runs the version in my head.

So the discipline that falls out of a zero-user language is: **the ecosystem has to check itself, because there is no one else in it.** Every place where two artifacts are supposed to agree — the docs and the compiler, the highlighter and the lexer, the lockfile and the source — has to be wired together by a test that fails when they drift, because there is no swarm to notice the drift for me. With users, self-checking is good hygiene. With zero users, it's load-bearing, because the cost of a silent drift isn't a bug report — it's shipping a lie and never finding out.

## The lie I almost shipped

Here is the one that made me build the rest of it.

The guide a model reads to learn RSScript — the five-hundred-line document from [post 6](06-teaching-a-language-that-doesnt-exist.md) that *is*, for the model, the entire language — had a section on control flow. In it, I had written a loop like this:

```rust
loop condition { ... }
```

That syntax does not exist in RSScript. The real conditional loop is `while condition { }`; a bare `loop { }` takes no condition at all. I had simply made it up, fluently, while writing the guide, because it's a reasonable shape that exists in other languages, and I wrote it with exactly the same confidence as the lines around it that were correct. I did not notice. Rereading it, I saw a loop with a condition, which is what I meant, so it looked right.

Now sit with what that document is. It is not a tutorial a human skims while also having the compiler open to check against. It is the *only RSScript a model has ever seen.* A model reading that guide has no independent knowledge to contradict it — there is no training data, no Stack Overflow, no second source. Whatever the guide says, the model believes, completely, because the guide is the ground truth by construction. A wrong snippet in that document is not a bug a model will trip over and learn from. It is a falsehood I am installing, with full authority, into every model that reads the guide, which will then generate `loop condition { }` confidently forever and be baffled when the compiler rejects it. I had written a lie into the one document least able to survive one.

I did not catch this by rereading. I caught it because I'd started building the thing that makes rereading unnecessary.

## The fix is the discipline

The fix was a test. A small one. It reads the guide, pulls out every code snippet, and runs each through the actual compiler's front end, and it fails the build if any snippet is not valid RSScript. The guide is no longer allowed to *contain* code the compiler rejects, because the build won't go green. When I added it and ran it, it failed, and it pointed at `loop condition { }`, and that is how I learned I had been about to teach every model on earth a syntax I'd invented.

There's a precise lesson in the shape of that. The reason the guide is trustworthy *to a model* is now the same reason the whole language is trustworthy *to a user*: a single authority — the compiler — checks the claim, and you don't get to rely on your own memory of what's true. The guide doesn't describe RSScript anymore. It is *tested against* RSScript, the same way a user's program is. I'd written, in [post 8](07-the-agent-that-writes-the-language.md), that the compiler is necessary and not sufficient and that review is the bottleneck. This is the other half: the things you write *about* the language — the docs, the examples, the teaching material — need the same checking the code does, because for a zero-data language they are not commentary on the product, they *are* the product, and an unchecked claim in them is a shipped defect.

## The pattern, everywhere it applies

Once you've been bitten like that, you start wiring the same seam shut everywhere two things are supposed to agree, and a pattern emerges that I now reach for reflexively: **one source of truth, generated glue, and a freshness test that fails when the glue rots.**

The editor's syntax highlighting is not hand-maintained against the lexer; it's *generated* from the lexer's keyword table, and there's a test that fails if the generated grammar and the lexer ever disagree. The highlighter cannot color a keyword the language doesn't have, or miss one it does, because both come from one list — change the list and either the grammar regenerates or the build goes red. The semantic lockfile for a package is regenerated from the resolved dependency graph and checked for staleness, so a lock that's drifted from the source it's supposed to pin is a failure, not a silent inconsistency someone discovers months later. The language server doesn't reimplement the checker; it *is* the checker, wrapped in a protocol, so the squiggles in the editor cannot disagree with the command line because they're the same code producing the same diagnostics with the same stable codes.

None of these is clever. Each one is the same small move: find the place where two artifacts are supposed to say the same thing, make one of them generated from the other or both from a shared source, and write the test that fails the moment they diverge. It's tedious. It's also the entire substitute for a user base. Every one of those tests is a stand-in for the bug report I'm not going to get, the person who isn't going to paste my broken example back at me, the swarm that doesn't exist yet.

## Why this is more than hygiene

I want to be clear about why I think this is worth a post and not just a footnote about test coverage, because it would be easy to read it as "the author likes tests, how nice."

It's that a language for AI to generate has an unusually large surface of *claims about itself* — the guide, the interface contracts, the review metadata, the examples — and every one of those claims is consumed by a machine that takes it literally and has no independent way to check it. A human reading sloppy docs applies judgment and skepticism; they've seen other languages, they pattern-match, they go "that can't be right" and try it. A model conditioned on your guide does none of that. It extends the patterns you gave it, faithfully, including the wrong ones, with no internal alarm. The blast radius of a false claim is therefore much larger than in a human-consumed language, and it's silent, because the model won't tell you it learned something wrong — it'll just generate it and you'll find out, if you ever do, from the failures downstream.

Which means the discipline isn't optional polish you add when you have time. For a language whose distribution is "the spec is the product and the consumer is a machine," self-checking *is* the quality system. There is no other one. A reputable, more mature project in this space has reached the same conclusion from the other direction and built expect-testing and documentation-as-tests in as first-class language features — because the same forces produce the same answer. I'm reaching for the same discipline with hand-rolled guard tests because the alternative is shipping a guide full of `loop condition { }` and teaching a generation of models a language I made up by accident.

You are the entire ecosystem. So the ecosystem has to check itself, with a thoroughness that feels excessive right up until the morning a test you almost didn't write tells you you were about to lie to every model on earth.

## Where the series pauses

That's the workbench for now. Across these last three posts — [the feature I declined to build](08-the-feature-i-didnt-build.md), [the pattern matching the language had already solved](09-patterns-are-just-places.md), and the self-checking that caught me teaching a syntax that doesn't exist — the throughline is the same one from the very first post: the bottleneck is review, and a tool earns its keep by making the truth cheap to see, whether the thing being reviewed is generated code, a language design decision, or the documentation itself.

The next posts I owe this series are the ones I keep deferring on purpose, because they're the only ones that count and they require running software, not arguments: real benchmarks behind [the performance claims](03-less-is-more-20-80-rust.md), the constrained-decoding layer from [post 6](06-teaching-a-language-that-doesnt-exist.md) turned from a draft into something that actually constrains a real decoder, and self-hosted validation at a scale where the language is checking itself on programs I didn't write to flatter it. Until one of those is real, more posts would be more words, and the one thing this series has tried not to do is mistake words for evidence. So it pauses here, and picks back up when there's a running thing to point at. That's the deal I made in [post 4](04-why-no-one-built-this-before.md), and I'd rather keep it than fill the gap with noise.

Thanks for reading this far. The problem is real, the moves are visible, and the only honest thing left to do is build the parts that have to run before they can be written about.

---

*The series so far: [1](01-100k-lines-of-ai-rust.md), [2](02-ai-writes-library-code.md), [3](03-less-is-more-20-80-rust.md), [4](04-why-no-one-built-this-before.md), [5](05-explicit-is-a-budget.md), [6](06-teaching-a-language-that-doesnt-exist.md), [7](07-the-agent-that-writes-the-language.md), [8](08-the-feature-i-didnt-build.md), [9](09-patterns-are-just-places.md), [10](10-when-you-are-the-entire-ecosystem.md).*
