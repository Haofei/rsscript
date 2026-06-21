# RsScript for VS Code

Syntax highlighting **and** language-server features (diagnostics, hover,
formatting) for RsScript source (`.rss`) and interface (`.rssi`) files.

- **Highlighting** is a pure TextMate grammar — works with no build step.
- **Diagnostics / hover / formatting** come from the `rss-lsp` language server,
  which reuses the `rsscript` checker library, so the editor agrees with the CLI.

## Language server (rss-lsp)

The server lives in the `lsp/` crate at the repo root. Build it:

```sh
cargo build --release -p rss-lsp     # produces target/release/rss-lsp
```

Then make the extension find it, either by:

- putting `rss-lsp` on your `PATH` (e.g. `cargo install --path lsp`), or
- setting `rsscript.server.path` to an absolute path, for repo-local builds:
  `"rsscript.server.path": "${workspaceFolder}/target/release/rss-lsp"`.

The client needs its npm dependency once:

```sh
cd editors/vscode
npm install
```

What the server provides:

| Feature | Source in the `rsscript` crate |
|---------|--------------------------------|
| Diagnostics (on open / edit / save) | `analyze_source_with_core` + `lint_source` |
| Hover (diagnostic code + explanation) | `explain_diagnostic_code` |
| Formatting (`Format Document`) | `format_source` |
| Go to Definition | `symbol_index` → `definition_at` |
| Find All References | `symbol_index` → `references_at` |

Edits sync **incrementally** (the editor sends only the changed range, not the
whole file). Navigation is **name-based and file-local** — it does not yet model
scopes or cross-file imports, so identical names in different functions resolve
to the same symbol. Scope-accurate resolution is a future refinement that would
reuse the analyzer's resolver.

Set `"rsscript.server.enable": false` to keep highlighting only.

## The grammar is generated — do not hand-edit it

`syntaxes/rsscript.tmLanguage.json` is generated from the keyword tables in
`src/lexer.rs` (`KEYWORDS`, `CONTEXTUAL_KEYWORDS`, `BUILTIN_CONSTANTS`). That is
the single source of truth shared with the lexer, so the highlighter can never
silently drift from the language.

When you add, rename, or recategorize a keyword in `src/lexer.rs`, regenerate:

```sh
cargo run --bin gen-grammar
```

A guard test fails if the committed grammar is stale, so you cannot forget:

```sh
cargo test --test static vscode_grammar
# vscode_grammar_is_up_to_date ... FAILED  ->  run `cargo run --bin gen-grammar`
```

To regenerate automatically on every commit, add a pre-commit hook:

```sh
cat > .git/hooks/pre-commit <<'EOF'
#!/bin/sh
cargo run --quiet --bin gen-grammar || exit 1
git add editors/vscode/syntaxes/rsscript.tmLanguage.json
EOF
chmod +x .git/hooks/pre-commit
```

The structural rules (comments, strings, numbers, calls, operators) live in
`src/editor_grammar.rs`; edit them there, not in the JSON.

## Try it locally (no packaging)

1. Copy or symlink this folder into your VS Code extensions directory:
   - macOS / Linux: `~/.vscode/extensions/rsscript`
   - Windows: `%USERPROFILE%\.vscode\extensions\rsscript`
2. Reload VS Code (`Cmd/Ctrl+Shift+P` → *Developer: Reload Window*).
3. Open any `.rss` / `.rssi` file.

Symlink example:

```sh
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/rsscript
```

## Run in the Extension Development Host

Open this `editors/vscode` folder in VS Code and press `F5`. A second
VS Code window launches with the extension loaded.

## Package as a .vsix

```sh
npm install -g @vscode/vsce
cd editors/vscode
vsce package
```

Install the produced `rsscript-0.1.0.vsix` via
*Extensions: Install from VSIX…* in the command palette.

## Inspecting scopes

To see which TextMate scope a token gets (useful when tweaking the grammar):
`Cmd/Ctrl+Shift+P` → *Developer: Inspect Editor Tokens and Scopes*.
