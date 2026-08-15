# Threat model

RSScript separates language validation, host integration, and process isolation.
These boundaries must not be presented as interchangeable security controls.

## What Core validates

The compiler validates syntax, types, ownership transitions, retention,
resource lifetime, structured asynchronous control flow, and external semantic
signatures. The bytecode loader validates artifact structure before execution.
The runtime enforces configured limits for steps, memory, output, recursion,
deadlines, host calls, and child processes where applicable.

These checks protect language and runtime invariants. They do not make a script,
provider, native plugin, JIT, or generated program trustworthy.

## Trust boundaries

- In-process VM execution is for scripts trusted by the embedding application.
- Providers are trusted host code and may possess all authority of the process.
- Native plugins and generated Rust are trusted code execution mechanisms.
- JIT-generated executable memory is not an isolation boundary.
- Review and REIR report evidence; they do not grant or revoke authority.

## Untrusted and generated input

Machine generation is not a trust signal. An untrusted or externally supplied
script must run in a separately hardened runner, process, container, or stronger
isolation boundary with OS-enforced resource and authority restrictions. The
runner is responsible for choosing providers and limiting their authority.

The reference runner uses a versioned, size-bounded protocol, verifies the
Artifact Bundle again in the child, accepts no dynamic Provider or library path,
and applies process-tree and resource limits. Its opt-in Linux reference
profiles can additionally require `no_new_privs`, user/mount/network namespaces,
or a Landlock ABI-v5 filesystem allowlist rooted at a parent-owned path. A
missing kernel feature or denied control rejects that profile; it never falls
back to the ambient boundary. These profiles remain defense in depth, not a
complete container: deployments that require filesystem, network, identity,
namespace, or syscall isolation must select and validate the OS controls that
match their own Provider authority.

No RSScript API or documentation may describe the in-process runtime as a
sandbox. Vulnerability reports and deployment guidance must identify which
boundary failed: compiler invariant, verifier, runtime limit, provider, native
code, or external isolation.

## Out of scope

Core does not implement language-level permissions, deployment policy,
capability grants, a package trust hierarchy, or a claim that static review can
prove arbitrary host code safe.
