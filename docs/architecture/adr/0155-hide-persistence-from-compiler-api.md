# ADR 0155: Keep artifact persistence out of the compiler API

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler's package compatibility module used the standalone Artifact Store
adapter, but re-exported its lock and atomic-write API. This made filesystem
persistence look like a compiler capability and widened the compiler's public
surface.

## Decision

Compiler package compatibility code may use `rsscript-artifact-store` only
behind its explicit `package` feature, and keeps that dependency private. The
compatibility SDK re-exports the adapter directly for existing callers; the
reviewed compiler and SDK execution paths do not expose persistence.

## Non-goals

This does not move all historical package/review operations yet, or alter the
Artifact Store's locking and atomic-write behavior.

## Compatibility and migration

SDK compatibility names remain available. New compiler consumers must select
an Artifact Store adapter explicitly instead of receiving it through compiler
imports.

## Verifier, security, and backend impact

Artifact persistence remains outside parsing, semantic validation, bytecode
emission, verification, Provider linking, and VM execution. This reduces the
compiler API's ambient filesystem implication without changing Artifact
integrity checks.

## Evidence

Architecture tests enforce the private compiler import, direct compatibility
SDK re-export, and feature-gated dependency. Compiler/package and SDK
compatibility test suites cover the retained behavior.
