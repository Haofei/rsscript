# Threat model

RSScript has three separate boundaries, each with its own job: language
validation proves what a program means, host integration decides what a program
can reach, and process isolation contains what a program does. They are not
interchangeable security controls, and a vulnerability report should say which
one it is about.

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
- Native JIT selection is a trusted-host deployment choice and is unavailable
  to the reference isolated runner. A natively executed region now reports the
  interpreter's exact source-step count and stops for the same reason under an
  armed step budget, cancellation token, or deadline; a region whose step cost
  cannot be attributed exactly declines to the interpreter instead of running
  unmetered. Allocation controls are admitted only with a per-region proof, and
  intrinsic-call and Provider-call budgets remain interpreter-owned, so
  `rss run --native` still selects the explicit unbounded trusted-host limits
  profile until those two budgets are accounted natively. See
  `docs/spec/native-jit-contract.md` for the parity status table.
- Review and REIR report evidence; authority stays with the host.

## Untrusted and generated input

Machine generation is not a trust signal. An untrusted or externally supplied
script must run in a separately hardened runner, process, container, or stronger
isolation boundary with OS-enforced resource and authority restrictions. The
runner is responsible for choosing providers and limiting their authority.

The reference runner uses a versioned, size-bounded protocol, verifies the
Artifact Bundle again in the child, accepts no dynamic Provider or library path,
and applies process-tree and resource limits. Its opt-in Linux reference
profiles can additionally require `no_new_privs`, user/mount/network namespaces,
a Landlock ABI-v5 filesystem allowlist rooted at a parent-owned path, or a
narrow Linux x86-64 seccomp deny-list. A separate cgroup-v2 profile creates a
child boundary only where the parent has explicit controller delegation. A
missing kernel feature, denied control, or unavailable cgroup delegation
rejects that profile; it never falls back to the ambient boundary.

These profiles are defense in depth rather than a complete container. A
deployment that requires filesystem, network, identity, namespace, or syscall
isolation selects and validates the OS controls that match its own Provider
authority.

## Reporting and documentation rules

The in-process runtime is not a sandbox, and no RSScript API or documentation
may describe it as one. Vulnerability reports and deployment guidance identify
which boundary failed: compiler invariant, verifier, runtime limit, provider,
native code, or external isolation.

## Out of scope

Language-level permissions, deployment policy, capability grants, and a package
trust hierarchy are outside Core. So is any claim that static review can prove
arbitrary host code safe.
