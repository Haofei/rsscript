//! Spec §2.6/§2A — lints and review budget
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn required_spec_diagnostics_have_regression_coverage() {
    let fixture_codes = common::fail_fixture_expected_code_set();
    let dedicated_test_codes = BTreeSet::from([
        "RS1102",  // rustc_diagnostics_report_unmappable_generated_spans
        "RS1201",  // runtime_diagnostic_lines_parse_to_rsscript_diagnostics
        "RS0310",  // checker_rejects_exclusive_use_of_for_read_view
        "PKG0101", // package feature resolution diagnostics
        "PKG0102", // unsupported package dependency source diagnostics
        "PKG0501", // package review policy diagnostics
        "PKG0601", // package native binding diagnostics
        "PKG0901", // package provider declaration diagnostics
    ]);

    for &(spec_class, code) in REQUIRED_SPEC_DIAGNOSTICS {
        assert!(
            explain_diagnostic_code(code).is_some(),
            "{spec_class} maps to {code}, but the code has no explanation"
        );
        assert!(
            fixture_codes.contains(code) || dedicated_test_codes.contains(code),
            "{spec_class} maps to {code}, but no fail fixture or dedicated regression test covers it"
        );
    }
}
