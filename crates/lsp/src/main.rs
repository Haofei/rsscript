//! Language server for RsScript.
//!
//! Reuses the `rsscript` checker library directly: diagnostics come from the
//! same `analyze_source_with_core` + `lint_source` path as the CLI, and
//! formatting from `format_source`, so the editor never disagrees with the
//! command line.

mod backend;
mod diagnostics;
mod documents;
mod features;
mod publication;
mod scheduler;
mod scope;
mod source_index;
#[cfg(test)]
mod tests;
mod text;
mod workspace;

use backend::Backend;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
