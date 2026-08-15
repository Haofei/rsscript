//! Spec §5A — statements, control flow, parser/lexer surface
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parser_preserves_large_scale_package_module_paths() {
    let source = r#"
module rss.package.review

use rss.package.contract.PackageContract
use rss.review.ReviewMap

fn main() -> Unit {
    return Unit
}
"#;
    let program = parse_source("package-review-module.rss", source);

    assert!(
        matches!(&program.items[0], Item::Module(module) if module.path == ["rss", "package", "review"])
    );
    assert!(
        matches!(&program.items[1], Item::Use(use_decl) if use_decl.path == ["rss", "package", "contract", "PackageContract"])
    );
    assert!(
        matches!(&program.items[2], Item::Use(use_decl) if use_decl.path == ["rss", "review", "ReviewMap"])
    );
}

#[test]
fn syntax_analysis_ignores_cross_file_semantic_references_for_fmt() {
    let source = r#"
fn use_other_file(config: read AgentConfig) -> Unit {
    OtherFile.helper(config)
    return Unit
}
"#;
    let diagnostics = analyze_syntax_source("fmt-cross-file.rss", source);

    assert_eq!(diagnostics, Vec::new());
}

#[test]
fn syntax_analysis_still_reports_malformed_parser_surface_for_fmt() {
    let source = r#"
fn broken() -> Unit {
    let =
}
"#;
    let diagnostics = analyze_syntax_source("fmt-broken.rss", source);

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "malformed statement"
    }));
}

#[test]
fn parser_preserves_type_visibility() {
    let source = r#"
pub struct PublicConfig {
    name: String
}

struct PrivateConfig {
    name: String
}

pub opaque resource PublicHandle
"#;
    let program = parse_source("type-visibility.rss", source);

    assert!(
        matches!(&program.items[0], Item::Type(type_decl) if type_decl.name == "PublicConfig" && type_decl.is_public)
    );
    assert!(
        matches!(&program.items[1], Item::Type(type_decl) if type_decl.name == "PrivateConfig" && !type_decl.is_public)
    );
    assert!(
        matches!(&program.items[2], Item::Type(type_decl) if type_decl.name == "PublicHandle" && type_decl.is_public && type_decl.is_opaque)
    );
}

#[test]
fn checker_reports_noescape_callback_operator_type_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn(Int) -> Bool) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: |value| value == "x")
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-operator-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `==` has operands `Int` and `String`, expected matching operand types."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_substitutes_explicit_generic_callback_parameter_types() {
    let source = r#"
fn main() -> Unit {
    match Option.filter<Int>(value: read Some(1), predicate: |item| {
        return item > 3
    }) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "none")
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("generic-callback-parameter.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0210"),
        "explicit generic arguments must apply to callback parameter expressions: {diagnostics:?}"
    );
}

#[test]
fn lint_warns_on_public_signature_complexity() {
    let source = r#"

pub fn overloaded<A, B, C, D>(
    first: read Result<Option<List<Map<String, Image>>>, Error>,
    second: read String,
    third: read String,
    fourth: read String,
    fifth: read String,
    sixth: read String,
    seventh: read String,
) -> Result<Option<List<Map<String, Image>>>, Error>
{
    return Ok(None)
}
"#;
    let diagnostics = lint_source("lint.rss", source);
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RSL001"));
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.severity.is_error())
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("7 parameters"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("4 generic parameters"))
    );
}

#[test]
fn parser_accepts_qualified_interface_function_signatures() {
    let source = r#"
struct HtmlEscaped

pub fn Html.escape(text: read String) -> fresh HtmlEscaped
"#;
    let program = parse_source("html.rssi", source);

    assert!(
        matches!(&program.items[1], Item::Function(function) if function.name == "Html.escape")
    );
}

#[test]
fn parser_rejects_removed_native_and_effect_syntax() {
    let source = "native fn Host.emit(message: read String) -> Unit effects(pure)\n";
    assert!(!analyze_source("host.rssi", source).is_empty());
}

#[test]
fn parser_preserves_explicit_fn_capture_contract() {
    let source = r#"

fn run() -> Int {
    let offset = 2
    local add = fn(value) captures(read offset) {
        return value + offset
    }
    return add(40)
}
"#;
    let program = parse_source("explicit-fn-capture.rss", source);
    let function = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .find(|function| function.name == "run")
        .expect("run should parse");
    let closure = function
        .body
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            rsscript_sdk::syntax::ast::Stmt::Let(stmt) => match stmt.value.as_ref() {
                Some(Expr::Closure { .. }) => stmt.value.as_ref(),
                _ => None,
            },
            _ => None,
        })
        .and_then(|expr| match expr {
            Expr::Closure {
                captures, explicit, ..
            } => Some((captures, explicit)),
            _ => None,
        })
        .expect("explicit fn expression should parse as closure");

    assert!(*closure.1);
    assert_eq!(closure.0.len(), 1);
    assert_eq!(closure.0[0].name, "offset");
    assert_eq!(closure.0[0].effect, DataEffect::Read);
}

#[test]
fn integer_literal_out_of_range_is_rejected() {
    let source = "fn main() -> Int {\n    return 99999999999999999999999999999999999999\n}\n";
    let diagnostics = analyze_source("bigint.rss", source);
    assert!(
        diagnostics.iter().any(|d| d.code == "RS0033"),
        "out-of-range integer literal should be RS0033, got {:?}",
        diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn max_i64_literal_is_accepted() {
    let source = "fn main() -> Int {\n    return 9223372036854775807\n}\n";
    let diagnostics = analyze_source("maxint.rss", source);
    assert!(
        !diagnostics.iter().any(|d| d.code == "RS0033"),
        "i64::MAX literal must not be flagged"
    );
}
