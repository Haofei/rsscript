# Code Agent Package Notes

This package demonstrates a real but small RSScript agent loop. Keep the example split by responsibility instead of collapsing it into one file.

When extending it:

1. Add tools in `src/tools.rss`.
2. Add response/request parsing in `src/protocol.rss`.
3. Add loop state changes in `src/state.rss`.
4. Keep `src/main.rss` as orchestration only.
5. Do not add `features: native` or `features: local` to the agent source.

For RSScript APIs, read `examples/AGENTS.md` and `schemas/core-package-index.json`, then open the exact `core/**/*.rssi` file named by the index.
