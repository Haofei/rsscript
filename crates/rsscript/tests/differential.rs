//! Backend differential and generative integration tests.
//!
//! This is the single target for backend agreement: curated differentials,
//! fixture corpus, generated programs, examples, and metamorphic checks.
#![allow(clippy::duplicate_mod)]

#[path = "backend_differential.rs"]
mod backend_differential;
#[path = "differential_corpus.rs"]
mod differential_corpus;
#[path = "examples_exec.rs"]
mod examples_exec;
#[path = "generative.rs"]
mod generative;
#[path = "metamorphic.rs"]
mod metamorphic;
