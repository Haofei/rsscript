//! Curated Core backend differential integration tests.
//!
//! This is the single target for backend agreement: curated differentials,
//! fixture corpus, examples, and metamorphic checks. Generated-program testing
//! lives with the experimental `rss-testgen` workspace.
#![allow(clippy::duplicate_mod)]

#[path = "backend_differential.rs"]
mod backend_differential;
#[path = "differential_corpus.rs"]
mod differential_corpus;
#[path = "examples_exec.rs"]
mod examples_exec;
#[path = "metamorphic.rs"]
mod metamorphic;
