// VS Code client for the RsScript language server (rss-lsp).
//
// Syntax highlighting works without this file (it is pure TextMate grammar).
// This client adds diagnostics, hover, and formatting by launching the
// `rss-lsp` binary and speaking LSP over stdio.

const { workspace, window } = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function resolveServerPath() {
  const configured = workspace
    .getConfiguration("rsscript")
    .get("server.path", "rss-lsp");
  // Allow ${workspaceFolder} in the setting for repo-local builds.
  const folder = workspace.workspaceFolders && workspace.workspaceFolders[0];
  if (folder) {
    return configured.replace("${workspaceFolder}", folder.uri.fsPath);
  }
  return configured;
}

function activate() {
  const config = workspace.getConfiguration("rsscript");
  if (!config.get("server.enable", true)) {
    return;
  }

  const command = resolveServerPath();
  const serverOptions = {
    run: { command, transport: TransportKind.stdio },
    debug: { command, transport: TransportKind.stdio },
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "rsscript" }],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.{rss,rssi}"),
    },
  };

  client = new LanguageClient(
    "rsscript",
    "RsScript Language Server",
    serverOptions,
    clientOptions
  );

  client.start().catch((error) => {
    window.showErrorMessage(
      `RsScript language server failed to start (${command}): ${error.message}. ` +
        `Build it with \`cargo build --release -p rss-lsp\` or set "rsscript.server.path".`
    );
  });
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
