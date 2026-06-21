//! Static frontend/lowering/package integration tests.
//!
//! This is the single target for non-executing checks: parser/checker,
//! lowering/source-map assertions, package review, editor grammar, and docs.
#![allow(clippy::duplicate_mod)]

#[path = "agent_md_doctest.rs"]
mod agent_md_doctest;
#[path = "capability_taxonomy.rs"]
mod capability_taxonomy;
#[path = "checker_frontend.rs"]
mod checker_frontend;
#[path = "checker_lowering.rs"]
mod checker_lowering;
#[path = "checker_package.rs"]
mod checker_package;
#[path = "checker_review.rs"]
mod checker_review;
#[path = "cli_fix.rs"]
mod cli_fix;
#[path = "core_index.rs"]
mod core_index;
#[path = "execution_coverage.rs"]
mod execution_coverage;
#[path = "generate.rs"]
mod generate;
#[path = "native_audit.rs"]
mod native_audit;
#[path = "vscode_grammar.rs"]
mod vscode_grammar;
