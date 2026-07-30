//! RSScript language and package producer adapter for REIR.
//! Converts RSScript compiler/package review output into REIR facts.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::*;

use super::bounded::{
    AdapterBuildError, AdapterLimits, BoundedEvidenceBuilder, ProducerProvenance, UnknownCoverage,
};

const FACT_SCHEMA: &str = "reir.fact.v0.1";
const EDGE_SCHEMA: &str = "reir.edge.v0.1";
const PRODUCER_VERSION: &str = "0.5.0";
const ADAPTER_VERSION: &str = "0.1";
const PRODUCER_SOURCE: &str = "compiler_contract";
const REVIEW_MAP_SOURCE: &str = "rsscript_review_map";
const PACKAGE_REVIEW_SOURCE: &str = "rsscript_package_review";
const PACKAGE_CHECK_SOURCE: &str = "rsscript_package_check";
const LOCKFILE_SOURCE: &str = "rsscript_lockfile";
const PACKAGE_METADATA_SOURCE: &str = "rsscript_package_metadata";
const TREE_SOURCE: &str = "rsscript_tree";
const REVIEW_REQUIRED_KIND: &str = "review_required";
const RSSCRIPT_ADAPTER_LIMITS: AdapterLimits =
    AdapterLimits::new(1_000_000, 250_000, 64 * 1024 * 1024);

include!("input.rs");
include!("normalization.rs");
include!("traversal.rs");
include!("facts.rs");
include!("provenance.rs");
include!("coverage.rs");
include!("pipeline.rs");

#[cfg(test)]
include!("tests.rs");
