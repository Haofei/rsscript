//! Self-hosting stress-test harness (test-only).
//!
//! Runs the rss-written lexer (`selfhost/lexer.rss`) on rss source and compares
//! its canonical token dump against the real Rust lexer (`crate::lexer::lex`),
//! which defines truth. In-process: the corpus file content is passed to the rss
//! program and its stdout is compared with the Rust oracle.

include!("selfhost_parity/lexer.rs");
include!("selfhost_parity/parser.rs");
include!("selfhost_parity/checker.rs");
include!("selfhost_parity/ast_oracle.rs");
include!("selfhost_parity/ast_parity.rs");
