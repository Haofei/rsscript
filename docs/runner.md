# Reference isolated runner

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
