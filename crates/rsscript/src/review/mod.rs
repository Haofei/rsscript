//! Source-level review: the review-MAP (AST/HIR fact extraction + classification)
//! and the semantic-DIFF (signature contracts).
//!
//! This module was mechanically split from a single flat file into submodules.
//! All submodules share one logical namespace via `use super::*;`, so the public
//! re-exports below preserve every path consumed outside this module.

mod diff;
mod facts_ast;
mod facts_hir;
mod map;
pub(crate) use map::review_map_semantic_database;

use crate::text_util::{type_arg_names, type_root_name};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::diagnostic::{Span, code};
use crate::hir::{
    CallResolution, FunctionSig as HirFunctionSig, Hir, HirBindingKind, HirBlock, HirExpr, HirStmt,
    ParamEffect, ResolvedCalleeKind,
};
use crate::interfaces::standard_package_interfaces;
use crate::syntax::ast::{
    Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FileFeature, FunctionDecl,
    GenericBound, Item, LetKind, MatchPattern, Param, Program, ProtocolImpl, Stmt, TypeDecl,
    TypeKind, TypeRef, merge_programs,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewFinding {
    pub code: String,
    pub risk: ReviewRisk,
    pub summary: String,
    pub spans: Vec<ReviewSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub fixes: Vec<ReviewFix>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewSpan {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewFix {
    pub kind: String,
    pub title: String,
    pub applicability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMap {
    pub summary: ReviewMapSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<ReviewMapModule>,
    pub files: Vec<ReviewMapFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewMapSummary {
    pub total_functions: usize,
    pub total_lines: usize,
    pub must_review_lines: usize,
    pub low_semantic_risk_lines: usize,
    pub unknown_lines: usize,
    pub suggested_review_lines: usize,
    pub review_ratio: ReviewRatio,
    pub unknown_ratio: ReviewRatio,
    pub unknown_function_ratio: ReviewRatio,
    #[serde(rename = "must_review")]
    pub review_required: ReviewMapCategorySummary,
    #[serde(rename = "low_semantic_risk")]
    pub foldable: ReviewMapCategorySummary,
    pub unknown: ReviewMapCategorySummary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReviewRatio {
    scaled: u32,
}

impl Serialize for ReviewRatio {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_f64(f64::from(self.scaled) / 1000.0)
    }
}

impl ReviewRatio {
    fn from_parts(numerator: usize, denominator: usize) -> Self {
        if denominator == 0 {
            return Self { scaled: 0 };
        }
        let scaled = ((numerator.saturating_mul(1000)) / denominator).min(1000) as u32;
        Self { scaled }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewMapCategorySummary {
    pub functions: usize,
    pub lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapFile {
    pub file: String,
    pub features: Vec<String>,
    pub risk: ReviewMapFileRisk,
    pub reasons: Vec<String>,
    pub regions: Vec<ReviewMapRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapModule {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uses: Vec<ReviewMapUse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapUse {
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMapFileRisk {
    Low,
    Elevated,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapRegion {
    pub function: String,
    pub classification: ReviewMapClassification,
    pub line: usize,
    pub line_count: usize,
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receiver_calls: Vec<ReviewMapReceiverCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapReceiverCall {
    pub line: usize,
    pub column: usize,
    pub source: String,
    pub canonical_callee: String,
    pub self_effect: String,
    pub resolution: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMapClassification {
    #[serde(rename = "must_review")]
    ReviewRequired,
    #[serde(rename = "low_semantic_risk")]
    Foldable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewRisk {
    Feature,
    Api,
    TypeLayout,
    Effect,
    Boundary,
    Unsafe,
    Guarantee,
}

impl ReviewRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::Api => "api",
            Self::TypeLayout => "type-layout",
            Self::Effect => "effect",
            Self::Boundary => "boundary",
            Self::Unsafe => "unsafe",
            Self::Guarantee => "guarantee",
        }
    }
}

pub use diff::*;
use facts_ast::*;
use facts_hir::*;
pub use map::*;
