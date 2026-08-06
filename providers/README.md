# Host providers

These crates are composition-root packages. Each one implements explicit
`rsscript-provider-api` symbols and may access only its named host service:

- `fs`: filesystem text I/O;
- `env`: environment lookup;
- `process`: guarded child-process execution;
- `http`: synchronous HTTP GET;
- `time`: wall-clock Unix time;
- `entropy`: system entropy;
- `log`: injected or stderr logging sink;
- `cli`: captured command-line arguments.

The compiler, semantics, executable IR, and runtime-core do not depend on these
packages. Hosts select and register only the providers they need. A provider
descriptor and its implementation signatures are validated together before any
script executes.
