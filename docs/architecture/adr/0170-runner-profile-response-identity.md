# ADR 0170: Bind runner responses to the selected profile identity

- Status: Accepted
- Date: 2026-08-14

## Problem

The runner request selected a fixed, host-installed profile, but a completed
response carried no evidence of that selection. As more fail-closed profiles
are added, a parent must not infer the child profile from a child-controlled
report or leave profile selection as invisible deployment state.

## Decision and non-goals

`rsscript.runner_response.v1` now includes a non-secret
`RunnerProfileIdentityV1`: profile ID, profile version, and descriptor digest.
Every response, including pre-execution rejection, carries that identity. The
parent validates it against the profile it requested before it accepts a child
response.

The profile identity is audit evidence only. It neither transfers Provider
implementations nor grants authority. This change does not add remote profile
selection, dynamic libraries, credentials, filesystem roots, endpoints, or a
policy language to the protocol.

## Compatibility and migration

The protocol schema remains v1 during this pre-release migration, so all
first-party sender/reader pairs are updated together. A response without the
identity is rejected by the checked JSON schema and protocol decoder. Artifact,
language, Provider ABI, and SDK contracts are unaffected.

## Verifier and security impact

Profile matching happens after response framing/state-machine validation and
before the CLI treats a frame as an execution outcome. A malicious or confused
child cannot substitute a response that claims a different profile. The digest
does not authenticate the child; hosts requiring origin authentication still
need their own admission/provenance policy.

## Provider and backend impact

No Provider invocation changes. The sole reference profile remains
`no_providers`; future profiles must supply a stable identity before they can
be selected by a runner host.

## Evidence

The runner-protocol suite round-trips profile identity through bounded frames,
requires a matching parent-selected profile, and rejects a forged digest. CLI
runner tests cover protocol framing, while the Core architecture test requires
the default isolated path to call `validate_response_profile`.
