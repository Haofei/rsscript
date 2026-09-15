//! Optional package review.
//!
//! Four crates were merged here; each keeps its own module and the layering
//! between them is unchanged, so the direction of a reference still says which
//! layer it belongs to:
//!
//! * [`review_core`] — pure, policy-neutral interpretation of review evidence.
//!   It knows nothing about source files, manifests, providers, artifact
//!   persistence, or authorization, and must not depend on any other module
//!   here. It is not an execution permission mechanism.
//! * [`review_facts`] — source-level review facts: the review-MAP (AST/HIR
//!   extraction and classification) and the semantic-DIFF (signature
//!   contracts). Reads the frontend; does not read manifests or the filesystem.
//! * [`package_model`] — versioned package review and compatibility data
//!   models, including the compatibility re-exports of Artifact-owned analysis
//!   types. This is the compiler-independent evidence model.
//! * [`package`] — captured package review inputs and compatibility execution:
//!   manifest and source-set capture, dependency graphs, locks, policy, and
//!   presentation.
//!
//! Nothing in Core depends on this crate. It is an optional integration that
//! consumes the reviewed frontend, never the other way round.
#![forbid(unsafe_code)]
// Optional source-level review rendering keeps its lint debt out of compiler Core.

pub mod package;
pub mod package_model;
pub mod review_core;
pub mod review_facts;
