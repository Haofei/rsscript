# How do you teach a language that doesn't exist yet?

[Post 3](03-less-is-more-20-80-rust.md) and [post 4](04-why-no-one-built-this-before.md) leaned on a single bet about distribution: a language small enough to fit in an LLM's context window doesn't need the usual multi-year adoption cycle. You don't build a community, seed tutorials, wait to get into the training data, and hope the next model knows your language. You write the spec, the user pastes it into the prompt, and the model generates the language from the first token.

That's a clean line in an essay. Building it turned the line into a list of things you actually have to make, and a problem I'd underestimated. This post is the list, the problem, and the layer I now think comes after.

## The starting condition is brutal, and it's also a gift

RSScript has **zero presence in any model's training data.** None. There is no Stack Overflow answer, no GitHub corpus, no "the model has seen ten thousand examples and absorbed the idioms." The only RSScript any model has ever seen is whatever is in the current prompt. The first time I asked a frontier model to write it cold — no spec, just the name and a vague description — it produced confident, fluent, completely invented syntax: Rust with the serial numbers filed off, `impl` blocks and lifetimes and all. It had no idea, and it had no idea that it had no idea.

That sounds like pure disadvantage, and for an afternoon it felt like one. But recall the diagnosis from [post 2](02-ai-writes-library-code.md): the reason AI writes Rust badly for application code is that it has a massive *library-voice prior* baked into its weights, and no prompt instruction is a match for gradient descent over millions of examples. A zero-data language has no such prior. There is nothing in the weights pulling the model toward the wrong register, the wrong idiom, the wrong anything — because there is nothing in the weights about this language at all. The confident garbage it produced cold was the *Rust* prior leaking in; the moment you give it real RSScript to condition on, there's no competing memory to fight. It's the cleanest slate a language can have. Post 2's structural curse, inverted into a structural advantage.

So the whole game becomes: what do you put in front of it, and how do you make sure the model can't get out from under it?

## Layer 1 — the guide, and the rule that it cannot lie

The first artifact is a single document — I call it `AGENT.md` — that is the language, distilled to roughly five hundred lines: the syntax, the ownership and effect model, the package manifest format, and, up front, a section literally titled "the rules you will most likely get wrong." That ordering is deliberate. A model arriving with a Rust or Swift or TypeScript prior misfires in *predictable* places, so you disarm those reflexes before you teach anything else:

```text
1. Every call argument is named: f(name: value), never f(value).
2. By-reference arguments carry a data-effect keyword: read / mut / take.
3. No implicit conversion. let p: Path = "s" is rejected; you write
   Path.from_string(value: read "s").
4. Methods are qualified by type: Image.resize(image: mut image, ...),
   not image.resize(...).
```

Those four are most of the difference between a model fluent in RSScript and a model writing Rust with a costume on. Putting them first, as prohibitions, does more than any amount of careful grammar exposition later in the document.

The non-obvious lesson came from writing the thing. A teaching document for a language with no training data has a property a normal tutorial doesn't: **a hallucinated example teaches the model wrong with exactly the same authority as a correct one.** If a human reads a tutorial with a bug in it, they hit the bug, get a compiler error, swear, and learn. If a model reads your guide as its *entire* knowledge of the language, a wrong snippet isn't a bug it will discover — it's ground truth it will faithfully reproduce, and then defend, because you told it so.

So every snippet in the guide is run through the compiler before it ships. Not spot-checked — every one. The guide is only allowed to contain code the checker accepts, because the guide is not *describing* the language; for the model, the guide *is* the language. Enforcing that caught me being wrong in my own document more than once. I had a control-flow example with a `loop condition { ... }` form that doesn't exist — the real conditional loop is `while condition { }`, and bare `loop {}` takes no condition. I'd have taught every model that misfeature with total confidence. The compiler caught me. There's a quiet lesson in that: the discipline that makes the guide trustworthy for the model is the same discipline — run it through the authority, don't trust your own memory — that the whole language is trying to give its users.

This layer is honest about what it is: **persuasion.** It raises the odds that the model writes valid code from the first token. It does not make invalid code impossible. After a few hundred lines a model can still drift, especially if the conversation wanders far from the pasted spec. Which is why there's a second layer, and eventually a third.

## Layer 2 — the toolchain you don't get for free

With an established language you inherit an ecosystem: an editor mode, a language server, a formatter, examples, linters, a decade of accreted tooling. With a zero-data language you inherit none of it, and you discover, building it, how much of "a language" is actually the scaffolding around the language rather than the language itself.

You build all of it. And the part that surprised me is how much of that scaffolding should be *generated from a single source of truth*, so that it physically cannot drift out of sync with the real language:

- **The editor syntax highlighting is generated from the lexer's keyword table.** There's a test that fails the build if the generated grammar and the lexer ever disagree. The highlighter cannot describe a keyword the language doesn't have, or miss one it does, because both come from the same list — change the keywords and the grammar regenerates or the build goes red.
- **The language server reuses the compiler's own checker.** The squiggles in the editor are the *same* diagnostics, with the same stable codes (`RS0206` for an unknown call, `RS0026` for an unbound name, `RS0202` for a missing effect), that you get from the command line. The editor can never disagree with the build, because the editor *is* the build, wrapped in a protocol.
- **Everything defers to one authority — the checker.** Formatting, diagnostics, go-to-definition, the package tools, the editor: none of them holds a second opinion about what the language means. There is exactly one implementation of "what is valid RSScript," and every tool is a thin client of it.

The pattern that kept recurring: **one source of truth, generated glue, a freshness test that fails if the glue rots.** It is a small discipline, and it matters more for a zero-data language than a normal one, for a blunt reason: there is no community to notice and file a bug when the docs and the compiler drift apart. There are no users yet. *You* are the entire ecosystem, which means the ecosystem has to be self-checking, because no one else is checking it. This layer is not a design sketch — it's real and built today, and it's most of what "make the language usable" actually consisted of.

## Layer 3 — from persuasion to enforcement

Here's the part I'm most excited about, and I want to be scrupulous about labeling it as a design rather than a shipped feature, because the line between "I built this" and "I think this is right" is exactly the line this series promised not to blur.

The guide raises the odds. The toolchain keeps the language honest. But neither makes an invalid program *impossible to generate* — the model can still, mid-stream, emit a method that doesn't exist, or forget an effect keyword, or drift back toward a Rust idiom after a long conversation. The frontier layer closes that gap by moving the constraint from the prompt into the decoder itself.

A model generates one token at a time, each conditioned on everything before it. At each step, instead of letting it pick freely from its whole vocabulary and *hoping* the guide did its job, you ask the compiler a question: *given the partial, half-written program so far, which next tokens are even legal?* — and you only let the model sample from those.

The effect is that the two reflexes Layer 1 spends its opening paragraphs trying to teach stop being possible at all:

```text
let path = Path.|              <- cursor here
   the only legal continuations are Path's actual methods, read straight
   out of the checker's view of the type:  from_string, exists, ...
   `Path.nonexistent` is not in the set. The model cannot select it.

Path.from_string(|             <- one token later
   the only legal next token is the parameter label `value`, and it must
   be followed by the effect keyword `read` the signature requires.
   The model cannot forget the `read`. It is not an option on the table.
```

Named arguments and effect keywords — the exact things a model with a foreign prior gets wrong — become things it is *structurally incapable of getting wrong*. The compiler stops being a thing that judges the output after the fact and becomes a thing that shapes the output as it's produced. Persuasion becomes enforcement. And note what makes it possible: this only works because the language is small enough, and its checker fast enough, to ride along inside the decoding loop. A language with Rust's surface could not do this; the constraint solver would be the bottleneck. The smallness from post 3 isn't just a review property — it's what lets the compiler sit in the generator's inner loop.

I have the pieces this needs — the lexer, the parser, the checker — all already built, and all already the single authority from Layer 2. Wiring them into a decoder is designed and drafted, not shipped. I'm flagging it as the direction, not claiming it as done. (If you want evidence the direction is sound rather than my own optimism about my own checker: a much more mature project has already shipped a version of exactly this. That's the next post, and it's the best news I've gotten about this whole bet.)

## The shape of the answer

So the bet from post 3 — "distribute by fitting in context" — turned out to have three layers underneath it, each needing the one before:

```text
persuade    a compiler-checked guide the model reads as ground truth
scaffold    a toolchain generated from one source of truth, self-checking
enforce     constrain the decoder with the compiler, so invalid is impossible
```

A zero-data language isn't a handicap you spend the project working around. It's the condition that makes all three layers clean: no prior to fight in Layer 1, one authority to defer to in Layer 2, and a surface small enough that the compiler can ride inside the decoder in Layer 3. The thing I was most afraid of when I started — *no one has ever seen this language, so how will anyone, human or model, ever write it* — turned out to be the project's biggest structural advantage, the same way post 2's curse on Rust turned out to be a clean slate here. You just have to build the three layers that turn the empty prior into an asset instead of leaving it as a void.

---

*Next: [The agent that writes the language](07-the-agent-that-writes-the-language.md) — the first real program in RSScript is, fittingly, an AI agent that writes RSScript, and the most important thing it produced wasn't the agent. It was a bug the compiler couldn't see and review could.*
