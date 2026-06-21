# Convenience targets for the RSScript dev container.
#
# Run everything inside the Docker dev environment (see docs/DOCKER.md):
#   docker compose run --rm dev make <target>
#
# These are thin wrappers around the cargo invocations documented in
# docs/DEVELOPMENT.md; they exist so CI and contributors invoke the *same*
# command.

.PHONY: test-compile test-fast test-full test-soak miri fuzz-no-panic test-native-jit-unit

# Public test taxonomy. These targets are intentionally the only normal test
# entry points; Cargo internals use the same four names: static, runtime,
# differential, and soak.
test-compile:
	docker compose run --rm dev cargo test -p rsscript --no-run

test-fast:
	docker compose run --rm dev cargo test -p rsscript

test-full:
	docker compose run --rm dev cargo clippy -p rsscript --tests -- -D warnings
	docker compose run --rm dev cargo test -p rsscript --features native-jit --no-run
	docker compose run --rm dev cargo test -p rsscript
	git diff --check

test-soak:
	docker compose run --rm dev bash -lc 'RSSCRIPT_FULL_BACKEND_PARITY=1 RSS_DIFF_PROPTEST_CASES=200 RSS_GENERATIVE_CASES=64 RSS_GENERATIVE_MUTATION_CASES=200 cargo test -p rsscript --test differential'
	docker compose run --rm dev cargo test -p rsscript --test soak -- --ignored

# Tier C (runtime hardening): run Miri over the largest pure-Rust subset it can
# soundly interpret — the `rss-testgen` seed decoder (pure arithmetic / control
# flow, no I/O, no FFI). Miri double-checks that subset for undefined behaviour
# (out-of-bounds, use-after-free, invalid-value, data races).
#
# What Miri canNOT cover here, by construction:
#   * the vm-jit tier — it executes generated *native machine code*, which Miri
#     (a MIR interpreter) cannot run at all;
#   * the FFI / syscall seams (native plugin cdylibs, process/network/filesystem
#     in `rsscript-runtime` and `reir`) — Miri's isolation blocks real syscalls,
#     so those test binaries abort under Miri rather than reporting UB.
# Invariant 2 (`panic = "abort"`) is what keeps those out-of-Miri-scope seams
# safe in production; Miri's job is the pure value/logic core.
#
# Requires a nightly toolchain with the `miri` component:
#   rustup toolchain install nightly --component miri,rust-src
miri:
	cargo +nightly miri test -p rss-testgen --lib

# Smoke-run the Invariant-1 no-panic fuzz target (requires `cargo install
# cargo-fuzz` and a nightly toolchain). Bounded so it is CI-friendly.
fuzz-no-panic:
	cd fuzz && cargo +nightly fuzz run no_panic -- -runs=20000 -max_total_time=60

# Fast path for native-JIT unit tests. Run through the Docker dev environment so
# macOS hosts do not build/link native Rust artifacts in the bind-mounted tree.
test-native-jit-unit:
	docker compose run --rm dev cargo test -p rsscript --lib register_window_tests --features native-jit
