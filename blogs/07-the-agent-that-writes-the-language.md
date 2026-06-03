# The agent that writes the language

Six posts of argument. [The problem](01-100k-lines-of-ai-rust.md): AI-generated systems code is expensive to review. [The diagnosis](02-ai-writes-library-code.md): it's written in the wrong register, for structural reasons. [The shape of the fix](03-less-is-more-20-80-rust.md): a smaller, review-first language that lowers to Rust. [Why now](04-why-no-one-built-this-before.md), [where to spend explicitness](05-explicit-is-a-budget.md), [how to teach a language no one has seen](06-teaching-a-language-that-doesnt-exist.md). Every one of those posts ended on the same honest note: words are a hypothesis, and only working software is evidence.

This is the first post with software in it. Not a benchmark, not a proof — one real program. But it's the first time the argument had to survive contact with a running thing, and the most valuable thing the program produced was not the program. It was a bug.

## The first real program is an agent that writes the language

The first non-trivial thing I built in RSScript is, fittingly, an AI code agent. It talks to an OpenAI-compatible chat-completions endpoint, runs a narrow set of tools — read a file, write a file, edit a file, run a shell command, run the RSScript checker, query the IDE services, finish — and feeds the structured results back into the next model turn. It's a simplified Codex loop, about fourteen hundred lines of RSScript across nine files, split by responsibility: config, protocol, conversation state, the tool types, the tool schemas, the file tools, the command tools, the dispatch, and the loop.

I picked it on purpose, and not only because agents are what everyone's building. There's a recursion in it that I wanted to feel directly. The entire premise of this series is that a language can be designed to make *AI-generated code* cheaper to review. So what better first program than the AI agent itself — a thing that generates code, written in the language that exists to make generated code reviewable. If the thesis is real, it should hold when I point it at its own tooling. If it's vapor, building the agent is where I'd find out.

## Where the language held up

The good news first, because it's the boring kind and I want to get it out of the way honestly. The language mostly disappeared, which is the highest compliment you can pay a tool.

Across fourteen hundred lines there is not one `Arc`, not one `Mutex`, not one `RwLock`, not one lifetime annotation, not one `Pin<Box<dyn Future>>`. Managed-by-default ([post 3](03-less-is-more-20-80-rust.md)) meant the chat history is just a `List<ChatMessage>` I pass around with `read` and `mut`, and the runtime handle machinery that would be four `Arc`s deep in idiomatic AI Rust ([post 1](01-100k-lines-of-ai-rust.md), pattern 1) is simply not in the source. The tool dispatch is the shape the language wanted it to be:

```rust
fn execute_core_tool(request: read ToolRequest, config: read AgentConfig) -> fresh ToolAction {
    match request.name {
        "read"  => { return execute_read(request.arguments, config) }
        "write" => { return execute_write(request.arguments, config) }
        "shell" => { return execute_shell(request.arguments, config) }
        ...
        _ => { return ToolAction.error(content: read "unknown tool") }
    }
}
```

Every argument named, every reference carrying its `read` or `mut`, the runtime a sealed sum type you `match` exhaustively, tool results threaded back into the history as structured `role=tool` messages instead of pasted into a prose transcript. None of that is clever. All of it reviews fast, which was the entire point — I could read a tool's whole contract from its signature without descending into the body to find out what it mutated or retained. The register ([post 2](02-ai-writes-library-code.md)) was application voice from the first line, because the language doesn't have the library-voice constructs to drift into.

If the post ended here it would be a pleasant, unconvincing advertisement. It doesn't end here.

## The compiler was happy. Review wasn't.

[Post 1 closed on a section](01-100k-lines-of-ai-rust.md) titled "The compiler was always happy." The whole series rests on it: that the Rust compiler accepting AI-generated code tells you almost nothing about whether the code is reviewable, or even whether it does the right thing. I believed that about *Rust*. I did not expect to catch my own language red-handed proving it, in the first real program, within the first review pass.

The agent's main loop has four failure paths — an HTTP error, a tool aborting, the step budget running out, a transport failure. Every one of them did this:

```rust
state.failed = true
state.failure_reason = reason.copy()
Assert.equal(left: read "agent completed", right: read reason)
```

`Assert.equal` is a *test* primitive. It panics when its two arguments differ. Those two arguments — the constant string `"agent completed"` and an error reason — can never be equal. So every failure path was an unconditional panic, dressed up as an equality check. The code was using a test assertion as a "crash with this message" mechanism. And the function it lived in was declared `-> Result<Unit, HttpError>` and *never returned `Err` anywhere* — the entire error channel of the signature was unused, while failures aborted the process through a testing helper.

`rss check` was perfectly green. Zero diagnostics. The types all line up — `Assert.equal` takes two strings, it got two strings, the function returns `Result<Unit, ...>` on its happy path, everything is locally, mechanically correct. The checker had no possible basis to complain, the same way the Rust compiler has no basis to complain about `Arc<RwLock<HashMap<...>>>`. It's not wrong. It's just not what anyone meant.

A human reading the function caught it in about thirty seconds, because the *intent* was legible enough that the violation stood out: a function whose signature promises to return errors, surfacing errors by panicking instead, using a primitive whose name says "assert" to mean "abort." That gap between what the signature claims and what the body does is exactly the review-cost surface this language is built to shrink — and here it was, in my own code, caught by exactly the activity the whole project says is the bottleneck. The compiler is necessary. It is nowhere near sufficient. I have never believed that sentence more than the moment I found this.

## The fixes were design lessons, not compiler errors

What made it worth a blog post is that fixing it surfaced three things, none of which a type checker could have told me.

First, the error type was *wrong*, and the language quietly told me so when I tried to do it right. The honest fix is to return `Err` on the failure paths — but `Err` of what? The signature said `HttpError`, and it turns out `HttpError` has no public constructor; you can only get one back from an HTTP call. Several of the failures — a tool aborting, the budget running out — aren't HTTP errors at all and can't be made into one. The signature had been lying about the shape of failure, and the panic-hack was how it got away with the lie. The real fix was to change the return type to `Result<Unit, String>` and let the existing `state.failure_reason` actually drive a single, honest exit: `if state.failed { return Err(reason) }`. The dead bookkeeping became load-bearing.

Second, the loop tracked token usage on every turn and *did nothing with it* — it accumulated a running total and logged it and never once checked it against a budget. That's not a bug the checker can see; it's a design smell you only notice reading the whole loop and asking "what stops this from running forever on a hard task?" I added a token budget: a real agent is bounded by *tokens*, not just by step count, and the data to enforce it was already being collected and thrown away.

Third — and this is the one that turned into more work than I expected — building real software in the language is what reveals the missing twenty percent. To re-lock the package after my edits I reached for `rss pkg lock`, the command the design docs describe for exactly that. It didn't exist. The library function existed; the CLI subcommand had never been wired up. I had to regenerate the lockfile through an embarrassing throwaway test, and then I went and wired the missing commands into the CLI properly, because the alternative was a spec that lied about what the tool could do. None of that surfaces from writing examples. It only surfaces from trying to ship a real thing and hitting the wall where the language's story and the language's reality diverge.

## The thesis ate its own tail and held

Here's the shape of the whole thing, and why the recursion was worth chasing.

I built an AI agent — a code generator — in a language designed to make code generators' output reviewable. The first thing I did with that language's review tooling was point it at the agent's own source. The tooling worked: `rss check` is green, the structure is application-voice, the signatures carry their effects. And then *review* — the human activity the entire series names as the real bottleneck — caught a class of bug the tooling structurally cannot, in code the compiler was entirely satisfied with. The project caught itself making exactly the mistake it exists to make cheap to catch.

The honest and more interesting result is that the bug was *mine*, not an AI's. I'd half-expected the dogfooding story to be "look how the language caught the model's mistakes." It's better than that. The review-first apparatus isn't a machine-error detector. It's a register and a discipline that make a certain kind of mistake — locally correct, globally wrong-intent, invisible to the type system — *fast to see no matter who wrote it*. That's a stronger claim than "it babysits the AI," and the agent proved it on its own author.

## What this is and isn't

One program. Fourteen hundred lines. It is not evidence about performance ([post 3](03-less-is-more-20-80-rust.md)'s managed-vs-Python claim is still a design target with no benchmark behind it), it is not evidence about scale, and it is certainly not evidence that anyone but me will ever want to write RSScript. It's the first time the argument had to run instead of merely read well, and the single most valuable thing it produced was a bug `rss check` couldn't see and a thirty-second read could.

That is the entire series compressed into one anecdote. The compiler is necessary and not sufficient. Review is the bottleneck. A language earns its keep by making the review cheap — and the proof that it's working is not that the checker is green, but that when something is wrong, you can *see* it, quickly, in the signature and the structure, before it ships.

What's next is the unglamorous part I keep promising and keep deferring because it's the only part that counts: more real programs, self-hosted validation at scale, the constrained-decoding layer from [post 6](06-teaching-a-language-that-doesnt-exist.md) turned from a draft into something running, and actual benchmarks behind the performance claims instead of design targets. I'll write about each when there's a running thing to point at, not before. The agent was the first. It held, it humbled me, and it found a bug in its own author's code. That's a better first result than I had any right to expect.

---

*Next: a run of shorter design notes from the workbench, written between the milestones — starting with [The feature I didn't build](08-the-feature-i-didnt-build.md).*
*Previous posts: [1](01-100k-lines-of-ai-rust.md), [2](02-ai-writes-library-code.md), [3](03-less-is-more-20-80-rust.md), [4](04-why-no-one-built-this-before.md), [5](05-explicit-is-a-budget.md), [6](06-teaching-a-language-that-doesnt-exist.md).*
