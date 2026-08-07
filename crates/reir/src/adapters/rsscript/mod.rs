//! One-way adapter from neutral RSScript package analysis into REIR facts.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::*;

use super::bounded::{
    AdapterBuildError, AdapterLimits, BoundedEvidenceBuilder, ProducerProvenance,
};

const FACT_SCHEMA: &str = "reir.fact.v0.1";
const PRODUCER_VERSION: &str = "0.5.0";
const ADAPTER_VERSION: &str = "0.1";
const PACKAGE_ANALYSIS_SCHEMA: &str = "rsscript.package_analysis.v1";
const PACKAGE_ANALYSIS_SOURCE: &str = "rsscript_package_analysis";
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
