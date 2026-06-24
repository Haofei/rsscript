//! Spec §6 — type and field model
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn hashable_struct_is_accepted_as_map_key_and_set_element() {
    let source = r#"
struct Coord derives(Clone, Eq, Hash) {
    x: Int
    y: Int
}

fn main() -> Unit {
    let m = Map.new<Coord, Int>()
    let c = Coord(x: 1, y: 2)
    Map.insert(map: mut m, key: read c, value: read 10)
    let here = Map.contains_key(map: read m, key: read c)
    let s = Set.new<Coord>()
    let added = Set.insert(set: mut s, value: read c)
    return Unit
}
"#;
    assert_eq!(
        analyze_source_with_core("hashable-key.rss", source),
        Vec::new()
    );
}

#[test]
fn non_hashable_struct_map_key_is_rejected_in_rsscript() {
    // A struct without `derives(Hash)` is not `Hashable`, so it must be rejected
    // with RS0032 in RSScript rather than leaking a rustc trait-bound error.
    let source = r#"
struct Coord derives(Clone) {
    x: Int
    y: Int
}

fn main() -> Unit {
    let m = Map.new<Coord, Int>()
    let c = Coord(x: 1, y: 2)
    Map.insert(map: mut m, key: read c, value: read 10)
    return Unit
}
"#;
    let codes = analyze_source_with_core("non-hashable-key.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"RS0032".to_string()),
        "expected RS0032, got {codes:?}"
    );
}

#[test]
fn non_hashable_set_element_is_rejected_in_rsscript() {
    let source = r#"
struct Coord derives(Clone) {
    x: Int
    y: Int
}

fn main() -> Unit {
    let s = Set.new<Coord>()
    let c = Coord(x: 1, y: 2)
    let added = Set.insert(set: mut s, value: read c)
    return Unit
}
"#;
    let codes = analyze_source_with_core("non-hashable-set.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"RS0032".to_string()),
        "expected RS0032, got {codes:?}"
    );
}

#[test]
fn interned_uop_struct_lowers_to_runnable_package() {
    // Canonical target case: a tinygrad-style UOp struct used as a Map<Uop, Uop>
    // intern key and a dedup Set<Uop>, end-to-end through Rust lowering.
    let source = r#"
struct Uop derives(Clone, Eq, Hash) {
    op: Int
    children: List<Int>
    arg: Option<Int>
}

fn main() -> Unit {
    let intern = Map.new<Uop, Uop>()
    let a = Uop(op: 1, children: List.new<Int>(), arg: Some(7))
    Map.insert(map: mut intern, key: read a, value: read a)
    let here = Map.contains_key(map: read intern, key: read a)
    let seen = Set.new<Uop>()
    let added = Set.insert(set: mut seen, value: read a)
    return Unit
}
"#;
    assert_eq!(analyze_source_with_core("uop.rss", source), Vec::new());
    let package = lower_source_to_rust_package("uop.rss", source, "uop", &common::runtime_path())
        .unwrap_or_else(|diagnostics| panic!("uop.rss: {diagnostics:?}"));
    assert!(package.main_rs.is_some());
    let main_rs = package.main_rs.unwrap();
    assert!(!main_rs.contains("todo!"));
    // The derived struct hash/eq lower to Rust derives that make it a HashMap key.
    assert!(package.lib_rs.contains("Hash"));
}

#[test]
fn checker_reports_call_argument_type_mismatch_before_backend_lowering() {
    let source = r#"
fn main() -> Result<Unit, JsonError> {
    Log.write(message: read 42)
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source_with_core("arg-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `message` for `Log.write` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_constructor_field_type_mismatch_before_backend_lowering() {
    let source = r#"
struct User {
    name: String
}

fn build() -> User {
    return User(name: read 42)
}
"#;
    let diagnostics = analyze_source("constructor-field-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `name` for `User` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_call_argument_type_mismatch_before_rustc() {
    let source = r#"
fn main() -> Result<Unit, JsonError> {
    Log.write(message: read 42)
    return Ok(Unit)
}
"#;
    let diagnostics = lower_source_to_rust("arg-type.rss", source)
        .expect_err("argument type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_option_argument_payload_type_mismatch() {
    let source = r#"
fn accept(value: read Option<String>) -> Unit
fn main() -> Unit {
    accept(value: read Some(42))
    return Unit
}
"#;
    let diagnostics = analyze_source("option-arg-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `value` for `accept` has payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_result_argument_payload_type_mismatch() {
    let source = r#"
class BuildError {
    code: Int
}

fn accept(value: read Result<String, BuildError>) -> Unit
fn main() -> Unit {
    accept(value: read Err("bad"))
    return Unit
}
"#;
    let diagnostics = analyze_source("result-arg-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `value` for `accept` has payload type `String`, expected `BuildError`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_return_type_mismatch_before_backend_lowering() {
    let source = r#"
fn build() -> String {
    return 42
}
"#;
    let diagnostics = analyze_source("return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary == "return in `build` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_return_type_mismatch_before_rustc() {
    let source = r#"
fn build() -> String {
    return 42
}
"#;
    let diagnostics = lower_source_to_rust("return-type.rss", source)
        .expect_err("return type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0208"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_function_fallthrough_return_type_mismatch() {
    let source = r#"
fn build() -> String {
    42
}
"#;
    let diagnostics = analyze_source("fallthrough-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary == "return in `build` has type `Unit`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_let_type_annotation_mismatch_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    let value: String = 42
    return Unit
}
"#;
    let diagnostics = analyze_source("let-annotation-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `value` has initializer type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_result_binding_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
class BuildError {
    code: Int
}

fn main() -> Unit {
    let value: Result<String, BuildError> = Ok(42)
    return Unit
}
"#;
    let diagnostics = analyze_source("result-binding-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `value` has initializer payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_nested_result_option_binding_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
class BuildError {
    code: Int
}

fn main() -> Unit {
    let value: Result<Option<String>, BuildError> = Ok(Some(42))
    return Unit
}
"#;
    let diagnostics = analyze_source("nested-result-option-binding-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `value` has initializer payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_generic_binding_annotation_mismatch_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    let values: List<String> = List<Int>.new()
    return Unit
}
"#;
    let diagnostics = analyze_source("generic-binding-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `values` has initializer type `List<Int>`, expected `List<String>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_generic_call_argument_mismatch_before_backend_lowering() {
    let source = r#"
fn accept(values: read List<String>) -> Unit {
    return Unit
}

fn main() -> Unit {
    let values = List<Int>.new()
    accept(values: read values)
    return Unit
}
"#;
    let diagnostics = analyze_source("generic-call-arg-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `values` for `accept` has type `List<Int>`, expected `List<String>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_nested_result_option_argument_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
class BuildError {
    code: Int
}

fn accept(value: read Result<Option<String>, BuildError>) -> Unit {
    return Unit
}

fn main() -> Unit {
    accept(value: read Ok(Some(42)))
    return Unit
}
"#;
    let diagnostics = analyze_source("nested-result-option-argument-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `value` for `accept` has payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_option_binding_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    let value: Option<String> = Some(42)
    return Unit
}
"#;
    let diagnostics = analyze_source("option-binding-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `value` has initializer payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_let_type_annotation_mismatch_before_rustc() {
    let source = r#"
fn main() -> Unit {
    let value: String = 42
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("let-annotation-type.rss", source)
        .expect_err("binding type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_generic_binding_annotation_mismatch_before_rustc() {
    let source = r#"
fn main() -> Unit {
    let values: List<String> = List<Int>.new()
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("generic-binding-type.rss", source)
        .expect_err("generic binding mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_result_binding_payload_type_mismatch_before_rustc() {
    let source = r#"
class BuildError {
    code: Int
}

fn main() -> Unit {
    let value: Result<String, BuildError> = Ok(42)
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("result-binding-payload-type.rss", source)
        .expect_err("binding payload mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_nested_result_option_return_payload_mismatch_before_rustc() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<Option<String>, BuildError> {
    return Ok(Some(42))
}
"#;
    let diagnostics = lower_source_to_rust("nested-result-option-return-payload-type.rss", source)
        .expect_err("nested return payload mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0208"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_result_ok_payload_type_mismatch() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<String, BuildError> {
    return Ok(42)
}
"#;
    let diagnostics = analyze_source("result-ok-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary == "Ok payload in `build` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_nested_result_option_return_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<Option<String>, BuildError> {
    return Ok(Some(42))
}
"#;
    let diagnostics = analyze_source("nested-result-option-return-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary
                    == "Some payload in `build` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_result_err_payload_type_mismatch() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<String, BuildError> {
    return Err("bad")
}
"#;
    let diagnostics = analyze_source("result-err-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary
                    == "Err payload in `build` has type `String`, expected `BuildError`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_option_some_payload_type_mismatch() {
    let source = r#"
fn maybe_name() -> Option<String> {
    return Some(42)
}
"#;
    let diagnostics = analyze_source("option-some-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary
                    == "Some payload in `maybe_name` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_non_option_result_match_scrutinee_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    let value = "yes"
    match value {
        Some(result) => return Unit
        None => return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-scrutinee-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0209"
                && diagnostic.summary
                    == "match pattern `Some` cannot match scrutinee type `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_non_option_result_match_scrutinee_before_rustc() {
    let source = r#"
fn main() -> Unit {
    let value = "yes"
    match value {
        Some(result) => return Unit
        None => return Unit
    }
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("match-scrutinee-type.rss", source)
        .expect_err("match scrutinee type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0209"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_option_match_result_variants_before_backend_lowering() {
    let source = r#"
fn maybe() -> Option<String> {
    return Some("x")
}

fn main() -> Unit {
    let value = maybe()
    match value {
        Ok(result) => return Unit
        Err(error) => return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-variant-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0209"
                && diagnostic.summary
                    == "match pattern `Ok` cannot match scrutinee type `Option<String>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_match_variant_mismatch_before_rustc() {
    let source = r#"
fn maybe() -> Result<String, BuildError> {
    return Ok("x")
}

fn main() -> Unit {
    let value = maybe()
    match value {
        Some(result) => return Unit
        None => return Unit
    }
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("match-variant-type.rss", source)
        .expect_err("match variant mismatch should fail before Rust generation");

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0209"
                && diagnostic.summary
                    == "match pattern `Some` cannot match scrutinee type `Result<String, BuildError>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_uses_structured_sum_pattern_field_type() {
    let source = r#"
sum Expr {
    Call(callee: String)
}

fn main(expr: read Expr) -> Unit {
    match read expr {
        Call { callee } => {
            if callee == 1 {
                return Unit
            }
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-sum-field-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `==` has operands `String` and `Int`, expected matching operand types."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_uses_nested_option_result_pattern_binding_type() {
    let source = r#"
fn build() -> Result<Option<String>, BuildError> {
    return Ok(Some("rss"))
}

fn main() -> Unit {
    match build() {
        Ok(Some(value)) => {
            if value == 1 {
                return Unit
            }
        }
        Ok(None) => {
            return Unit
        }
        Err(error) => {
            return Unit
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-nested-option-result.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `==` has operands `String` and `Int`, expected matching operand types."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_uses_nested_struct_field_pattern_binding_type() {
    let source = r#"
sum Callee {
    Name(value: String)
    Builtin
}

sum Expr {
    Call(callee: Callee)
}

fn main(expr: read Expr) -> Unit {
    match read expr {
        Call { callee: Name(value) } => {
            if value == 1 {
                return Unit
            }
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-nested-field.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `==` has operands `String` and `Int`, expected matching operand types."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_non_exhaustive_nested_struct_field_pattern() {
    let source = r#"
sum Callee {
    Name(value: String)
    Builtin
}

sum Expr {
    Call(callee: Callee)
}

fn main(expr: read Expr) -> String {
    match read expr {
        Call { callee: Name(value) } => {
            return read value
        }
    }
}
"#;
    let diagnostics = analyze_source("match-nested-field-non-exhaustive.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0021"
                && diagnostic.label == "non-exhaustive match"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_accepts_exhaustive_nested_struct_field_patterns() {
    let source = r#"
sum Callee {
    Name(value: String)
    Builtin
}

sum Expr {
    Call(callee: Callee)
}

fn main(expr: read Expr) -> String {
    match read expr {
        Call { callee: Name(value) } => {
            return read value
        }
        Call { callee: Builtin } => {
            return "builtin"
        }
    }
}
"#;
    let diagnostics = analyze_source("match-nested-field-exhaustive.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0021"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_non_exhaustive_multi_field_pattern_matrix() {
    let source = r#"
sum Pair {
    Both(left: Bool, right: Bool)
}

fn main(pair: read Pair) -> String {
    match read pair {
        Both { left: true, right: true } => {
            return "tt"
        }
        Both { left: true, right: false } => {
            return "tf"
        }
        Both { left: false, right: true } => {
            return "ft"
        }
    }
}
"#;
    let diagnostics = analyze_source("match-multi-field-matrix-missing.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0021"
                && diagnostic.label == "non-exhaustive match"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_accepts_exhaustive_multi_field_pattern_matrix() {
    let source = r#"
sum Pair {
    Both(left: Bool, right: Bool)
}

fn main(pair: read Pair) -> String {
    match read pair {
        Both { left: true, right: true } => {
            return "tt"
        }
        Both { left: true, right: false } => {
            return "tf"
        }
        Both { left: false, right: true } => {
            return "ft"
        }
        Both { left: false, right: false } => {
            return "ff"
        }
    }
}
"#;
    let diagnostics = analyze_source("match-multi-field-matrix-complete.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0021"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_unknown_structured_pattern_fields() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

fn main(point: read Point) -> Int {
    return match read point {
        Point { x, z, .. } => {
            z
        }
    }
}
"#;
    let diagnostics = analyze_source("match-pattern-unknown-field.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0025"
                && diagnostic.summary == "unknown field `z` on type `Point`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_duplicate_structured_pattern_fields() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

fn main(point: read Point) -> Int {
    return match read point {
        Point { x, x, .. } => {
            x
        }
    }
}
"#;
    let diagnostics = analyze_source("match-pattern-duplicate-field.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0302"
                && diagnostic.summary == "pattern field `x` is listed more than once."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_requires_rest_marker_when_structured_pattern_omits_fields() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

fn main(point: read Point) -> Int {
    return match read point {
        Point { x } => {
            x
        }
    }
}
"#;
    let diagnostics = analyze_source("match-pattern-missing-rest.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0209" && diagnostic.summary.contains("omits fields without `..`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_mixed_equality_operand_types_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    if 1 == "1" {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("operator-equality-type.rss", source);

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
fn rust_lowering_rejects_mixed_equality_operand_types_before_rustc() {
    let source = r#"
fn main() -> Unit {
    if 1 == "1" {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("operator-equality-type.rss", source)
        .expect_err("operator type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0210"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_ordering_operand_types_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    if "a" > 1 {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("operator-ordering-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `>` has operands `String` and `Int`, expected numeric operands."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_logical_operand_types_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    if true && "yes" {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("operator-logical-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `&&` has operands `Bool` and `String`, expected Bool operands."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_allows_bare_result_success_returns_to_match_ok_type() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<String, BuildError> {
    return "ok"
}
"#;
    let diagnostics = analyze_source("result-bare-success.rss", source);

    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS0208"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_unknown_fields_before_backend_lowering() {
    let source = r#"
struct User {
    id: Int
}

fn main() -> Unit {
    let user = User(id: 1)
    Assert.equal_int(left: user.missing, right: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("unknown-field.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0025"
                && diagnostic.summary == "unknown field `missing` on type `User`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_unknown_fields_before_rustc() {
    let source = r#"
struct User {
    id: Int
}

fn main() -> Unit {
    let user = User(id: 1)
    Assert.equal_int(left: user.missing, right: 1)
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("unknown-field.rss", source)
        .expect_err("unknown field should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0025"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_unknown_types_before_backend_lowering() {
    let source = r#"
fn bad(value: read MissingType) -> Result<Unit, MissingError> {
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("unknown-type.rss", source);
    let unknown_types = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0024")
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        unknown_types,
        vec![
            "unknown type `MissingType`.",
            "unknown type `MissingError`."
        ],
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_unknown_types_before_rustc() {
    let source = r#"
fn bad(value: read MissingType) -> Unit {
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("unknown-type.rss", source)
        .expect_err("unknown type should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0024"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_accepts_exhaustive_option_match() {
    let source = r#"
fn pick() -> Int {
    let value = Some(42)
    match value {
        Some(result) => return result
        None => return 0
    }
}
"#;
    let diagnostics = analyze_source("match.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("match.rss", source).expect("match should lower");
    assert!(lowered.contains("match value"));
    assert!(lowered.contains("Some(result) =>"));
    assert!(lowered.contains("None =>"));
}

#[test]
fn checker_reports_non_exhaustive_option_match() {
    let source = r#"
fn pick() -> Int {
    let value = Some(42)
    match value {
        Some(result) => return result
    }
}
"#;
    let diagnostics = analyze_source("match.rss", source);

    assert!(diagnostics.iter().any(
        |diagnostic| diagnostic.code == "RS0021" && diagnostic.label == "non-exhaustive match"
    ));
}

#[test]
fn checker_accepts_wildcard_match_on_bare_none_literal() {
    // A bare `None` scrutinee resolves to `Option`, so a wildcard arm is
    // exhaustive. Regression: the analyzer's exhaustiveness helper formerly
    // failed to classify `None` as `Option` (its `builtin_value_type_name`
    // copy lacked the `None` arm the canonical `checks::shared` copy carries),
    // so it fell back to the constructor-name path and spuriously reported the
    // wildcard match as non-exhaustive.
    let source = r#"
fn pick() -> Int {
    match None {
        _ => return 0
    }
}
"#;
    let diagnostics = analyze_source("match-bare-none-wildcard.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0021"),
        "wildcard arm covers a bare `None` scrutinee; got {diagnostics:?}"
    );
}

#[test]
fn checker_reports_non_exhaustive_sum_type_match() {
    let source = r#"
sum Color {
    Red
    Green
    Blue
}

fn describe(c: read Color) -> String {
    match c {
        Red => { return "red" }
        Green => { return "green" }
    }
}
"#;
    let diagnostics = analyze_source("sum-match-non-exhaustive.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0021"
                && diagnostic.label == "non-exhaustive match"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_sum_type_variant_mismatch_in_match() {
    let source = r#"
sum Color {
    Red
    Green
    Blue
}

sum Size {
    Small
    Medium
    Large
}

fn describe(s: read Size) -> String {
    match s {
        Red => { return "red" }
        Green => { return "green" }
        Blue => { return "blue" }
    }
}
"#;
    let diagnostics = analyze_source("sum-match-mismatch.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0209" && d.summary.contains("cannot match scrutinee type")),
        "should reject matching Size with Color variants: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0021" && d.label == "non-exhaustive match"),
        "matching all Color variants must not count as exhaustive for Size: {diagnostics:?}"
    );
}

#[test]
fn checker_accepts_correct_sum_type_match() {
    let source = r#"
sum Color {
    Red
    Green
    Blue
}

fn describe(c: read Color) -> String {
    match c {
        Red => { return "red" }
        Green => { return "green" }
        Blue => { return "blue" }
    }
}
"#;
    let diagnostics = analyze_source("sum-match-correct.rss", source);
    assert!(
        diagnostics.is_empty(),
        "should accept matching Color with Color variants: {diagnostics:?}"
    );
}
