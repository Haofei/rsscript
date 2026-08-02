//! Spec §2.5 — review semantic diff (RSR codes)
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn review_diff_detects_sum_type_added() {
    let old_source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let new_source = r#"
sum Color {
    Red
    Green
    Blue
}

fn main() -> Unit {
    return Unit
}
"#;
    let findings = rsscript::review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|f| f.code == "RSR018" && f.summary.contains("Color")),
        "should detect sum type addition: {findings:?}"
    );
}

#[test]
fn review_diff_detects_sum_type_variant_field_changed() {
    let old_source = r#"
sum PackageError {
    Io(path: String)
    Invalid(code: Int)
}

fn main() -> Unit {
    return Unit
}
"#;
    let new_source = r#"
sum PackageError {
    Io(path: Path)
    Invalid(code: Int)
}

fn main() -> Unit {
    return Unit
}
"#;
    let findings = rsscript::review_sources("old.rss", old_source, "new.rss", new_source);
    let finding = findings
        .iter()
        .find(|finding| finding.code == "RSR018" && finding.summary.contains("PackageError"))
        .unwrap_or_else(|| panic!("should detect sum type variant field change: {findings:?}"));

    assert_eq!(
        finding.before.as_deref(),
        Some("variants: Io(path: String), Invalid(code: Int)")
    );
    assert_eq!(
        finding.after.as_deref(),
        Some("variants: Io(path: Path), Invalid(code: Int)")
    );
}

#[test]
fn review_diff_detects_const_added() {
    let old_source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let new_source = r#"
const MAX_SIZE: Int = 100

fn main() -> Unit {
    return Unit
}
"#;
    let findings = rsscript::review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|f| f.code == "RSR019" && f.summary.contains("MAX_SIZE")),
        "should detect const addition: {findings:?}"
    );
}

#[test]
fn review_diff_detects_type_alias_added() {
    let old_source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let new_source = r#"
type Name = String

fn main() -> Unit {
    return Unit
}
"#;
    let findings = rsscript::review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|f| f.code == "RSR020" && f.summary.contains("Name")),
        "should detect type alias addition: {findings:?}"
    );
}

#[test]
fn review_diff_detects_function_removed() {
    let old_source = r#"
fn helper(x: read Int) -> Int {
    return x
}

fn main() -> Unit {
    return Unit
}
"#;
    let new_source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let findings = rsscript::review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|f| f.code == "RSR002" && f.summary.contains("helper")),
        "should detect function removal: {findings:?}"
    );
}

#[test]
fn review_diff_detects_parallel_boundary_added() {
    let old_source = r#"
fn work(x: read Int) -> Int {
    return x
}

fn main() -> Unit {
    return Unit
}
"#;
    let new_source = r#"
fn work(x: read Int) -> Int
{
    return x
}

fn main() -> Unit {
    return Unit
}
"#;
    let findings = rsscript::review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|f| f.code == "RSR017" && f.summary.contains("work")),
        "should detect parallel boundary addition: {findings:?}"
    );
}
