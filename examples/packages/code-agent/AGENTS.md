# Code Agent Package Notes

This package demonstrates a real but small RSScript agent loop. Keep the example split by responsibility instead of collapsing it into one file.

When extending it:

1. Add tools in `src/tools.rss`.
2. Add response/request parsing in `src/protocol.rss`.
3. Add loop state changes in `src/state.rss`.
4. Keep `src/main.rss` as orchestration only.
5. Keep tool results as structured `role=tool` messages; do not append them to a natural-language transcript.
6. Keep dispatch explicit behind `ToolRuntime.execute`; do not add callback registries for tools.
7. Do not add `features: native` or `features: local` to the agent source.
8. Prefer `rss_check` and `rss_cmd` over `shell` for RSScript commands.

For the RSScript language and package manager themselves, read the root `AGENT.md` guide first — it covers syntax, the ownership/effect model, and `rsspkg.toml`. For RSScript APIs, read this file and `schemas/core-package-index.json`, then open the exact `.rssi` file named by the index. Built-in language/core signatures live under `core/`; standard packages such as `rss-async` live under `rss/*/interface/`.
