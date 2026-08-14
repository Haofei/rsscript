# ADR 0162: Gate CLI AOT behind an explicit experimental feature

- Status: Accepted
- Date: 2026-08-14

## Problem

The CLI's ordinary `execution` feature enabled compiler Rust/AOT lowering even
though the reference verified VM and isolated runner are the supported product
path. This widened the default CLI build closure and made an experimental
backend appear alongside the normal execution workflow.

## Decision

Keep `rsscript-cli/execution` limited to the SDK, project adapter, process
guard, runner protocol, reference VM, and isolated runner. Add
`rsscript-cli/aot-rust` as an explicit extension that enables
`rsscript-compiler/aot-rust`. Generated-Rust helpers, cache management,
subprocess build logic, and AOT usage text compile only with that feature.

An ordinary CLI build rejects `rss run --aot` with an explicit feature error.
It continues to offer only the isolated runner by default, with the existing
explicit trusted in-process option.

## Non-goals

This does not remove the experimental AOT backend, change its generated Rust
ABI, or create a new `rss-lab` binary. It also does not claim that the runner
is a universal sandbox.

## Compatibility and migration

The change is a Cargo feature-boundary tightening. Hosts that intentionally
use CLI AOT must build with `--features aot-rust`; normal source, Artifact,
Provider, VM, and runner protocol contracts do not change.

## Verifier, security, and backend impact

The verified VM/runner path is unaffected. Removing AOT from ordinary CLI
execution reduces the default dependency and attack surface; it introduces no
new Provider capability or verification rule. Architecture tests assert both
the manifest feature graph and the feature-gated help/error behavior.
