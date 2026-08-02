//! Spec §6.4/§7.3 — resources and with-scopes
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn tempdir_keep_may_consume_with_resource() {
    let source = r#"

fn keep_tempdir() -> Result<Unit, FileError> {
    with TempDir.new()? as dir {
        let path = TempDir.keep(dir: take dir)
        Log.write(message: read Path.to_string(path: read path))
        Directory.remove_dir_all(path: read path)?
    }
    return Ok(Unit)
}
"#;
    assert_eq!(analyze_source("tempdir-keep.rss", source), Vec::new());
}

#[test]
fn checker_accepts_multi_field_pattern_matrix_with_wildcards() {
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
        Both { left: false, right: _ } => {
            return "f"
        }
    }
}
"#;
    let diagnostics = analyze_source("match-multi-field-matrix-wildcard.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0021"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_accepts_if_is_with_then_scoped_pattern_bindings() {
    let source = r#"
sum Expr {
    Call(callee: String, args: Int)
    Name(value: String)
}

fn main(expr: read Expr) -> String {
    if read expr is Call { callee, args } {
        if args == 0 {
            return callee
        }
    } else {
        return "other"
    }
    return "done"
}
"#;
    let diagnostics = analyze_source("if-is-pattern.rss", source);

    assert_eq!(diagnostics, Vec::new(), "{diagnostics:?}");
}

#[test]
fn file_core_resource_producers_return_result_contracts() {
    let program = core_interfaces()
        .iter()
        .find_map(|(file, source)| (file.contains("file.rssi")).then(|| parse_source(file, source)))
        .expect("File interface should be available");
    let functions = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();

    for name in ["File.open", "File.open_read", "File.open_write"] {
        let return_ty = functions
            .iter()
            .find(|function| function.name == name)
            .and_then(|function| function.return_ty.as_ref())
            .unwrap_or_else(|| panic!("{name} should have a return type"));
        assert_eq!(return_ty.name, "Result", "{name}");
        assert_eq!(
            return_ty.args.first().map(|arg| arg.name.as_str()),
            Some("File"),
            "{name}"
        );
        assert_eq!(
            return_ty.args.get(1).map(|arg| arg.name.as_str()),
            Some("FileError"),
            "{name}"
        );
    }
}
