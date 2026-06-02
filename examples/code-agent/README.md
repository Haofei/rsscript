# RSScript Code Agent Example

This package is a small RSScript code agent that talks to an OpenAI-compatible chat-completions endpoint, executes a narrow tool set, and feeds tool results back into the next model turn.

Run it against the local Codex bridge:

```sh
AGENT_API_KEY=test_key cargo run -- run examples/code-agent
```

The example is intentionally structured like a simplified Codex loop:

- `src/config.rss`: environment-derived model, endpoint, API key, loop budget, retry policy, and the write sandbox root.
- `src/protocol.rss`: request JSON, response extraction, and tool-call parsing.
- `src/state.rss`: transcript state and tool-result injection.
- `src/tools.rss`: narrow tool registry, path sandboxing, and tool execution.
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
| `AGENT_PROMPT` | (read-file task) | Override the agent task. |

## Safety and robustness

- **Write sandbox**: `write_file` only writes under `AGENT_WORKSPACE_ROOT`, and
  both tools reject absolute paths and `..` traversal, so the model cannot write
  outside the workspace.
- **HTTP errors**: a non-success response is logged as a `turn.failed` event and
  ends the loop instead of being parsed as if it were a successful turn.
- **Budget**: when the step budget is exhausted before the task finishes, the
  agent emits a `turn.budget_exhausted` event.

The agent should not guess RSScript APIs. It reads `examples/AGENTS.md`, then `schemas/core-package-index.json`, then the relevant `core/**/*.rssi` files before writing RSScript code.
