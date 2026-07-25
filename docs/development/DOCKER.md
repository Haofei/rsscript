# Containerized development

RSScript builds and runs identically on macOS, Windows, and Linux through a
single Docker toolchain image. The container carries the Rust toolchain, the
test runner, and the system libraries the workspace needs; your checkout is
bind-mounted in, so edits on the host take effect immediately.

This is the recommended setup for contributors who do not want to install the
Rust toolchain and C build dependencies locally, and for keeping builds
reproducible across platforms.

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) with Compose v2 (`docker
  compose`). Docker Desktop (macOS/Windows) or Docker Engine (Linux) both work.
- That's all — no local Rust, C compiler, or SQLite needed.

## Quick start

```sh
# Build the dev image (first run downloads the toolchain; later runs are cached).
docker compose build

# Normal edit loop: runs the focused RSScript library suite.
docker compose run --rm dev cargo run --quiet -p rsscript --bin rss -- test

# Full workspace gate: lint, generated packages, every test target, and examples.
docker compose run --rm dev cargo run --quiet -p rsscript --bin rss -- test --all

# Pre-commit compile gate with the native-JIT feature set.
docker compose run --rm dev cargo test -p rsscript --features native-jit --no-run

# Slow release/demo parity checks.
docker compose run --rm dev cargo test -p rsscript --test soak -- --ignored

# Open an interactive shell in the toolchain.
docker compose run --rm dev bash
```

Inside the shell (or via `docker compose run --rm dev <cmd>`) every normal
workflow is available:

```sh
cargo run --quiet -p rsscript --bin rss -- test         # focused edit loop
cargo run --quiet -p rsscript --bin rss -- test --all   # exhaustive workspace gate
cargo test -p rsscript --no-run            # compile rsscript tests only
cargo test -p rsscript --features native-jit --no-run
cargo clippy --all-targets                 # lints
cargo fmt --all                            # format
cargo run -p rsscript --bin rss -- <args>              # drive the rss CLI
```

## Test feedback budgets

Use `rss test` for the normal edit loop and reserve `rss test --all` for a
before-push or CI-equivalent gate. The default profile deliberately runs only
formatting, the `rsscript` library tests, and the whitespace check; it should
remain a short feedback command. The full profile includes clippy, generated
Rust package compilation, the workspace nextest suite, self-host parity, and
example/package checks.

`rss test --json` emits a `duration_ms` field for every manifest item. When a
full run slows down, use that output to identify the slow item before changing
parallelism or profiles. The full suite keeps compiler/adapter tests separate
from the complete runtime's default net/GPU feature set; combining every
workspace member in one Cargo invocation would unify those features into the
compiler's core-only runtime dependency and build a much larger test artifact.
Generated-Rust tests remain serialized because nested Cargo builds otherwise
contend on one shared target directory and make the entire gate slower.

The Docker target and Cargo registry volumes are part of the performance
contract. Do not reset them for normal measurements; use two warm runs and
compare the second. Database adapters are already separate workspace packages,
so there is no runtime `db` feature to compile on ordinary compiler tests.

## How it is wired

- **Image** (`Dockerfile`): `rust:1-bookworm` plus `build-essential`, `cmake`,
  and `pkg-config` (for `ring`/rustls and the bundled-SQLite `rusqlite`),
  `clippy`/`rustfmt`, and `cargo-nextest`. The workspace uses rustls/ring
  throughout, so no OpenSSL is required.
- **Source** is bind-mounted at `/work` (see `compose.yaml`) — not copied into
  the image — so host edits are picked up with no rebuild.
- **Caches** live in named volumes, deliberately kept off the host bind mount so
  compilation persists between runs and stays fast on macOS/Windows:
  - `target` → `/work/target`
  - `cargo-registry` → `/usr/local/cargo/registry`
  - `cargo-git` → `/usr/local/cargo/git`

  Reset a cache with `docker compose down -v` (removes all three volumes).

## Reproducible toolchain

The image tracks the latest stable Rust 1.x (`rust:1-bookworm`), which always
satisfies the workspace's edition-2024 requirement (Rust ≥ 1.85). For a fully
pinned toolchain, change the base tag in `Dockerfile` to a concrete version such
as `rust:1.88-bookworm` and rebuild.

## VS Code / Codespaces

`.devcontainer/devcontainer.json` reuses the same compose service. In VS Code,
run **Dev Containers: Reopen in Container**; on GitHub, **Create codespace**.
You get rust-analyzer (clippy-backed), TOML, and LLDB wired to the identical
toolchain and caches.

## Apple Silicon and other architectures

The image builds natively on both `amd64` and `arm64` (Apple Silicon); the
nextest install picks the matching prebuilt binary and falls back to a source
build elsewhere. No extra configuration is needed.
