# RSScript Code Agent Example

This package is a small RSScript code agent that talks to an OpenAI-compatible chat-completions endpoint, executes a narrow tool set, and feeds tool results back into the next model turn.

Run it against the local Codex bridge:

```sh
AGENT_API_KEY=test_key cargo run -- run examples/packages/code-agent
```

The example is intentionally structured like a simplified Codex loop:

- `src/config.rss`: environment-derived model, endpoint, API key, loop budget, retry policy, and the write sandbox root.
- `src/protocol.rss`: request JSON, response extraction, usage extraction, and tool-call parsing.
- `src/state.rss`: structured chat message history.
- `src/tool_types.rss`: tool request/output/action types and shared helpers.
- `src/tool_specs.rss`: JSON schemas sent to the model.
- `src/tool_file.rss`: `read`, `write`, and `edit`.
- `src/tool_command.rss`: `shell`, `rss_check`, `rss_cmd`, `rss_ide`, and `finish`.
- `src/tools.rss`: explicit `ToolRuntime` dispatch and chat-history glue.
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
| `AGENT_MAX_READ_BYTES` | `200000` | Maximum file size the `read` tool will load. |
| `AGENT_MAX_WRITE_BYTES` | `200000` | Maximum new file content size for write/edit/patch tools. |
| `AGENT_TIMEOUT_MS` | `60000` | Per-request timeout. |
| `AGENT_MAX_ATTEMPTS` | `3` | HTTP retry attempts (transient failures). |
| `AGENT_BACKOFF_MS` | `500` | Backoff between retries. |
| `AGENT_WORKSPACE_ROOT` | `target/` | Write/edit/patch tools are confined to this safe relative prefix. |
| `AGENT_REPO_ROOT` | `RSS_RUN_WORKSPACE_ROOT` | Repository root used for read-only tools and RSScript checks. |
| `AGENT_PROMPT` | (read-file task) | Override the agent task. |

## Safety and robustness

- **Structured history**: model turns are stored as chat messages. Tool results
  are appended as `role=tool` messages with the original tool call id, not as
  natural-language transcript text.
- **Discovery tools**: `read` and `rss_ide` let the model inspect source files
  and indexed interfaces before it edits.
- **Edit tools**: `write` overwrites files and `edit` replaces exact text.
- **Command tools**: `rss_check` and `rss_cmd` run structured RSScript commands
  from the repository root; `shell` refuses RSScript commands so language
  checks stay reviewable. Command execution uses `ProcessRequest` with explicit
  `cwd`, timeout, merged stdout/stderr, and runtime-enforced output caps.
- **Finish tool**: `finish` ends the loop explicitly with a final answer.
- **Tool argument enforcement**: tool arguments must decode as the declared JSON
  object shape. Missing required fields or wrong types return an explicit tool
  error instead of falling back to a default action.
- **Read/write scope**: `read`, `rss_check`, `rss_cmd`, and `rss_ide` resolve
  safe relative paths from `AGENT_REPO_ROOT`. `write`, `edit`, and `apply_patch`
  are additionally confined to `AGENT_WORKSPACE_ROOT`, reject absolute paths and
  `..` traversal, and cap new content with `AGENT_MAX_WRITE_BYTES`. `read`
  refuses files larger than `AGENT_MAX_READ_BYTES`.
- **Real checks**: `rss_check` runs the package checker and returns
  status/stdout/stderr.
- **HTTP errors**: a non-success response is logged as a `turn.failed` event and
  fails the run instead of being parsed as if it were a successful turn.
- **Budget**: when the step budget is exhausted before the task finishes, the
  agent emits a `turn.budget_exhausted` event and returns an error.

The agent should not guess RSScript APIs. It reads `examples/packages/code-agent/AGENTS.md`, uses `rss_ide` or direct `read` calls against `schemas/core-package-index.json`, then opens the relevant indexed `.rssi` files under `core/` or `rss/*/interface/` before writing RSScript code.
