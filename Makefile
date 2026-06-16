# Convenience targets for the RSScript dev container.
#
# Run everything inside the Docker dev environment (see docs/DOCKER.md):
#   docker compose run --rm dev make <target>
#
# These are thin wrappers around the cargo invocations documented in
# docs/DEVELOPMENT.md; they exist so CI and contributors invoke the *same*
# command.

.PHONY: miri fuzz-no-panic

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
