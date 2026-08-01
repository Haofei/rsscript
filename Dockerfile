# Cross-platform development image for RSScript.
#
# The workspace is edition 2024. Keep the image aligned with CI and pin the
# multi-architecture manifest so an upstream tag move cannot change builds.
#
# The source tree is NOT copied into the image — it is bind-mounted at runtime
# (see compose.yaml) so edits on the host are seen instantly on every platform.
# This image only provides the toolchain and system libraries.
FROM rust:1.96.1-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663

# System libraries needed to build the workspace and Rust packages emitted by
# RSScript, plus the usual fetch/build helpers:
#   build-essential, cmake -> native Rust dependencies such as `ring`
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

# Toolchain components used by the test/lint flow, and cargo-nextest used by
# selected CI validation. Prefer the prebuilt nextest binary per architecture
# (works on amd64 and Apple-Silicon/arm64); fall back to a source build on
# other arches.
ARG CARGO_NEXTEST_VERSION=0.9.140
ARG CARGO_NEXTEST_X86_64_SHA256=4ee9aaa0d0171a985a5d0eb735b87355894c1c455972e9674fb9fdbd1387c9a3
ARG CARGO_NEXTEST_AARCH64_SHA256=8b3f4d4560b6b0f83774fecc6be07e47716dbad0eb0bb6c3890f478f4affe4b6

RUN rustup component add clippy rustfmt \
    && set -eux; \
    case "$(uname -m)" in \
        x86_64)  url="https://get.nexte.st/${CARGO_NEXTEST_VERSION}/linux"; expected="${CARGO_NEXTEST_X86_64_SHA256}" ;; \
        aarch64) url="https://get.nexte.st/${CARGO_NEXTEST_VERSION}/linux-arm"; expected="${CARGO_NEXTEST_AARCH64_SHA256}" ;; \
        *)       url=""; expected="" ;; \
    esac; \
    if [ -n "$url" ]; then \
        curl -LsSf "$url" -o /tmp/cargo-nextest.tar.gz; \
        echo "${expected}  /tmp/cargo-nextest.tar.gz" | sha256sum -c -; \
        tar zxf /tmp/cargo-nextest.tar.gz -C "${CARGO_HOME:-/usr/local/cargo}/bin"; \
    else \
        cargo install cargo-nextest --version "${CARGO_NEXTEST_VERSION}" --locked; \
    fi

# The base image puts the toolchain on PATH via the container environment, but a
# login shell (`bash -l`, some devcontainer setups) re-derives PATH from
# /etc/profile and would drop it. Add a profile snippet so cargo/rustc/nextest
# are on PATH for every shell flavor.
RUN printf 'export PATH="%s/bin:$PATH"\n' "${CARGO_HOME:-/usr/local/cargo}" \
        > /etc/profile.d/rust-cargo.sh

ARG RSSCRIPT_UID=1000
ARG RSSCRIPT_GID=1000
RUN if ! getent group "${RSSCRIPT_GID}" >/dev/null; then \
        groupadd --gid "${RSSCRIPT_GID}" rsscript; \
    fi \
    && useradd \
        --create-home \
        --uid "${RSSCRIPT_UID}" \
        --gid "${RSSCRIPT_GID}" \
        --shell /bin/bash \
        rsscript \
    && mkdir -p /work \
    && chown -R "${RSSCRIPT_UID}:${RSSCRIPT_GID}" /work "${CARGO_HOME:-/usr/local/cargo}"

WORKDIR /work

# `rss` (and the test suite) compiles generated Rust packages at runtime, so the
# container keeps the full toolchain available. Default to an interactive shell;
# compose overrides the command for one-off runs.
USER rsscript
CMD ["bash"]
