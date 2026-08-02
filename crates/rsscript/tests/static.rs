//! Static frontend/lowering/package integration tests.
//!
//! This is the single target for non-executing checks: parser/checker,
//! lowering/source-map assertions, package review, editor grammar, and docs.
#![allow(clippy::duplicate_mod)]

#[path = "agent_md_doctest.rs"]
mod agent_md_doctest;
#[path = "checker_frontend.rs"]
mod checker_frontend;
#[path = "checker_lowering.rs"]
mod checker_lowering;
#[path = "checker_package.rs"]
mod checker_package;
#[path = "cli_fix.rs"]
mod cli_fix;
#[path = "generate.rs"]
mod generate;
#[path = "language_deletion.rs"]
mod language_deletion;
#[path = "vscode_grammar.rs"]
mod vscode_grammar;
