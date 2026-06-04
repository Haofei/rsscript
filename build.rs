use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct CorePackageIndex {
    schema: &'static str,
    generated_by: &'static str,
    default_core: Vec<CoreInterfaceEntry>,
    packages: Vec<PackageEntry>,
}

#[derive(Debug, Serialize)]
struct CoreInterfaceEntry {
    path: String,
    module: String,
    functions: Vec<String>,
    types: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PackageEntry {
    kind: PackageKind,
    name: String,
    version: String,
    path: String,
    interface_files: Vec<String>,
    source_files: Vec<String>,
    native_rust: Option<NativeRustEntry>,
    virtual_package: Option<VirtualEntry>,
    dependencies: Vec<String>,
    dev_dependencies: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum PackageKind {
    Core,
    Adapter,
    Package,
}

#[derive(Debug, Serialize)]
struct NativeRustEntry {
    crate_name: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct VirtualEntry {
    has_default: bool,
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    package: ManifestPackage,
    #[serde(default)]
    interfaces: ManifestPathSection,
    #[serde(default)]
    sources: ManifestPathSection,
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default)]
    native: Option<ManifestNative>,
    #[serde(default, rename = "virtual")]
    virtual_package: Option<ManifestVirtual>,
}

#[derive(Debug, Deserialize)]
struct ManifestPackage {
    name: String,
    version: String,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestPathSection {
    #[serde(default)]
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestNative {
    #[serde(default)]
    rust: Option<ManifestNativeRust>,
}

#[derive(Debug, Deserialize)]
struct ManifestNativeRust {
    #[serde(default)]
    enabled: bool,
    path: Option<String>,
    #[serde(rename = "crate")]
    crate_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestVirtual {
    #[serde(default)]
    has_default: bool,
    provider: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct InterpreterIntrinsicSpec {
    namespace: &'static str,
    name: &'static str,
    variant: &'static str,
    eval_kind: InterpreterEvalKind,
}

#[derive(Debug, Clone, Copy)]
enum InterpreterEvalKind {
    ArgsAll,
    ArgsCount,
    ArgsGet,
    ArgsGetOrDefault,
    AssertEqual,
    AssertEqualBool,
    AssertEqualInt,
    CharCompare,
    CharFromCode,
    CharIsAlphanumeric,
    CharIsAlpha,
    CharIsDigit,
    CharIsWhitespace,
    CharToCode,
    CharToString,
    IntToString,
    StringConcat,
    StringCopy,
    StringFromBool,
    StringIsEmpty,
    StringLen,
    StringLines,
    StringChars,
    StringSlice,
    StringSplit,
    StringToBytes,
    StringTrim,
    StringToLowercase,
    StringToUppercase,
    StringView,
    StringViewAfter,
    StringViewBefore,
    StringViewContains,
    StringViewIsEmpty,
    StringViewLen,
    StringViewSlice,
    StringViewStartsWith,
    StringViewToString,
    StringReplace,
    StringRepeat,
    StringContains,
    StringStartsWith,
    StringEndsWith,
    StringParseInt,
    StringIndexOf,
    StringStripPrefix,
    StringBefore,
    StringAfter,
    LogError,
    LogErrorJson,
    LogTrace,
    LogWrite,
    LogWriteJson,
}

const INTERPRETER_INTRINSICS: &[InterpreterIntrinsicSpec] = &[
    InterpreterIntrinsicSpec {
        namespace: "Args",
        name: "all",
        variant: "ArgsAll",
        eval_kind: InterpreterEvalKind::ArgsAll,
    },
    InterpreterIntrinsicSpec {
        namespace: "Args",
        name: "count",
        variant: "ArgsCount",
        eval_kind: InterpreterEvalKind::ArgsCount,
    },
    InterpreterIntrinsicSpec {
        namespace: "Args",
        name: "get",
        variant: "ArgsGet",
        eval_kind: InterpreterEvalKind::ArgsGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "Args",
        name: "get_or_default",
        variant: "ArgsGetOrDefault",
        eval_kind: InterpreterEvalKind::ArgsGetOrDefault,
    },
    InterpreterIntrinsicSpec {
        namespace: "Assert",
        name: "equal",
        variant: "AssertEqual",
        eval_kind: InterpreterEvalKind::AssertEqual,
    },
    InterpreterIntrinsicSpec {
        namespace: "Assert",
        name: "equal_bool",
        variant: "AssertEqualBool",
        eval_kind: InterpreterEvalKind::AssertEqualBool,
    },
    InterpreterIntrinsicSpec {
        namespace: "Assert",
        name: "equal_int",
        variant: "AssertEqualInt",
        eval_kind: InterpreterEvalKind::AssertEqualInt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Char",
        name: "compare",
        variant: "CharCompare",
        eval_kind: InterpreterEvalKind::CharCompare,
    },
    InterpreterIntrinsicSpec {
        namespace: "Char",
        name: "from_code",
        variant: "CharFromCode",
        eval_kind: InterpreterEvalKind::CharFromCode,
    },
    InterpreterIntrinsicSpec {
        namespace: "Char",
        name: "is_alphanumeric",
        variant: "CharIsAlphanumeric",
        eval_kind: InterpreterEvalKind::CharIsAlphanumeric,
    },
    InterpreterIntrinsicSpec {
        namespace: "Char",
        name: "is_alpha",
        variant: "CharIsAlpha",
        eval_kind: InterpreterEvalKind::CharIsAlpha,
    },
    InterpreterIntrinsicSpec {
        namespace: "Char",
        name: "is_digit",
        variant: "CharIsDigit",
        eval_kind: InterpreterEvalKind::CharIsDigit,
    },
    InterpreterIntrinsicSpec {
        namespace: "Char",
        name: "is_whitespace",
        variant: "CharIsWhitespace",
        eval_kind: InterpreterEvalKind::CharIsWhitespace,
    },
    InterpreterIntrinsicSpec {
        namespace: "Char",
        name: "to_code",
        variant: "CharToCode",
        eval_kind: InterpreterEvalKind::CharToCode,
    },
    InterpreterIntrinsicSpec {
        namespace: "Char",
        name: "to_string",
        variant: "CharToString",
        eval_kind: InterpreterEvalKind::CharToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Int",
        name: "to_string",
        variant: "IntToString",
        eval_kind: InterpreterEvalKind::IntToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "chars",
        variant: "StringChars",
        eval_kind: InterpreterEvalKind::StringChars,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "concat",
        variant: "StringConcat",
        eval_kind: InterpreterEvalKind::StringConcat,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "copy",
        variant: "StringCopy",
        eval_kind: InterpreterEvalKind::StringCopy,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "from_bool",
        variant: "StringFromBool",
        eval_kind: InterpreterEvalKind::StringFromBool,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "from_int",
        variant: "StringFromInt",
        eval_kind: InterpreterEvalKind::IntToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "is_empty",
        variant: "StringIsEmpty",
        eval_kind: InterpreterEvalKind::StringIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "len",
        variant: "StringLen",
        eval_kind: InterpreterEvalKind::StringLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "lines",
        variant: "StringLines",
        eval_kind: InterpreterEvalKind::StringLines,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "trim",
        variant: "StringTrim",
        eval_kind: InterpreterEvalKind::StringTrim,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "to_lowercase",
        variant: "StringToLowercase",
        eval_kind: InterpreterEvalKind::StringToLowercase,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "to_uppercase",
        variant: "StringToUppercase",
        eval_kind: InterpreterEvalKind::StringToUppercase,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "replace",
        variant: "StringReplace",
        eval_kind: InterpreterEvalKind::StringReplace,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "repeat",
        variant: "StringRepeat",
        eval_kind: InterpreterEvalKind::StringRepeat,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "contains",
        variant: "StringContains",
        eval_kind: InterpreterEvalKind::StringContains,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "starts_with",
        variant: "StringStartsWith",
        eval_kind: InterpreterEvalKind::StringStartsWith,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "ends_with",
        variant: "StringEndsWith",
        eval_kind: InterpreterEvalKind::StringEndsWith,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "parse_int",
        variant: "StringParseInt",
        eval_kind: InterpreterEvalKind::StringParseInt,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "slice",
        variant: "StringSlice",
        eval_kind: InterpreterEvalKind::StringSlice,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "split",
        variant: "StringSplit",
        eval_kind: InterpreterEvalKind::StringSplit,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "to_bytes",
        variant: "StringToBytes",
        eval_kind: InterpreterEvalKind::StringToBytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "view",
        variant: "StringView",
        eval_kind: InterpreterEvalKind::StringView,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringView",
        name: "after",
        variant: "StringViewAfter",
        eval_kind: InterpreterEvalKind::StringViewAfter,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringView",
        name: "before",
        variant: "StringViewBefore",
        eval_kind: InterpreterEvalKind::StringViewBefore,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringView",
        name: "contains",
        variant: "StringViewContains",
        eval_kind: InterpreterEvalKind::StringViewContains,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringView",
        name: "is_empty",
        variant: "StringViewIsEmpty",
        eval_kind: InterpreterEvalKind::StringViewIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringView",
        name: "len",
        variant: "StringViewLen",
        eval_kind: InterpreterEvalKind::StringViewLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringView",
        name: "slice",
        variant: "StringViewSlice",
        eval_kind: InterpreterEvalKind::StringViewSlice,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringView",
        name: "starts_with",
        variant: "StringViewStartsWith",
        eval_kind: InterpreterEvalKind::StringViewStartsWith,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringView",
        name: "to_string",
        variant: "StringViewToString",
        eval_kind: InterpreterEvalKind::StringViewToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "index_of",
        variant: "StringIndexOf",
        eval_kind: InterpreterEvalKind::StringIndexOf,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "strip_prefix",
        variant: "StringStripPrefix",
        eval_kind: InterpreterEvalKind::StringStripPrefix,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "before",
        variant: "StringBefore",
        eval_kind: InterpreterEvalKind::StringBefore,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "after",
        variant: "StringAfter",
        eval_kind: InterpreterEvalKind::StringAfter,
    },
    InterpreterIntrinsicSpec {
        namespace: "Log",
        name: "error",
        variant: "LogError",
        eval_kind: InterpreterEvalKind::LogError,
    },
    InterpreterIntrinsicSpec {
        namespace: "Log",
        name: "error_json",
        variant: "LogErrorJson",
        eval_kind: InterpreterEvalKind::LogErrorJson,
    },
    InterpreterIntrinsicSpec {
        namespace: "Log",
        name: "trace",
        variant: "LogTrace",
        eval_kind: InterpreterEvalKind::LogTrace,
    },
    InterpreterIntrinsicSpec {
        namespace: "Log",
        name: "write",
        variant: "LogWrite",
        eval_kind: InterpreterEvalKind::LogWrite,
    },
    InterpreterIntrinsicSpec {
        namespace: "Log",
        name: "write_json",
        variant: "LogWriteJson",
        eval_kind: InterpreterEvalKind::LogWriteJson,
    },
];

fn main() {
    if let Err(error) = write_core_package_index() {
        panic!("{error}");
    }
    if let Err(error) = write_interpreter_intrinsics() {
        panic!("{error}");
    }
}

fn write_core_package_index() -> Result<(), String> {
    println!("cargo:rerun-if-changed=core");
    println!("cargo:rerun-if-changed=rss");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let index = CorePackageIndex {
        schema: "rss.core_package_index.v1",
        generated_by: "build.rs",
        default_core: default_core_entries(&manifest_dir)?,
        packages: package_entries(&manifest_dir)?,
    };
    let json = serde_json::to_string_pretty(&index).expect("core package index should serialize");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(
        out_dir.join("rss-core-package-index.json"),
        format!("{json}\n"),
    )
    .map_err(|error| format!("core package index should be written: {error}"))?;
    Ok(())
}

fn write_interpreter_intrinsics() -> Result<(), String> {
    println!("cargo:rerun-if-changed=core");
    println!("cargo:rerun-if-changed=rss");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    ensure_interpreter_intrinsic_interfaces(&manifest_dir)?;
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(
        out_dir.join("rss-interpreter-intrinsics-enum.rs"),
        generated_interpreter_intrinsic_enum(),
    )
    .map_err(|error| format!("interpreter intrinsic enum should be written: {error}"))?;
    fs::write(
        out_dir.join("rss-interpreter-intrinsics-dispatch.rs"),
        generated_interpreter_intrinsic_dispatch(),
    )
    .map_err(|error| format!("interpreter intrinsic dispatcher should be written: {error}"))?;
    fs::write(
        out_dir.join("rss-interpreter-intrinsics-lookup.rs"),
        generated_interpreter_intrinsic_lookup(),
    )
    .map_err(|error| format!("interpreter intrinsic lookup should be written: {error}"))?;
    Ok(())
}

fn ensure_interpreter_intrinsic_interfaces(root: &Path) -> Result<(), String> {
    let mut functions = BTreeSet::new();
    let mut files = Vec::new();
    collect_files_with_extension(&root.join("core"), "rssi", &mut files)?;
    collect_files_with_extension(&root.join("rss"), "rssi", &mut files)?;
    for path in files {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        functions.extend(collect_functions(&source));
    }
    for intrinsic in INTERPRETER_INTRINSICS {
        let signature = format!("{}.{}", intrinsic.namespace, intrinsic.name);
        if !functions.contains(&signature) {
            return Err(format!(
                "interpreter intrinsic `{signature}` has no bundled public interface signature"
            ));
        }
    }
    Ok(())
}

fn generated_interpreter_intrinsic_enum() -> String {
    let mut out = String::new();
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n");
    out.push_str("pub(crate) enum InterpreterIntrinsic {\n");
    for intrinsic in INTERPRETER_INTRINSICS {
        out.push_str("    ");
        out.push_str(intrinsic.variant);
        out.push_str(",\n");
    }
    out.push_str("}\n");
    out
}

fn generated_interpreter_intrinsic_dispatch() -> String {
    let mut out = String::new();
    out.push_str(
        "fn eval_generated_runtime_intrinsic(\n    interpreter: &mut Interpreter<'_>,\n    intrinsic: InterpreterIntrinsic,\n    args: &[crate::hir::HirCallArg],\n) -> Result<Value, EvalError> {\n    match intrinsic {\n",
    );
    for intrinsic in INTERPRETER_INTRINSICS {
        out.push_str("        InterpreterIntrinsic::");
        out.push_str(intrinsic.variant);
        out.push_str(" => ");
        out.push_str(eval_kind_body(intrinsic.eval_kind));
        out.push_str(",\n");
    }
    out.push_str("    }\n}\n");
    out
}

fn generated_interpreter_intrinsic_lookup() -> String {
    let mut out = String::new();
    out.push_str(
        "pub(crate) fn generated_interpreter_intrinsic(\n    namespace: &str,\n    name: &str,\n) -> Option<InterpreterIntrinsic> {\n    match (namespace, name) {\n",
    );
    for intrinsic in INTERPRETER_INTRINSICS {
        out.push_str("        (\"");
        out.push_str(intrinsic.namespace);
        out.push_str("\", \"");
        out.push_str(intrinsic.name);
        out.push_str("\") => Some(InterpreterIntrinsic::");
        out.push_str(intrinsic.variant);
        out.push_str("),\n");
    }
    out.push_str("        _ => None,\n    }\n}\n\n");
    out.push_str(
        "pub(crate) fn generated_interpreter_intrinsic_signatures() -> Vec<String> {\n    vec![\n",
    );
    for intrinsic in INTERPRETER_INTRINSICS {
        out.push_str("        \"");
        out.push_str(intrinsic.namespace);
        out.push('.');
        out.push_str(intrinsic.name);
        out.push_str("\".to_string(),\n");
    }
    out.push_str("    ]\n}\n");
    out
}

fn eval_kind_body(kind: InterpreterEvalKind) -> &'static str {
    match kind {
        InterpreterEvalKind::ArgsAll => {
            "{\n            Ok(Value::List(interpreter.args.iter().cloned().map(Value::String).collect()))\n        }"
        }
        InterpreterEvalKind::ArgsCount => {
            "{\n            Ok(Value::Int(interpreter.args.len() as i64))\n        }"
        }
        InterpreterEvalKind::ArgsGet => {
            "{\n            let index = interpreter.eval_first_arg(args)?;\n            let index = expect_int(index)?;\n            let value = if index < 0 { None } else { interpreter.args.get(index as usize).cloned() };\n            Ok(value_option(value, Value::String))\n        }"
        }
        InterpreterEvalKind::ArgsGetOrDefault => {
            "{\n            let index = interpreter.eval_named_or_positional_arg(args, \"index\", 0)?;\n            let default = interpreter.eval_named_or_positional_arg(args, \"default\", 1)?;\n            let index = expect_int(index)?;\n            let default = expect_string(default)?;\n            Ok(Value::String(if index < 0 { default } else { interpreter.args.get(index as usize).cloned().unwrap_or(default) }))\n        }"
        }
        InterpreterEvalKind::AssertEqual => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let left = expect_string(left)?;\n            let right = expect_string(right)?;\n            if left != right {\n                return Err(EvalError::Runtime(format!(\"assertion failed: left `{left}` did not equal right `{right}`\")));\n            }\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::AssertEqualBool => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let left = expect_bool(left)?;\n            let right = expect_bool(right)?;\n            if left != right {\n                return Err(EvalError::Runtime(format!(\"assertion failed: left `{left}` did not equal right `{right}`\")));\n            }\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::AssertEqualInt => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let left = expect_int(left)?;\n            let right = expect_int(right)?;\n            if left != right {\n                return Err(EvalError::Runtime(format!(\"assertion failed: left `{left}` did not equal right `{right}`\")));\n            }\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::CharCompare => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let value = match expect_char(left)?.cmp(&expect_char(right)?) {\n                std::cmp::Ordering::Less => -1,\n                std::cmp::Ordering::Equal => 0,\n                std::cmp::Ordering::Greater => 1,\n            };\n            Ok(Value::Int(value))\n        }"
        }
        InterpreterEvalKind::CharFromCode => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(value_option(u32::try_from(expect_int(value)?).ok().and_then(char::from_u32), Value::Char))\n        }"
        }
        InterpreterEvalKind::CharIsAlphanumeric => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_char(value)?.is_ascii_alphanumeric()))\n        }"
        }
        InterpreterEvalKind::CharIsAlpha => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_char(value)?.is_ascii_alphabetic()))\n        }"
        }
        InterpreterEvalKind::CharIsDigit => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_char(value)?.is_ascii_digit()))\n        }"
        }
        InterpreterEvalKind::CharIsWhitespace => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_char(value)?.is_whitespace()))\n        }"
        }
        InterpreterEvalKind::CharToCode => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_char(value)? as u32 as i64))\n        }"
        }
        InterpreterEvalKind::CharToString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_char(value)?.to_string()))\n        }"
        }
        InterpreterEvalKind::IntToString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_int(value)?.to_string()))\n        }"
        }
        InterpreterEvalKind::StringChars => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::List(expect_string(value)?.chars().map(Value::Char).collect()))\n        }"
        }
        InterpreterEvalKind::StringConcat => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            Ok(Value::String(format!(\"{}{}\", expect_string(left)?, expect_string(right)?)))\n        }"
        }
        InterpreterEvalKind::StringCopy => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_string(value)?))\n        }"
        }
        InterpreterEvalKind::StringFromBool => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_bool(value)?.to_string()))\n        }"
        }
        InterpreterEvalKind::StringIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_string(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::StringLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_string(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::StringLines => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::List(expect_string(value)?.lines().map(|line| Value::String(line.to_string())).collect()))\n        }"
        }
        InterpreterEvalKind::StringTrim => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_string(value)?.trim().to_string()))\n        }"
        }
        InterpreterEvalKind::StringToLowercase => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_string(value)?.to_lowercase()))\n        }"
        }
        InterpreterEvalKind::StringToUppercase => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_string(value)?.to_uppercase()))\n        }"
        }
        InterpreterEvalKind::StringReplace => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let from = interpreter.eval_named_or_positional_arg(args, \"from\", 1)?;\n            let to = interpreter.eval_named_or_positional_arg(args, \"to\", 2)?;\n            Ok(Value::String(expect_string(value)?.replace(&expect_string(from)?, &expect_string(to)?)))\n        }"
        }
        InterpreterEvalKind::StringRepeat => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let count = interpreter.eval_named_or_positional_arg(args, \"count\", 1)?;\n            Ok(Value::String(expect_string(value)?.repeat(expect_int(count)?.max(0) as usize)))\n        }"
        }
        InterpreterEvalKind::StringContains => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let needle = interpreter.eval_named_or_positional_arg(args, \"needle\", 1)?;\n            Ok(Value::Bool(expect_string(value)?.contains(&expect_string(needle)?)))\n        }"
        }
        InterpreterEvalKind::StringStartsWith => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let prefix = interpreter.eval_named_or_positional_arg(args, \"prefix\", 1)?;\n            Ok(Value::Bool(expect_string(value)?.starts_with(&expect_string(prefix)?)))\n        }"
        }
        InterpreterEvalKind::StringEndsWith => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let suffix = interpreter.eval_named_or_positional_arg(args, \"suffix\", 1)?;\n            Ok(Value::Bool(expect_string(value)?.ends_with(&expect_string(suffix)?)))\n        }"
        }
        InterpreterEvalKind::StringParseInt => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(value_option(expect_string(value)?.parse::<i64>().ok(), Value::Int))\n        }"
        }
        InterpreterEvalKind::StringSlice => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let start = interpreter.eval_named_or_positional_arg(args, \"start\", 1)?;\n            let len = interpreter.eval_named_or_positional_arg(args, \"len\", 2)?;\n            Ok(Value::String(string_slice(expect_string(value)?, expect_int(start)?, expect_int(len)?)))\n        }"
        }
        InterpreterEvalKind::StringSplit => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let delimiter = interpreter.eval_named_or_positional_arg(args, \"delimiter\", 1)?;\n            Ok(Value::List(expect_string(value)?.split(&expect_string(delimiter)?).map(|part| Value::String(part.to_string())).collect()))\n        }"
        }
        InterpreterEvalKind::StringToBytes => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bytes(expect_string(value)?.into_bytes()))\n        }"
        }
        InterpreterEvalKind::StringView | InterpreterEvalKind::StringViewSlice => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let start = interpreter.eval_named_or_positional_arg(args, \"start\", 1)?;\n            let len = interpreter.eval_named_or_positional_arg(args, \"len\", 2)?;\n            Ok(Value::String(string_slice(expect_string(value)?, expect_int(start)?, expect_int(len)?)))\n        }"
        }
        InterpreterEvalKind::StringViewAfter => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let delimiter = interpreter.eval_named_or_positional_arg(args, \"delimiter\", 1)?;\n            let value = expect_string(value)?;\n            Ok(value_option(value.split_once(&expect_string(delimiter)?).map(|(_, right)| right.to_string()), Value::String))\n        }"
        }
        InterpreterEvalKind::StringViewBefore => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let delimiter = interpreter.eval_named_or_positional_arg(args, \"delimiter\", 1)?;\n            let value = expect_string(value)?;\n            let delimiter = expect_string(delimiter)?;\n            Ok(value_option(value.find(&delimiter).map(|index| value[..index].to_string()), Value::String))\n        }"
        }
        InterpreterEvalKind::StringViewContains => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let needle = interpreter.eval_named_or_positional_arg(args, \"needle\", 1)?;\n            Ok(Value::Bool(expect_string(value)?.contains(&expect_string(needle)?)))\n        }"
        }
        InterpreterEvalKind::StringViewIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_string(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::StringViewLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_string(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::StringViewStartsWith => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let prefix = interpreter.eval_named_or_positional_arg(args, \"prefix\", 1)?;\n            Ok(Value::Bool(expect_string(value)?.starts_with(&expect_string(prefix)?)))\n        }"
        }
        InterpreterEvalKind::StringViewToString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_string(value)?))\n        }"
        }
        InterpreterEvalKind::StringIndexOf => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let needle = interpreter.eval_named_or_positional_arg(args, \"needle\", 1)?;\n            let value = expect_string(value)?;\n            Ok(value_option(value.find(&expect_string(needle)?).map(|index| index as i64), Value::Int))\n        }"
        }
        InterpreterEvalKind::StringStripPrefix => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let prefix = interpreter.eval_named_or_positional_arg(args, \"prefix\", 1)?;\n            let value = expect_string(value)?;\n            Ok(value_option(value.strip_prefix(&expect_string(prefix)?).map(str::to_string), Value::String))\n        }"
        }
        InterpreterEvalKind::StringBefore => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let delimiter = interpreter.eval_named_or_positional_arg(args, \"delimiter\", 1)?;\n            let value = expect_string(value)?;\n            let delimiter = expect_string(delimiter)?;\n            Ok(value_option(value.find(&delimiter).map(|index| value[..index].to_string()), Value::String))\n        }"
        }
        InterpreterEvalKind::StringAfter => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let delimiter = interpreter.eval_named_or_positional_arg(args, \"delimiter\", 1)?;\n            let value = expect_string(value)?;\n            Ok(value_option(value.split_once(&expect_string(delimiter)?).map(|(_, right)| right.to_string()), Value::String))\n        }"
        }
        InterpreterEvalKind::LogError => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"message\", 0)?;\n            interpreter.stderr.push_str(&expect_string(value)?);\n            interpreter.stderr.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::LogErrorJson => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            interpreter.stderr.push_str(&expect_json(value)?.to_string());\n            interpreter.stderr.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::LogTrace => {
            "{\n            let event = interpreter.eval_named_or_positional_arg(args, \"event\", 0)?;\n            let message = interpreter.eval_named_or_positional_arg(args, \"message\", 1)?;\n            interpreter.stdout.push_str(\"trace \");\n            interpreter.stdout.push_str(&expect_string(event)?);\n            interpreter.stdout.push_str(\": \");\n            interpreter.stdout.push_str(&expect_string(message)?);\n            interpreter.stdout.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::LogWrite => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"message\", 0)?;\n            interpreter.stdout.push_str(&expect_string(value)?);\n            interpreter.stdout.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::LogWriteJson => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            interpreter.stdout.push_str(&expect_json(value)?.to_string());\n            interpreter.stdout.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
    }
}

fn default_core_entries(root: &Path) -> Result<Vec<CoreInterfaceEntry>, String> {
    let core_dir = root.join("core");
    let mut files = Vec::new();
    collect_files_with_extension(&core_dir, "rssi", &mut files)?;
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let relative = relative_path(root, &path);
            Ok(CoreInterfaceEntry {
                module: relative
                    .trim_start_matches("core/")
                    .trim_end_matches(".rssi")
                    .replace('/', "."),
                path: relative,
                functions: collect_functions(&source),
                types: collect_types(&source),
            })
        })
        .collect()
}

fn package_entries(root: &Path) -> Result<Vec<PackageEntry>, String> {
    let rss_dir = root.join("rss");
    let mut manifests = Vec::new();
    collect_named_files(&rss_dir, "rsspkg.toml", &mut manifests)?;
    manifests.sort();
    manifests
        .into_iter()
        .map(|manifest_path| package_entry(root, &manifest_path))
        .collect()
}

fn package_entry(root: &Path, manifest_path: &Path) -> Result<PackageEntry, String> {
    let package_dir = manifest_path.parent().ok_or_else(|| {
        format!(
            "package manifest has no parent: {}",
            manifest_path.display()
        )
    })?;
    let source = fs::read_to_string(manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))?;
    let relative_dir = relative_path(root, package_dir);
    let native_rust = manifest
        .native
        .and_then(|native| native.rust)
        .and_then(|rust| {
            rust.enabled.then_some(NativeRustEntry {
                crate_name: rust.crate_name,
                path: rust.path,
            })
        });
    Ok(PackageEntry {
        kind: package_kind(&relative_dir),
        name: manifest.package.name,
        version: manifest.package.version,
        path: relative_dir,
        interface_files: package_files(
            package_dir,
            &manifest.interfaces.paths,
            "interface",
            "rssi",
        )?,
        source_files: package_files(package_dir, &manifest.sources.paths, "src", "rss")?,
        native_rust,
        virtual_package: manifest
            .virtual_package
            .map(|virtual_package| VirtualEntry {
                has_default: virtual_package.has_default,
                provider: virtual_package.provider,
            }),
        dependencies: manifest.dependencies.keys().cloned().collect(),
        dev_dependencies: manifest.dev_dependencies.keys().cloned().collect(),
    })
}

fn package_kind(path: &str) -> PackageKind {
    if path.starts_with("rss/core/") {
        PackageKind::Core
    } else if path.starts_with("rss/adapters/") {
        PackageKind::Adapter
    } else {
        PackageKind::Package
    }
}

fn package_files(
    package_dir: &Path,
    roots: &[String],
    default_root: &str,
    extension: &str,
) -> Result<Vec<String>, String> {
    let roots = if roots.is_empty() {
        vec![default_root.to_string()]
    } else {
        roots.to_vec()
    };
    let mut files = Vec::new();
    for root in roots {
        collect_files_with_extension(&package_dir.join(root), extension, &mut files)?;
    }
    files.sort();
    Ok(files
        .into_iter()
        .map(|path| relative_path(package_dir, &path))
        .collect())
}

fn collect_functions(source: &str) -> Vec<String> {
    let mut functions = collect_symbols_after_keywords(source, &["fn"]);
    functions.sort();
    functions.dedup();
    functions
}

fn collect_types(source: &str) -> Vec<String> {
    let mut types =
        collect_symbols_after_keywords(source, &["struct", "resource", "sum", "protocol"]);
    types.sort();
    types.dedup();
    types
}

fn collect_symbols_after_keywords(source: &str, keywords: &[&str]) -> Vec<String> {
    let mut symbols = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if starts_line_comment(bytes, index) {
            index = skip_line_comment(bytes, index + 2);
            continue;
        }
        if bytes[index] == b'"' {
            index = skip_string_literal(bytes, index + 1);
            continue;
        }
        if !is_ident_start(bytes[index]) {
            index += 1;
            continue;
        }
        let token_start = index;
        index += 1;
        while index < bytes.len() && is_ident_continue(bytes[index]) {
            index += 1;
        }
        let token = &source[token_start..index];
        if keywords.contains(&token) {
            index = skip_ascii_whitespace(bytes, index);
            if index < bytes.len() && is_symbol_start(bytes[index]) {
                let symbol_start = index;
                index += 1;
                while index < bytes.len() && is_symbol_continue(bytes[index]) {
                    index += 1;
                }
                symbols.push(source[symbol_start..index].to_string());
            }
        }
    }
    symbols
}

fn starts_line_comment(bytes: &[u8], index: usize) -> bool {
    bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'/')
}

fn skip_line_comment(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

fn skip_string_literal(bytes: &[u8], mut index: usize) -> usize {
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            break;
        }
    }
    index
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_symbol_start(byte: u8) -> bool {
    is_ident_start(byte)
}

fn is_symbol_continue(byte: u8) -> bool {
    is_ident_continue(byte) || byte == b'.'
}

fn collect_named_files(
    path: &Path,
    file_name: &str,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        if path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        collect_named_files(&entry.path(), file_name, files)?;
    }
    Ok(())
}

fn collect_files_with_extension(
    path: &Path,
    extension: &str,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        collect_files_with_extension(&entry.path(), extension, files)?;
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
