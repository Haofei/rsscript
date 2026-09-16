# ADR 0169: Apply Linux no-new-privileges to strict runner children

- Status: Accepted
- Date: 2026-08-14

## Problem

The isolated CLI runner already used strict resource limits and a guarded
process tree on Linux, but it did not explicitly prohibit a child from gaining
new privileges through exec-time mechanisms. That left an available
kernel-level hardening primitive unused on the reference execution route.

## Decision and non-goals

`spawn_guarded_child_strict` now installs Linux/Android
`PR_SET_NO_NEW_PRIVS` in a child `pre_exec` hook before the runner program
starts. Any failure causes `Command::spawn` to fail, so strict callers never
receive a child after silently omitting the control. The regular guarded-child
API remains unchanged, so compiler and trusted helper processes do not acquire
an implicit behavior change.

The Unix CLI runner also starts the child in `/` rather than inheriting the
caller's project working directory. Its bundle arrives over stdin and the
runner executable path is absolute, so this removes an unnecessary ambient
filesystem reference without granting any filesystem isolation claim.

This is one defense-in-depth control only. It does not create namespaces,
restrict filesystem or network access, install seccomp, enforce cgroups, or
turn the in-process VM or isolated runner into a universal sandbox.

## Compatibility and migration

The runner protocol, Artifact format, Provider ABI, and SDK API are unchanged.
The effect is limited to the existing strict child-spawn path used by the
default isolated CLI runner on Linux/Android. Platforms without this kernel
control retain their existing strict process-limit support model and are not
advertised as equivalent.

## Verifier and security impact

Artifact verification and Provider linking still happen in the child before
execution. `no_new_privs` prevents later exec transitions from acquiring
additional privilege but cannot constrain already-authorized Provider code;
Provider authority remains host-owned.

## Provider and backend impact

No Provider or backend contract changes. Providers remain unavailable in the
reference `no_providers` profile.

## Evidence

The Linux/Android process-guard test reads the child's `/proc/self/status` and
requires `NoNewPrivs: 1`. The Core architecture test requires the default
isolated runner to use the strict guard and requires that guard to install the
kernel control.
