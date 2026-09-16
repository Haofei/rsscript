# ADR 0005: Bindgen emits canonical language signature spelling

## Status

Accepted

## Decision

Generated Provider descriptors use canonical RSScript type spelling inside the
legacy `FunctionSignature` text envelope (for example `String` and
`Option<Int>`). Rust `WireType` constructor expressions are retained only where
generated Rust source needs to construct a `WireType` value; they are never ABI
text.

## Compatibility and evidence

This restores signature-hash agreement between Artifact imports and generated
official Providers. Bindgen regression coverage asserts the generated spelling,
and the embedded report pipeline links its fs Provider through the public SDK.
