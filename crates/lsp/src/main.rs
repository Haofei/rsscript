//! Language server for RsScript.
//!
//! Reuses the frontend-only `rsscript-language-service` boundary: diagnostics
//! come from the same checker and lint path as the CLI, and formatting comes
//! from the same formatter. The protocol adapter never depends on the compiler
//! monolith, VM, Provider implementations, or optional backends.

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
