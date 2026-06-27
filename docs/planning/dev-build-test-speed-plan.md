# Developer build/test speed plan

Make RSScript development feel closer to Go's edit-test loop while preserving the
full Rust parity/soak gates for slices that need them.

Status: proposed. Created 2026-06-20.

## 0. Goal

The default developer path should be:

```bash
rssdev check
rssdev test
rssdev test jit
rssdev soak
```

Where:
- `check` is the fastest correctness pass for ordinary compiler/VM edits;
- `test` runs only the small local test set by default;
- `test jit` opts into Cranelift/native-JIT dependencies explicitly;
- `soak` is slow, Docker/parity-heavy, and used only as a slice-exit gate.

The user-facing rule: **fast commands by default, expensive commands by name**.

## 1. Current pain points

- `cargo test -p rsscript some_name --features native-jit` still builds/runs every
  `rsscript` test binary before filtering. Use `--lib` or `--test name` instead.
- `native-jit` pulls Cranelift. That is correct, but it must never be in the default
  edit loop.
- The workspace default includes many crates. Bare `cargo test` is too broad for
  day-to-day work.
- `rsscript-runtime` pulls network/GPU/async-heavy dependencies (`reqwest`, Tokio,
  websocket, Metal, ICU/url stack transitively). VM/JIT/compiler tests should not need
  all of that for the common path.
- Normal tests should be documented as Docker Compose commands directly. That
  keeps parity explicit and avoids a required wrapper layer.

## 2. Slice A — command UX first

Add a tiny developer command wrapper, either `scripts/rssdev` or an `xtask` crate.
Prefer a script first; it is faster to land and easier to change.

Commands:

```bash
rssdev check
rssdev test
rssdev test lib
rssdev test runtime <filter>
rssdev test differential <filter>
rssdev test jit <filter>
rssdev test vm-jit <filter>
rssdev soak
```

Implement these mappings:

```bash
rssdev check
  cargo check -p rsscript

rssdev test
  cargo test -p rsscript --lib

rssdev test runtime <filter>
  cargo test -p rsscript --test runtime <filter>

rssdev test differential <filter>
  cargo test -p rsscript --test differential <filter>

rssdev test jit <filter>
  cargo test -p rsscript --test runtime jit_acceptance --features native-jit
  cargo test -p rsscript --test differential <filter> --features native-jit

rssdev test vm-jit <filter>
  cargo test -p vm-jit <filter>

rssdev soak
  docker compose run --rm dev bash -lc \
    'RSSCRIPT_FULL_BACKEND_PARITY=1 RSS_DIFF_PROPTEST_CASES=200 RSS_GENERATIVE_CASES=64 RSS_GENERATIVE_MUTATION_CASES=200 cargo test -p rsscript --test differential'
  docker compose run --rm dev cargo test -p rsscript --test soak -- --ignored
```

Acceptance:
- ordinary contributors can run a useful local gate without knowing Cargo's test
  target syntax;
- `native-jit` is visible in the command name;
- Docker is visible in the command name or docs, never hidden in the fast path.

## 3. Slice B — local cache and linker setup

Document a one-time local setup:

```bash
brew install sccache
export RUSTC_WRAPPER=sccache
```

Optional later:
- evaluate a faster macOS linker (`zld`/`lld`) on this repo with a same-machine
  before/after;
- do not commit linker flags until verified on CI and all developer platforms.

Acceptance:
- cold build unchanged or better;
- branch/worktree rebuilds improve materially;
- no required global setup for CI.

## 4. Slice C — Docker-first command split

Document explicit Docker Compose commands by cost:

```sh
docker compose run --rm dev cargo test -p rsscript
docker compose run --rm dev cargo test -p rsscript --features native-jit --no-run
docker compose run --rm dev cargo test -p rsscript --test soak -- --ignored
```

Keep optional local shortcuts if scripts depend on them, but document Docker
Compose as the daily command.

Acceptance:
- normal Docker Compose test command completes;
- native-JIT compile/test command is explicit;
- soak remains opt-in.

## 5. Slice D — dependency diet

The largest structural win is to stop compiling heavy runtime dependencies for VM/JIT
tests that do not need them.

Target split:

```text
rsscript-runtime-core
  scalar/list/map/string/json helpers, tensor-free, net-free, gpu-free

rsscript-runtime-net
  reqwest, tokio-tungstenite, network/process async helpers

rsscript-runtime-gpu
  Metal compute

rsscript-runtime-full
  feature aggregator used by release/full parity
```

Shorter-term alternative: feature-gate `rsscript-runtime`:

```toml
[features]
default = ["core"]
net = ["dep:reqwest", "dep:tokio", "dep:tokio-tungstenite"]
gpu = ["dep:rss-metal-compute"]
full = ["core", "net", "gpu"]
```

Then make `rsscript` depend on the light feature set for normal tests and only enable
`full` for backend parity/soak.

Acceptance:
- `cargo check -p rsscript` no longer compiles network/websocket/GPU crates unless a
  test asks for them;
- file/process/network integration tests explicitly opt into the heavier runtime;
- full soak still uses the full runtime feature set.

## 6. Slice E — test target hygiene

Keep `autotests = false` and the four explicit `rsscript` integration binaries. Add
more only when there is a real isolation payoff.

Rules:
- unit tests for VM/value/JIT internals go under `--lib`;
- backend differential cases stay in `--test differential`;
- slow demos stay ignored in `--test soak`;
- native-JIT tests that only touch `vm-jit` stay in `cargo test -p vm-jit`;
- avoid test commands that filter after building every binary.

Acceptance:
- docs show exact target-specific commands for common failures;
- no new unignored soak/runtime-heavy tests enter the default local path.

## 7. Slice F — measured gates

Track three numbers before and after each slice:

```bash
time cargo check -p rsscript
time cargo test -p rsscript --lib
time cargo test -p rsscript --test runtime jit_acceptance
time cargo test -p rsscript --test runtime jit_acceptance --features native-jit
```

Optional cold-cache check:

```bash
cargo clean
time cargo check -p rsscript
```

Exit gate:
- each slice must improve at least one common command or materially improve command
  usability;
- no slice may weaken the soak/parity discipline.

## 8. Recommended implementation order

1. Add `scripts/rssdev` with target-specific commands.
2. Update development docs to expose Docker Compose commands directly.
3. Update `docs/development/DEVELOPMENT.md` to teach the fast loop first.
4. Add `sccache` setup docs.
5. Measure the current baseline commands.
6. Feature-gate or split heavy runtime dependencies.
7. Re-measure and keep only changes that reduce common command time.

## 9. Enforcement — keep it fast

This must be enforced like a performance invariant, not left as advice.

### 9.1 Command ownership

Only these commands are considered the official developer gates:

```bash
rssdev check
rssdev test
rssdev test jit
rssdev soak
```

Docs, PR templates, and contributor guidance should point to these commands, not raw
`cargo test` incantations. Raw Cargo commands are allowed for local debugging, but
they are not the documented path.

### 9.2 Time budgets

Establish machine-specific baseline numbers after Slice A lands, then track deltas.
Initial target budgets:

```text
rssdev check       <= 10s warm, <= 60s cold
rssdev test        <= 20s warm
rssdev test jit    <= 60s warm after first native-jit build
rssdev soak        no short budget; explicit slow gate
```

The exact numbers can be adjusted after measuring the repo on the reference machine,
but every budget must have an owner and a command.

### 9.3 CI guard

Add a lightweight CI job named `dev-speed-smoke`:

```bash
time rssdev check
time rssdev test
```

The job should not fail on raw wall-clock time at first because CI machines are noisy.
Instead, it should always print the timing summary and fail only on structural
violations:

- `rssdev test` must not enable `native-jit`;
- `rssdev test` must not invoke Docker;
- `rssdev test` must not run `--test differential` or `--test soak`;
- `rssdev check` must not build `vm-jit`, network, websocket, GPU, or DB crates unless
  a feature explicitly asks for them.

Once the baseline is stable, add a warning threshold:

```text
warn when p50 over 7 days regresses by >20%
block only after a human confirms the regression is real
```

### 9.4 Dependency creep rule

Any new dependency added to one of these crates requires a build-speed note in the PR:

```text
crates/rsscript
crates/runtime
crates/rss-testgen
crates/vm-jit
```

The note must answer:

```text
Does this dependency enter `rssdev check`?
Does this dependency enter `rssdev test`?
Can it be behind a feature?
Is it only needed by soak/integration/native adapters?
```

Default answer should be: heavy dependencies live behind features or in adapter crates.

### 9.5 Test placement rule

Every new test must declare its intended gate:

```text
unit/lib          -> rssdev test
runtime focused  -> rssdev test runtime <filter>
differential     -> rssdev test differential <filter>
native JIT       -> rssdev test jit <filter> or rssdev test vm-jit <filter>
soak/demo/slow   -> rssdev soak, ignored by default
```

Reject changes that add slow, native-JIT, network, GPU, process, or generative tests to
the default `rssdev test` path.

### 9.6 Feature hygiene rule

Feature names must reveal cost:

```text
native-jit
runtime-net
runtime-gpu
runtime-db
full-runtime
soak
```

Do not hide expensive features behind `default`. If a feature pulls Cranelift,
network/websocket, Metal/GPU, database clients, or large generated code, it must be
opt-in and named as expensive in docs.

### 9.7 Review checklist

Add this checklist to PR/review templates:

```text
- [ ] Fast path still uses `rssdev check` / `rssdev test`.
- [ ] No new dependency enters the default path without justification.
- [ ] New tests are placed in the right gate.
- [ ] Native-JIT changes ran `rssdev test jit`.
- [ ] Performance/JIT/value-rep slices ran `rssdev soak`.
- [ ] Timing summary included if build/test surface changed.
```

### 9.8 Regression response

When the fast path slows down:

1. Run the four measured commands from §7.
2. Identify whether the regression is dependency compile, crate rebuild, link time, or
   test runtime.
3. If dependency compile: feature-gate, move to adapter crate, or remove.
4. If crate rebuild: split module/crate boundaries or reduce build-script invalidation.
5. If link time: narrow test target or evaluate linker/cache changes.
6. If test runtime: move the test to the correct gate or reduce fixture size.
7. Add a regression note to this plan or the relevant PR.

## 10. Non-goals

- Do not weaken the full generative soak.
- Do not hide Docker behind a required wrapper layer.
- Do not enable `native-jit` by default.
- Do not commit platform-specific linker flags without a measured cross-platform path.
- Do not chase release-build speed before the edit/test loop is fixed.
