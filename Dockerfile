# Cross-platform development image for RSScript.
#
# The workspace is edition 2024, which requires Rust >= 1.85; `rust:1-bookworm`
# tracks the latest stable 1.x and always satisfies that. Pin to a concrete tag
# (e.g. `rust:1.88-bookworm`) if you want a fully reproducible toolchain.
#
# The source tree is NOT copied into the image — it is bind-mounted at runtime
# (see compose.yaml) so edits on the host are seen instantly on every platform.
# This image only provides the toolchain and system libraries.
FROM rust:1-bookworm

# System libraries needed to build the workspace and the Rust packages that
# RSScript lowers to. Everything here uses rustls/ring (no OpenSSL) and a
# source-bundled SQLite, so the list is just a C/C++ toolchain plus the usual
# fetch/build helpers:
#   build-essential, cmake -> `ring` (rustls) and `rusqlite` (bundled SQLite)
#   pkg-config             -> -sys crate probing
#   git, curl, ca-certificates -> fetching crates and tools over HTTPS
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        cmake \
        git \
        curl \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Toolchain components used by the test/lint flow, and cargo-nextest (the test
# runner the `rss test` command and CI use). Prefer the prebuilt nextest binary
# per architecture (works on amd64 and Apple-Silicon/arm64); fall back to a
# source build on other arches.
RUN rustup component add clippy rustfmt \
    && set -eux; \
    case "$(uname -m)" in \
        x86_64)  url="https://get.nexte.st/latest/linux" ;; \
        aarch64) url="https://get.nexte.st/latest/linux-arm" ;; \
        *)       url="" ;; \
    esac; \
    if [ -n "$url" ]; then \
        curl -LsSf "$url" | tar zxf - -C "${CARGO_HOME:-/usr/local/cargo}/bin"; \
    else \
        cargo install cargo-nextest --locked; \
    fi

# The base image puts the toolchain on PATH via the container environment, but a
# login shell (`bash -l`, some devcontainer setups) re-derives PATH from
# /etc/profile and would drop it. Add a profile snippet so cargo/rustc/nextest
# are on PATH for every shell flavor.
RUN printf 'export PATH="%s/bin:$PATH"\n' "${CARGO_HOME:-/usr/local/cargo}" \
        > /etc/profile.d/rust-cargo.sh

WORKDIR /work

# `rss` (and the test suite) compiles generated Rust packages at runtime, so the
# container keeps the full toolchain available. Default to an interactive shell;
# compose overrides the command for one-off runs.
CMD ["bash"]
