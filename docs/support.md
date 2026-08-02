# Support boundary

Supported language semantics include type checking, `read`/`mut`/`take`,
structured retention, local/manage ownership, resources/with, handle/weak rules,
and structured async control flow.

The default core is platform-neutral. Filesystem, environment, process, network,
wall-clock, entropy, logging, CLI, and OS-handle APIs require explicit packages
and runtime providers.

Execution limits, cancellation, deadlines, output caps, and child-process limits
are supported availability controls. RSScript does not claim that in-process VM,
JIT, generated code, or providers form a security sandbox. Deployment restriction
belongs to an independent runner/provider wrapper.
