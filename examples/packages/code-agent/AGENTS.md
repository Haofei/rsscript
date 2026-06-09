# Code Agent — Agent Working Notes

A small but real RSScript agent loop: it talks to an OpenAI-compatible
chat-completions endpoint, runs a narrow tool set, and feeds structured tool
results back into the next turn. Keep it readable and reviewable — this package
is an example, so clarity beats cleverness.

`README.md` has the full architecture, the env-var config table, and the safety
model. This file is the **operating contract for editing the package**.

## Read before writing RSScript

1. Root `AGENT.md` — the language + package-manager guide (syntax, effects,
   `rsspkg.toml`). RSS is not in your training data; do not guess it.
2. This file — package conventions and where things go.
3. `schemas/core-package-index.json`, then the exact `.rssi` it names. Core
   signatures live under `stdlib/`; standard packages under `packages/*/interface/`.

Do not invent stdlib or package APIs. If a signature is not in an `.rssi`, it
does not exist.

## Architecture map (one responsibility per file)

| File | Owns |
| --- | --- |
| `src/config.rss` | Env-derived config: model, endpoint, key, budgets, retry, sandbox roots. |
| `src/protocol.rss` | Builds the model request JSON (response/usage/tool-call parsing live in the `rss-chat-completions` dependency). |
| `src/state.rss` | Structured chat-message history. |
| `src/tool_types.rss` | `ToolRequest` / `ToolOutput` / `ToolAction` types and shared helpers. |
| `src/tool_specs.rss` | JSON tool schemas advertised to the model. |
| `src/tool_file.rss` | File tools: `read`, `write`, `edit`, `apply_patch`. |
| `src/tool_command.rss` | Command tools: `shell`, `rss_check`, `rss_cmd`, `rss_ide`, `finish`. |
| `src/tools.rss` | `ToolRuntime` dispatch + chat-history glue. No tool logic here. |
| `src/main.rss` | Bounded agent loop / orchestration only. |

Keep the split. Do not collapse files together or move tool logic into the
orchestration or dispatch layers.

## Adding or changing a tool (the canonical recipe)

A tool is `fn execute_<name>(arguments: read String, config: read AgentConfig)
-> fresh ToolAction` (`arguments` is the raw JSON text from the model; omit
`config` only for pure tools like `finish`). To add one:

1. **Type the arguments**: declare a `struct <Name>Args derives(Clone,
   JsonDecode) { ... }` in `tool_types.rss` (reuse an existing one if it fits).
2. **Implement** `execute_<name>` in the matching file — file operations in
   `tool_file.rss`, process/command operations in `tool_command.rss`. Decode with
   `Json.decode_text<<Name>Args>(text: read arguments)`; on a decode error or
   missing required field, return `ToolAction.error(content: ...)` — never a
   silent default. Return a normal `ToolOutput` response on success; set `abort`
   only to stop the whole run.
3. **Dispatch** it: add a `"<name>" => execute_<name>(...)` arm to
   `execute_core_tool` in `tools.rss`.
4. **Advertise** it: add `<name>_tool_spec()` in `tool_specs.rss` and include it
   in the `tool_specs()` list, or the model will never call it. The schema must
   match the `<Name>Args` shape.

## Invariants (do not break)

- **Explicit dispatch.** Tools are routed through `ToolRuntime.execute` and the
  `match` in `execute_core_tool`. Do not add callback/registry indirection.
- **Structured history.** Tool results go back as `role=tool` messages carrying
  the original tool-call id (see `push_tool_response`). Never append tool output
  to a natural-language transcript.
- **RSScript commands stay reviewable.** Use `rss_check` / `rss_cmd` for RSS
  commands; `shell` deliberately refuses them.
- **Sandbox stays intact.** `write` / `edit` / `apply_patch` are confined to
  `AGENT_WORKSPACE_ROOT`, reject absolute paths and `..`, and cap content by
  `AGENT_MAX_WRITE_BYTES`. Read-only tools resolve under `AGENT_REPO_ROOT`. Keep
  these checks; do not widen scope.
- **No capability creep.** Do not add `features: native` or `features: local` to
  the agent source — this loop is plain managed code on purpose.
- **Bounded loop.** Respect `max_steps` / `max_tool_calls`, and the cumulative
  token budget `max_total_tokens` (checked against the accumulated `usage_total`).
  Surface exhaustion as a `turn.budget_exhausted` / `turn.token_budget_exhausted`
  event and exit through `state.failed`, rather than looping unbounded.

## Verify changes

```sh
rss check examples/packages/code-agent          # type/effect check the package
AGENT_API_KEY=test_key cargo run -- run examples/packages/code-agent   # run the loop
```
