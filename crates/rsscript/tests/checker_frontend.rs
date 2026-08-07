#![allow(unused_imports, dead_code)]
mod common;
pub(crate) use rsscript::syntax::ast::{DataEffect, Expr, Item};
pub(crate) use rsscript::syntax::parse_source;
pub(crate) use rsscript::{
    Severity, analyze_source, analyze_source_with_core, analyze_source_with_interfaces,
    analyze_source_without_core, analyze_sources_with_interfaces, analyze_syntax_source,
    core_interfaces, explain_diagnostic_code, format_diagnostic_explanation,
    format_diagnostics_json, lint_source, lower_source_to_rust, lower_source_to_rust_package,
};
pub(crate) use serde_json::Value;
pub(crate) use std::collections::BTreeSet;
pub(crate) use std::path::Path;

const REQUIRED_SPEC_DIAGNOSTICS: &[(&str, &str)] = &[
    ("use after manage", "RS0401"),
    ("managed -> local attempt", "RS0301"),
    ("missing named argument", "RS0204"),
    ("missing read/mut/take effect", "RS0202"),
    ("call argument type mismatch", "RS0207"),
    ("return type mismatch", "RS0208"),
    ("control-flow type mismatch", "RS0209"),
    ("operator type mismatch", "RS0210"),
    ("derive requirement not satisfied", "RS0211"),
    ("unsupported resource derive", "RS0212"),
    ("invalid assignment", "RS0311"),
    ("assignment target deferred", "RS0312"),
    ("async function not lowerable", "RS0411"),
    ("cancellation token outside task_group", "RS0412"),
    ("assignment type mismatch", "RS0313"),
    ("same-call place conflict", "RS0302"),
    ("constructor/variant call-like conflict", "RS0203"),
    ("handle-field same-call conflict", "RS0303"),
    ("read view mutation", "RS0310"),
    ("retaining local value", "RS0501"),
    ("managed closure capturing local/resource", "RS0801"),
    (
        "managed closure capture retention in retained contexts",
        "RS0801",
    ),
    ("fresh function returning aliased value", "RS0601"),
    ("mut/take of unbound fresh expression", "RS0604"),
    ("resource escaping with", "RS0702"),
    ("resource wrapped in Ok/Some and escaping", "RS0702"),
    (
        "resource-producing expression used outside resource context",
        "RS0702",
    ),
    (
        "Result-returning resource producer missing explicit ?",
        "RS0706",
    ),
    (
        "invalid resource type in ordinary Result/Option/container context",
        "RS0704",
    ),
    ("local captured by managed closure", "RS0801"),
    ("Fd used outside native/resource internals", "RS0023"),
    ("noescape callback escape", "RS0802"),
    ("local closure escape", "RS0803"),
    ("noescape closure consuming a captured local", "RS0804"),
    ("take of handle field", "RS0901"),
    (
        "weak field initialized without explicit weak handle",
        "RS0904",
    ),
    ("weak field used without explicit upgrade", "RS0903"),
    ("implicit conversion attempt", "RS1002"),
    ("operator overload attempt", "RS1001"),
    ("unsupported syntax", "RS0015"),
    (
        "unstructured spawn used before source-level task support",
        "RS0015",
    ),
    ("async call not consumed", "RS0022"),
    ("unknown protocol", "RS0027"),
    ("unmappable rustc diagnostic", "RS1102"),
    ("package feature resolution violation", "PKG0101"),
    ("unsupported package dependency source", "PKG0102"),
    ("package review policy violation", "PKG0501"),
    ("package native binding metadata violation", "PKG0601"),
    ("package provider declaration violation", "PKG0901"),
];

#[path = "checker_frontend/async_resources.rs"]
mod async_resources;
#[path = "checker_frontend/conflicts.rs"]
mod conflicts;
#[path = "checker_frontend/lint.rs"]
mod lint;
#[path = "checker_frontend/parse.rs"]
mod parse;
#[path = "checker_frontend/types.rs"]
mod types;
#[path = "checker_frontend/world.rs"]
mod world;
