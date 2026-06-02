# RSScript Code Agent Example

This package is a small RSScript code agent that talks to an OpenAI-compatible chat-completions endpoint, executes a narrow tool set, and feeds tool results back into the next model turn.

Run it against the local Codex bridge:

```sh
AGENT_API_KEY=test_key cargo run -- run examples/packages/code-agent
```

The example is intentionally structured like a simplified Codex loop:

- `src/config.rss`: environment-derived model, endpoint, API key, loop budget, retry policy, and the write sandbox root.
- `src/protocol.rss`: request JSON, response extraction, and tool-call parsing.
- `src/state.rss`: structured chat message history.
- `src/tools.rss`: narrow tool registry, `ToolRuntime`, path sandboxing, and tool execution.
- `src/checks.rss`: RSScript-specific validation helpers.
- `src/main.rss`: bounded agent loop.

## Configuration

All knobs are environment-driven (with safe defaults), so the loop budget and
network behavior are not hard-coded:

| Env var | Default | Meaning |
| --- | --- | --- |
| `AGENT_MODEL` | `gpt-5.5:medium` | Model name sent to the endpoint. |
| `AGENT_ENDPOINT` | `http://localhost:8080/v1/chat/completions` | Chat-completions URL. |
| `AGENT_API_KEY` | `test_key` | Bearer token. |
| `AGENT_MAX_STEPS` | `8` | Maximum model turns before the loop stops. |
| `AGENT_MAX_TOOL_CALLS` | `8` | Maximum tool calls consumed per model turn. |
| `AGENT_TIMEOUT_MS` | `60000` | Per-request timeout. |
| `AGENT_MAX_ATTEMPTS` | `3` | HTTP retry attempts (transient failures). |
| `AGENT_BACKOFF_MS` | `500` | Backoff between retries. |
| `AGENT_WORKSPACE_ROOT` | `target/` | `write_file` is confined to this prefix. |
| `AGENT_REPO_ROOT` | `RSS_RUN_WORKSPACE_ROOT` | Repository root used for read/list/search/check tools. |
| `AGENT_PROMPT` | (read-file task) | Override the agent task. |

## Safety and robustness

- **Structured history**: model turns are stored as chat messages. Tool results
  are appended as `role=tool` messages with the original tool call id, not as
  natural-language transcript text.
- **Discovery tools**: `list_files` and `search_text` let the model find source
  files and indexed interfaces before it reads or edits.
- **Write sandbox**: `write_file` only writes under `AGENT_WORKSPACE_ROOT`, and
  tools reject absolute paths and `..` traversal. Write results include
  `old_bytes`, `new_bytes`, and `changed`.
- **Real checks**: `check_rss_file` validates the current read-file task shape;
  `check_rss_package` runs the package checker and returns status/stdout/stderr.
- **HTTP errors**: a non-success response is logged as a `turn.failed` event and
  ends the loop instead of being parsed as if it were a successful turn.
- **Budget**: when the step budget is exhausted before the task finishes, the
  agent emits a `turn.budget_exhausted` event.

The agent should not guess RSScript APIs. It reads `examples/packages/code-agent/AGENTS.md`, then `schemas/core-package-index.json`, then the relevant indexed `.rssi` files under `core/` or `rss/*/interface/` before writing RSScript code.
