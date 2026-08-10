# ADR 0002: Numeric bytecode v2 executable model

## Status

Accepted

## Problem

The v1 executable payload is a canonical CBOR encoding of a JSON-shaped object
tree. Although its verifier is bounded, opcode names and field maps remain
string-driven and can drift from the VM decoder.

## Decision and non-goals

`rsscript-bytecode::v2` owns an independent typed model with numeric table IDs,
numeric opcode tags, exact operand arities, and structural index validation.
Functions, constants, imports, registers, and instruction targets are all
numeric identities. The model contains resource and structured-async opcode
slots so those contracts do not require a future source-language string opcode.

The initial codec uses canonical CBOR arrays shaped as `[numeric_opcode,
operands]`; decoding re-encodes for canonical-byte equality, rejects unknown
opcodes, invokes the typed structural verifier, and returns a private-field
`VerifiedProgramV2` through `BytecodeV2Verifier`. This decision does not enable
a v2 Artifact writer, replace the v1 verifier, or freeze every future opcode.
Export/debug table design and data-flow/type verification remain separate
follow-up decisions.

The numeric tag, operand identity classes, validation arity, decoder lookup,
and generated opcode reference all derive from one `INSTRUCTION_SCHEMA_V2`
table. New v2 instructions must be added there rather than duplicating maps in
the codec, verifier, or documentation.

V2 also separates numeric Artifact-import links, exports, and optional debug
locations from the function/code table. The verifier checks every export and
debug function/instruction reference before returning a verified program.

The v2 decoder has a bounded arbitrary-byte property corpus so malformed
payloads cannot panic the verifier while explicit long-lived seed fixtures are
added with future Artifact v2 sections.

## Compatibility and migration

v1 remains the only deployed writer and reader. A future v2 container/ISA
version will serialize the typed model through a bounded codec while retaining a
read-only v1 compatibility path. No current Provider, SDK, or source-language
contract changes.

## Verifier and security impact

The new model validates finite operand layouts and every table index before a
VM is involved. It is compiler/VM independent, so malformed future v2 input can
be rejected at the Artifact boundary rather than by an execution decoder.

## Provider and backend impact

Provider imports are represented by `WireImportId`; VM, AOT, and JIT backends
will consume a verified typed program rather than reinterpreting textual call
keys. No backend changes land in this initial model.

## Evidence

Focused bytecode tests cover a valid numeric external call and wrong-arity or
out-of-range operand rejection. The existing bounded v1 verifier suite remains
green.
