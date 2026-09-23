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
  to the reference isolated runner. A natively executed region reports the
  interpreter's exact source-step count whether or not a limit is armed, and
  stops for the same reason under an armed step budget, cancellation token, or
  deadline; a region whose step cost cannot be attributed exactly declines to the
  interpreter instead of running unmetered. Allocation controls are admitted only
  with a per-region proof. An armed Provider-call budget no longer refuses native
  dispatch, because a Provider call is a barrier generated code never lowers and
  the interpreter charges every one of them. The intrinsic-call budget no longer
  refuses it either: every native item carries an explicit intrinsic cost beside
  its source cost, charged into the same call-owned limits cell, so a natively
  executed region reports the interpreter's exact `intrinsic_calls` armed or not
  and stops with the interpreter's reason under an armed budget. The logical
  `max_depth` is forwarded to whole-function entry as well as OSR, and entry
  declines when the configured limit is within reach of the region's static frame
  bound. `rss run --native` therefore keeps the default runner limit profile
  rather than replacing it with the unbounded trusted-host profile. See
  `docs/spec/native-jit-contract.md` for the parity status table and the
  remaining gaps.
- Review tooling reports evidence; authority stays with the host.

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
narrow Linux x86-64/AArch64 seccomp deny-list. A separate cgroup-v2 profile
creates a child boundary only where the parent has explicit controller
delegation. A missing kernel feature, denied control, or unavailable cgroup
delegation rejects that profile; it never falls back to the ambient boundary.

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

## Reference isolated runner

`rss run` executes Core bytecode in a separately bounded child process by
default. `--trusted-in-process` is the explicit escape hatch for a Rust host that
already trusts the script.

The parent sends `rsscript.runner_request.v1` plus one Artifact Bundle over a
length-prefixed protocol. The child:

1. rejects oversized or unknown protocol messages;
2. re-verifies the complete Artifact Bundle;
3. links only Providers installed by its local profile;
4. applies VM budgets and a monotonic deadline;
5. on Linux/Android, checks that the strict child launch installed kernel
   `no_new_privs` before it parses the Artifact;
6. returns `rsscript.runner_response.v1` containing the host-selected profile
   identity plus the normal
   `rsscript.execution_report.v2`.

Runner termination and VM termination are separate. A protocol, verification,
or link rejection is a runner failure. Script errors, cancellation, deadlines,
and budget exhaustion remain in the execution report.

The reference profile currently installs no host-service Providers. Requests
cannot name a dynamic library, Provider implementation path, credential, root
directory, or network allowlist; those authorities belong to a runner profile
constructed by the host.

The CLI exposes the same closed set of preinstalled presets through `rss profile
[--json] [profile-name]` and `rss run --profile <profile-name> …`. Selecting a
profile can never supply authority-bearing configuration; `rss profile` prints
only its stable name, non-secret identity, version, and descriptor digest.

The child receives process-tree, CPU, address-space, open-file, and file-size
limits where the platform supports them. This is defense in depth, not a
security-sandbox claim. Production isolation for untrusted input still requires
deployment-specific filesystem, network, identity, namespace, container, or
syscall controls.

The exact per-target control status is published in
[`architecture/runner-platforms.toml`](architecture/runner-platforms.toml).
`required` means the reference runner fails closed if the control cannot be
installed, `conditional` means the host must opt into and provision it,
`best_effort` is an availability control rather than an isolation guarantee,
and `unsupported` means the runner makes no claim for that control.
