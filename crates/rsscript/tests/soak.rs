//! Soak, demo, and benchmark integration tests.
//!
//! This is the single target for slow or release-shaped checks. Expensive flows
//! stay `#[ignore]` and are run through the soak/demo manifest.
#![allow(clippy::duplicate_mod)]

#[path = "file_upload_benchmark_e2e.rs"]
mod file_upload_benchmark_e2e;
#[path = "s3_iam_reir_demo_e2e.rs"]
mod s3_iam_reir_demo_e2e;
