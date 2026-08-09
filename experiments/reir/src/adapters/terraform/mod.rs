//! Terraform/OpenTofu IaC producer adapter for REIR.
//! Converts rendered `.tf` IAM policy resources into granted capability facts.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::*;

use super::bounded::{AdapterLimits, BoundedEvidenceBuilder, ProducerProvenance, UnknownCoverage};

const FACT_SCHEMA: &str = "reir.fact.v0.1";
const PRODUCER_VERSION: &str = "0.1.0";
const ADAPTER_VERSION: &str = "0.1";
const PRODUCER_SOURCE: &str = "terraform_iac";
const SOURCE_EVIDENCE_REASON: &str =
    "Terraform source scan is not proof of rendered, planned, or deployed authorization";
const MAX_TERRAFORM_PARSE_DIAGNOSTICS: usize = 1_024;

include!("input.rs");
include!("normalization.rs");
include!("traversal.rs");
include!("facts.rs");
include!("coverage.rs");
include!("budget.rs");
include!("provenance.rs");
include!("pipeline.rs");

#[cfg(test)]
include!("tests.rs");
