# I reviewed 100,000 lines of AI-generated Rust. Here's what hurts.

Over the past six months I have reviewed more than a hundred thousand lines of Rust written by AI. Different models, different prompts, different projects — the patterns repeat. The code compiles. The tests pass. The PRs still take much longer to review than they should.

This post is about what specifically hurts. Not about AI being wrong, not about AI being bad at Rust — most of the code I reviewed was, in some narrow technical sense, correct. The problem is reviewability. Rust signatures and idioms that a human can author at a slow steady pace become a wall of noise when a model authors them at the rate of a thousand lines per minute. The compiler accepts them. My eyes don't.

Below are the patterns I saw over and over. None of them are exotic. Most of them appear in the first 200 lines of any non-trivial AI-generated module.

## 1. Concurrency primitives stacked four deep

The most common shape:

```rust
pub fn cache() -> Arc<RwLock<HashMap<String, Arc<UserProfile>>>> {
    Arc::new(RwLock::new(HashMap::new()))
}
```

Then later:

```rust
fn put(
    cache: Arc<RwLock<HashMap<String, Arc<UserProfile>>>>,
    key: String,
    value: Arc<UserProfile>,
) -> Option<Arc<UserProfile>> {
    let mut guard = cache.write().unwrap();
    guard.insert(key, value)
}
```

A reviewer asks one question reading this: *does this store the value, and is the cache shared safely?* That's two bits. The signature carries roughly two dozen bits to give you those two — `Arc` twice, `RwLock` once, `HashMap`, `String`, `Arc` again, `Option`, `Arc` again. You spend the next minute mentally unwrapping the type to confirm what should be confirmable in three seconds.

Senior Rust engineers don't write code this way. They use a type alias, they wrap the cache in a struct, they hide the concurrency primitives behind a method. AI, having no internal aesthetic about layering, prints every primitive at every site. Across a 400-line module, the cumulative cost of unwrapping these onions adds up to most of the review time.

## 2. Eight trait bounds where one would do

```rust
pub fn process<T, F, E>(
    items: Vec<T>,
    handler: F,
) -> Result<Vec<T>, E>
where
    T: Clone + Send + Sync + Debug + 'static,
    F: Fn(&T) -> Result<T, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    items.into_iter().map(|item| handler(&item)).collect()
}
```

This function is called from exactly one place in the codebase. That call site uses concrete types and never touches threads. Every bound in the `where` clause exists not because the implementation needs it, but because the model defensively guessed they might be needed. Once they're there, removing them is a multi-hour exercise in figuring out which ones are actually load-bearing — so the reviewer leaves them, the next AI-generated function inherits the same pattern, and the codebase slowly accumulates trait bounds the way attics accumulate boxes.

The pattern compiles. The pattern works. The pattern obscures the fact that this is internal code with one caller.

## 3. `Pin<Box<dyn Future>>` blocking the view

Async signatures are where the noise gets out of hand:

```rust
pub fn fetch_user<'a>(
    db: &'a Database,
    id: UserId,
) -> Pin<Box<dyn Future<Output = Result<User, Box<dyn std::error::Error + Send + Sync + 'static>>> + Send + 'a>> {
    Box::pin(async move { db.find(id).await })
}
```

A reviewer reads that and wants to know: *fetch a user, return it or an error.* Two bits. The signature gives you those two bits behind a thick wall of `Pin`, `Box`, `dyn`, `Future`, `Output`, `Box<dyn Error + Send + Sync + 'static>`, `Send`, `'a`. Each piece is technically meaningful, but stacked together they form a barrier that you have to *parse and discard* before you can think about whether the function does the right thing.

The honest version is `async fn fetch_user(...) -> Result<User, ServiceError>`. The model knows about `async fn`. It still writes the desugared form anyway, because the desugared form is what shows up most often in the libraries it learned from.

## 4. Retention buried three call levels down

Here's a function that looks innocent:

```rust
pub fn build_index(rules: &RuleSet, store: &mut Store) -> Result<Index, BuildError> {
    let mut index = Index::new();
    for rule in rules.iter() {
        register_rule(&mut index, rule, store)?;
    }
    Ok(index)
}
```

Nothing in the signature tells you that the `Rule` references end up cloned into the index, that the store gets a callback registered against it, or that the store's callback closes over a reference to the index. To find out, you have to follow `register_rule` into its implementation, then follow whatever it calls in turn. By the time you've finished, you've read most of the module just to answer *"what does this function retain?"*

This is the worst review cost in Rust, and AI generates it constantly. Retention is invisible at the signature level. The language has no way to put it there. AI has no incentive to put it in a doc comment. So every reviewer who wants to know what survives the call has to do the work themselves, every time.

## 5. The "kitchen sink" error type

```rust
pub fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    ...
}
```

Convenient to write. Almost useless to review. *What can actually go wrong here?* Could be IO, parse, validation, network, anything. The error type is a wildcard, so the reviewer can't reason about failure modes from the signature.

The well-engineered alternative is a sealed `enum Error { ... }` with concrete variants. AI knows this exists. It writes the boxed-dyn version anyway because that's what beginner Rust tutorials use, and beginner tutorials are over-represented in the training data.

## 6. Everything is `pub`

A 400-line generated module where every function, every struct, every field carries `pub`. The model doesn't know which symbols are internal helpers and which are the module's surface, so it assumes everything is surface. The reviewer now has no fast way to tell *what's the public contract here* — they have to read every function and decide for themselves.

A 20-line struct with three `pub` methods and twelve private helpers reviews in two minutes. The same 20 lines with everything `pub` reviews in fifteen, because every helper now potentially has external callers and you have to verify it's safe at the module boundary, not just internally.

## 7. `impl Into<T>` parameters that no caller needs

```rust
pub fn set_name(user: &mut User, name: impl Into<String>) {
    user.name = name.into();
}
```

The function is called from three places, all of which pass `String` directly. The `impl Into<String>` flexibility is unused, but it makes the signature harder to read and forces the reviewer to think about which conversions are legal here. AI defaults to maximum API flexibility because library code does that. Application code shouldn't, but AI doesn't know it's writing application code.

## 8. Deriving the entire universe

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff_ms: u64,
}
```

Ten derives on a struct that is constructed in exactly one place, compared nowhere, used as a hash key never, and serialized only because someone imagined a future where it might be. Each derive is a small promise — *this type is totally ordered, this type is a valid map key, this type round-trips through JSON* — and a careful reviewer has to register every promise and ask whether it's load-bearing. Here, none of them are. The struct wanted `Clone`, maybe `Debug`. The other eight are there because library types derive the full set, and library types derive the full set because a library genuinely can't know which capabilities its callers will reach for. Application code does know. It just doesn't get asked, because the model is writing in library register ([the subject of the next post](02-ai-writes-library-code.md)) and library register derives everything by reflex.

## 9. The 400-line module with no review map

Worst of all, the cumulative effect: an AI-generated module is a wall of locally-correct code with no signal about where the risk lives. Twenty functions, all roughly the same shape, all roughly equally formatted. The reviewer can't tell which three are load-bearing and which seventeen are mechanical helpers. So they read all twenty.

This is the actual review cost. Not any one of the patterns above. The compounding effect — that every signature is slightly too noisy, every function is slightly over-engineered, every error type is slightly too vague, every module is slightly too flat — adds up to PRs that take three to five times longer to review than the equivalent hand-written code, even when the AI-generated code is technically equivalent.

## The compiler was always happy

I want to be precise about what this post is and isn't claiming. None of the code I reviewed was *wrong* in any way the Rust compiler would catch. None of it had soundness holes. Most of it shipped without bugs. The problem isn't correctness. The problem is that **review is a finite human resource and AI generation rate is effectively infinite**, and the gap is widening.

The ratio that used to define software engineering — writing code is expensive, reviewing it is manageable — has flipped. Generating is cheap. Reviewing is the bottleneck. Every team I know that has adopted AI coding tools heavily is hitting the same wall: the code arrives faster than humans can sanity-check it, and the only honest choices are *review less of it* (scary) or *make each line cheaper to review* (which is the interesting problem).

That's the problem I started thinking about after the first 30k lines. By 100k I had a sketch of what a smaller, review-first language might look like; eventually I started building it. It's called RSScript, it lowers to Rust, and it's the subject of the next few posts in this series. But you don't need to care about RSScript to find the problem above real. The problem is real whether or not anyone solves it. I just got tired of waiting and started solving it for myself.

---

*Next: [Why AI writes Rust like library code](02-ai-writes-library-code.md) — a diagnosis of why the patterns above keep showing up even when the model "knows better".*
