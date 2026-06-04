use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::analyze_source_with_interfaces;
use crate::diagnostic::Diagnostic;
use crate::hir::{Hir, HirBlock, HirCallArg, HirExpr, HirMatchArm, HirStmt};
use crate::interfaces::standard_package_interfaces;
use crate::runtime_abi;
use crate::runtime_abi::InterpreterIntrinsic;
use crate::syntax::ast::{BinaryOp, Callee, Expr, Item, MatchLiteral, MatchPattern, Program};
use crate::syntax::parse_source;

include!(concat!(
    env!("OUT_DIR"),
    "/rss-interpreter-intrinsics-dispatch.rs"
));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOutput {
    pub value: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    Diagnostics(Vec<Diagnostic>),
    Runtime(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InterpreterCoverageReport {
    pub runtime_intrinsics: CoverageBucket,
    pub hir_statements: CoverageBucket,
    pub hir_expressions: CoverageBucket,
    pub value_types: CoverageBucket,
    pub function_kinds: CoverageBucket,
    pub parity_features: CoverageBucket,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageBucket {
    pub all: Vec<String>,
    pub supported: Vec<String>,
    pub missing: Vec<String>,
}

impl CoverageBucket {
    pub fn total(&self) -> usize {
        self.all.len()
    }

    pub fn supported_count(&self) -> usize {
        self.supported.len()
    }

    pub fn missing_count(&self) -> usize {
        self.missing.len()
    }
}

const HIR_STMT_VARIANTS: &[&str] = &[
    "Let", "Return", "With", "If", "Loop", "For", "Match", "Select", "Assign", "Break", "Continue",
    "Expr", "Unknown",
];

const INTERPRETER_SUPPORTED_HIR_STMT_VARIANTS: &[&str] = &[
    "Let", "Return", "With", "If", "Loop", "For", "Match", "Assign", "Break", "Continue", "Expr",
];

const HIR_EXPR_VARIANTS: &[&str] = &[
    "Ident",
    "Number",
    "String",
    "ObjectLiteral",
    "MapLiteral",
    "ArrayLiteral",
    "Binary",
    "Field",
    "Index",
    "Call",
    "Effect",
    "Manage",
    "Spawn",
    "Await",
    "Try",
    "Closure",
    "Match",
    "Unknown",
];

const INTERPRETER_SUPPORTED_HIR_EXPR_VARIANTS: &[&str] = &[
    "Ident",
    "Number",
    "String",
    "ObjectLiteral",
    "MapLiteral",
    "ArrayLiteral",
    "Binary",
    "Field",
    "Index",
    "Call",
    "Effect",
    "Manage",
    "Try",
    "Match",
];

const VALUE_TYPES: &[&str] = &[
    "Unit", "Int", "Bool", "String", "Char", "Bytes", "List", "Map", "Json", "Struct", "Variant",
    "Managed", "Closure", "Native",
];

const INTERPRETER_SUPPORTED_VALUE_TYPES: &[&str] = &[
    "Unit", "Int", "Bool", "String", "Char", "Bytes", "List", "Map", "Json", "Struct", "Variant",
];

const FUNCTION_KINDS: &[&str] = &["sync", "async", "native"];
const INTERPRETER_SUPPORTED_FUNCTION_KINDS: &[&str] = &["sync"];

const INTERPRETER_HANDWRITTEN_INTRINSICS: &[(&str, &str)] = &[
    ("Args", "all"),
    ("Args", "count"),
    ("Args", "get"),
    ("Args", "get_or_default"),
    ("Assert", "equal"),
    ("Assert", "equal_bool"),
    ("Assert", "equal_int"),
    ("Buffer", "clear"),
    ("Buffer", "consume"),
    ("Buffer", "is_empty"),
    ("Buffer", "len"),
    ("Buffer", "new"),
    ("Buffer", "view"),
    ("BufferView", "is_empty"),
    ("BufferView", "len"),
    ("BufferView", "slice"),
    ("BufferView", "to_bytes"),
    ("Cache", "insert"),
    ("Cache", "lookup"),
    ("Cache", "new"),
    ("Clock", "now"),
    ("Clock", "system_unix_ms"),
    ("Config", "load"),
    ("Config", "name"),
    ("Config", "new"),
    ("Config", "rule_count"),
    ("ConfigStore", "name"),
    ("ConfigStore", "new"),
    ("ConfigStore", "replace"),
    ("Counter", "add"),
    ("Counter", "new"),
    ("Counter", "value"),
    ("Csv", "open_read"),
    ("Csv", "parse_row"),
    ("Csv", "read_into"),
    ("Deadline", "after"),
    ("Deadline", "after_ms"),
    ("Deadline", "is_expired"),
    ("Deadline", "remaining_ms"),
    ("Deque", "clear"),
    ("Deque", "is_empty"),
    ("Deque", "len"),
    ("Deque", "new"),
    ("Deque", "pop_back"),
    ("Deque", "pop_front"),
    ("Deque", "push_back"),
    ("Deque", "push_front"),
    ("Deque", "to_list"),
    ("Directory", "create"),
    ("Directory", "create_all"),
    ("Directory", "create_dir_all"),
    ("Directory", "exists"),
    ("Directory", "is_dir"),
    ("Directory", "is_file"),
    ("Directory", "list_files"),
    ("Directory", "list_paths"),
    ("Directory", "metadata"),
    ("Directory", "read_string"),
    ("Directory", "remove_dir_all"),
    ("Directory", "remove_file"),
    ("Directory", "copy_file"),
    ("Directory", "rename"),
    ("Directory", "write_string"),
    ("Duration", "add"),
    ("Duration", "as_ms"),
    ("Duration", "as_seconds"),
    ("Duration", "ms"),
    ("Duration", "seconds"),
    ("Env", "get"),
    ("Env", "get_or_default"),
    ("Env", "current_dir"),
    ("Env", "home_dir"),
    ("Env", "run_workspace_root"),
    ("Env", "set"),
    ("Env", "temp_dir"),
    ("File", "append_bytes"),
    ("File", "append_string"),
    ("File", "exists"),
    ("File", "open"),
    ("File", "open_read"),
    ("File", "open_write"),
    ("File", "read_all"),
    ("File", "read_all_string"),
    ("File", "read_bytes"),
    ("File", "read_into"),
    ("File", "read_string"),
    ("File", "remove"),
    ("File", "write"),
    ("File", "write_atomic"),
    ("File", "write_bytes"),
    ("File", "write_bytes_view"),
    ("File", "write_buffer"),
    ("File", "write_buffer_view"),
    ("File", "write_string"),
    ("File", "write_string_to_path"),
    ("FileError", "message"),
    ("Diff", "unified"),
    ("Bytes", "concat"),
    ("Bytes", "consume"),
    ("Bytes", "from_buffer"),
    ("Bytes", "from_string"),
    ("Bytes", "is_empty"),
    ("Bytes", "len"),
    ("Bytes", "slice"),
    ("Bytes", "view"),
    ("BytesView", "is_empty"),
    ("BytesView", "len"),
    ("BytesView", "slice"),
    ("BytesView", "starts_with"),
    ("BytesView", "to_bytes"),
    ("Json", "array"),
    ("Json", "array_bools"),
    ("Json", "array_contains_prefix"),
    ("Json", "array_contains_string"),
    ("Json", "array_contains_substring"),
    ("Json", "array_get"),
    ("Json", "array_ints"),
    ("Json", "array_len"),
    ("Json", "array_strings"),
    ("Json", "as_bool"),
    ("Json", "as_int"),
    ("Json", "as_string"),
    ("Json", "at"),
    ("Json", "at_bool"),
    ("Json", "at_bool_or"),
    ("Json", "at_int"),
    ("Json", "at_int_or"),
    ("Json", "at_optional"),
    ("Json", "at_optional_bool"),
    ("Json", "at_optional_int"),
    ("Json", "at_optional_string"),
    ("Json", "at_or"),
    ("Json", "at_string"),
    ("Json", "at_string_or"),
    ("Json", "at_to_string"),
    ("Json", "at_to_string_or"),
    ("Json", "bool_at"),
    ("Json", "bool_at_or"),
    ("Json", "bool_field"),
    ("Json", "clone"),
    ("Json", "field"),
    ("Json", "field_bool"),
    ("Json", "field_int"),
    ("Json", "field_optional"),
    ("Json", "field_optional_bool"),
    ("Json", "field_optional_int"),
    ("Json", "field_optional_string"),
    ("Json", "field_string"),
    ("Json", "is_array"),
    ("Json", "is_null"),
    ("Json", "is_object"),
    ("Json", "json_parse"),
    ("Json", "kind"),
    ("Json", "int_field"),
    ("Json", "int_at"),
    ("Json", "int_at_or"),
    ("Json", "json_bool_at_or"),
    ("Json", "json_int_at_or"),
    ("Json", "json_string_at_or"),
    ("Json", "object"),
    ("Json", "object_keys"),
    ("Json", "object_len"),
    ("Json", "parse"),
    ("Json", "parse_file"),
    ("Json", "quote_string"),
    ("Json", "raw_field"),
    ("Json", "string_array"),
    ("Json", "string_at"),
    ("Json", "string_at_or"),
    ("Json", "strings"),
    ("Json", "string_field"),
    ("Json", "to_string"),
    ("Json", "to_string_at"),
    ("Json", "to_string_at_or"),
    ("Json", "value_at"),
    ("Json", "value"),
    ("Json", "values"),
    ("JsonError", "message"),
    ("Hash", "sha256_bytes"),
    ("Hash", "sha256_file"),
    ("Hash", "sha256_string"),
    ("Http", "get"),
    ("Http", "post_form"),
    ("Http", "post_json"),
    ("HttpError", "message"),
    ("HttpRequest", "json"),
    ("HttpRequest", "with_header"),
    ("HttpRequest", "with_retry"),
    ("HttpRequest", "with_timeout"),
    ("ImageCache", "len"),
    ("ImageCache", "new"),
    ("Instant", "elapsed"),
    ("List", "append"),
    ("List", "clear"),
    ("List", "contains_value"),
    ("List", "consume"),
    ("List", "first"),
    ("List", "get"),
    ("List", "is_empty"),
    ("List", "join"),
    ("List", "last"),
    ("List", "len"),
    ("List", "new"),
    ("List", "pop"),
    ("List", "push"),
    ("List", "reverse"),
    ("List", "remove_at"),
    ("List", "set"),
    ("List", "skip"),
    ("List", "slice"),
    ("List", "sort"),
    ("List", "take"),
    ("List", "to_json_strings"),
    ("List", "to_json_values"),
    ("Map", "clear"),
    ("Map", "contains_key"),
    ("Map", "get"),
    ("Map", "get_or_default"),
    ("Map", "insert"),
    ("Map", "insert_old"),
    ("Map", "is_empty"),
    ("Map", "keys"),
    ("Map", "len"),
    ("Map", "new"),
    ("Map", "remove"),
    ("Map", "values"),
    ("PersistentMap", "clear"),
    ("PersistentMap", "contains_key"),
    ("PersistentMap", "get"),
    ("PersistentMap", "insert"),
    ("PersistentMap", "is_empty"),
    ("PersistentMap", "len"),
    ("PersistentMap", "new"),
    ("PersistentMap", "remove"),
    ("Regex", "captures"),
    ("Regex", "compile"),
    ("Regex", "find"),
    ("Regex", "is_match"),
    ("Regex", "replace_all"),
    ("Regex", "split"),
    ("RegexError", "message"),
    ("Request", "new"),
    ("Request", "path"),
    ("Response", "body"),
    ("Response", "ok"),
    ("Response", "status"),
    ("Option", "is_none"),
    ("Option", "is_some"),
    ("Option", "ok_or"),
    ("Option", "or"),
    ("Option", "unwrap_or"),
    ("Ord", "compare"),
    ("Path", "extension"),
    ("Path", "file_name"),
    ("Path", "from_string"),
    ("Path", "is_absolute"),
    ("Path", "exists"),
    ("Path", "is_dir"),
    ("Path", "is_file"),
    ("Path", "join"),
    ("Path", "list_files"),
    ("Path", "list_paths"),
    ("Path", "normalize"),
    ("Path", "parent"),
    ("Path", "read_string"),
    ("Path", "resolve_relative"),
    ("Path", "safe_relative"),
    ("Path", "starts_with"),
    ("Path", "to_string"),
    ("Path", "with_extension"),
    ("Path", "write_string"),
    ("Patch", "apply_text"),
    ("Result", "err"),
    ("Result", "err_message"),
    ("Result", "is_err"),
    ("Result", "is_ok"),
    ("Result", "ok"),
    ("Result", "unwrap_or"),
    ("Row", "field_string"),
    ("RowBuffer", "new"),
    ("RuleLoader", "load_rules"),
    ("Set", "clear"),
    ("Set", "contains"),
    ("Set", "difference"),
    ("Set", "insert"),
    ("Set", "intersection"),
    ("Set", "is_empty"),
    ("Set", "is_subset"),
    ("Set", "len"),
    ("Set", "new"),
    ("Set", "remove"),
    ("Set", "to_list"),
    ("Set", "union"),
    ("SortedSet", "clear"),
    ("SortedSet", "contains"),
    ("SortedSet", "insert"),
    ("SortedSet", "is_empty"),
    ("SortedSet", "len"),
    ("SortedSet", "new"),
    ("SortedSet", "remove"),
    ("SortedSet", "to_list"),
    ("SortedMap", "clear"),
    ("SortedMap", "contains_key"),
    ("SortedMap", "get"),
    ("SortedMap", "insert"),
    ("SortedMap", "is_empty"),
    ("SortedMap", "keys"),
    ("SortedMap", "len"),
    ("SortedMap", "new"),
    ("SortedMap", "remove"),
    ("SortedMap", "values"),
    ("String", "env"),
    ("String", "env_or"),
    ("String", "safe_relative"),
    ("String", "join"),
    ("String", "to_path"),
    ("String", "to_url"),
    ("StringBuilder", "finish"),
    ("StringBuilder", "new"),
    ("StringBuilder", "push"),
    ("TempDir", "new"),
    ("TempDir", "new_in"),
    ("TempDir", "path"),
    ("Url", "from_string"),
    ("Url", "to_string"),
    ("Workspace", "resolve"),
    ("GlobalConfig", "new"),
    ("GlobalConfig", "replace"),
    ("GlobalConfig", "rule_count"),
    ("Toml", "parse_file"),
    ("Yaml", "parse"),
    ("Yaml", "parse_file"),
];

const INTERPRETER_PARITY_FEATURES: &[&str] = &[
    "function:sync",
    "hir_expr:ArrayLiteral",
    "hir_expr:Binary",
    "hir_expr:Call",
    "hir_expr:Effect",
    "hir_expr:Field",
    "hir_expr:Ident",
    "hir_expr:Index",
    "hir_expr:Manage",
    "hir_expr:MapLiteral",
    "hir_expr:Match",
    "hir_expr:Number",
    "hir_expr:ObjectLiteral",
    "hir_expr:String",
    "hir_expr:Try",
    "hir_stmt:Assign",
    "hir_stmt:Break",
    "hir_stmt:Continue",
    "hir_stmt:Expr",
    "hir_stmt:For",
    "hir_stmt:If",
    "hir_stmt:Let",
    "hir_stmt:Loop",
    "hir_stmt:Match",
    "hir_stmt:Return",
    "hir_stmt:With",
    "runtime:Args.all",
    "runtime:Args.count",
    "runtime:Args.get",
    "runtime:Args.get_or_default",
    "runtime:Assert.equal",
    "runtime:Assert.equal_bool",
    "runtime:Assert.equal_int",
    "runtime:Char.compare",
    "runtime:Char.from_code",
    "runtime:Char.is_alpha",
    "runtime:Char.is_alphanumeric",
    "runtime:Char.is_digit",
    "runtime:Char.is_whitespace",
    "runtime:Char.to_code",
    "runtime:Char.to_string",
    "runtime:Buffer.clear",
    "runtime:Buffer.consume",
    "runtime:Buffer.is_empty",
    "runtime:Buffer.len",
    "runtime:Buffer.new",
    "runtime:Buffer.view",
    "runtime:BufferView.is_empty",
    "runtime:BufferView.len",
    "runtime:BufferView.slice",
    "runtime:BufferView.to_bytes",
    "runtime:Cache.insert",
    "runtime:Cache.lookup",
    "runtime:Cache.new",
    "runtime:Clock.now",
    "runtime:Clock.system_unix_ms",
    "runtime:Config.load",
    "runtime:Config.name",
    "runtime:Config.new",
    "runtime:Config.rule_count",
    "runtime:ConfigStore.name",
    "runtime:ConfigStore.new",
    "runtime:ConfigStore.replace",
    "runtime:Counter.add",
    "runtime:Counter.new",
    "runtime:Counter.value",
    "runtime:Csv.open_read",
    "runtime:Csv.parse_row",
    "runtime:Csv.read_into",
    "runtime:Deadline.after",
    "runtime:Deadline.after_ms",
    "runtime:Deadline.is_expired",
    "runtime:Deadline.remaining_ms",
    "runtime:Deque.clear",
    "runtime:Deque.is_empty",
    "runtime:Deque.len",
    "runtime:Deque.new",
    "runtime:Deque.pop_back",
    "runtime:Deque.pop_front",
    "runtime:Deque.push_back",
    "runtime:Deque.push_front",
    "runtime:Deque.to_list",
    "runtime:Directory.create",
    "runtime:Directory.create_all",
    "runtime:Directory.create_dir_all",
    "runtime:Directory.exists",
    "runtime:Directory.is_dir",
    "runtime:Directory.is_file",
    "runtime:Directory.list_files",
    "runtime:Directory.list_paths",
    "runtime:Directory.metadata",
    "runtime:Directory.read_string",
    "runtime:Directory.remove_dir_all",
    "runtime:Directory.remove_file",
    "runtime:Directory.copy_file",
    "runtime:Directory.rename",
    "runtime:Directory.write_string",
    "runtime:Duration.add",
    "runtime:Duration.as_ms",
    "runtime:Duration.as_seconds",
    "runtime:Duration.ms",
    "runtime:Duration.seconds",
    "runtime:Env.current_dir",
    "runtime:Env.get",
    "runtime:Env.get_or_default",
    "runtime:Env.home_dir",
    "runtime:Env.run_workspace_root",
    "runtime:Env.set",
    "runtime:Env.temp_dir",
    "runtime:File.append_bytes",
    "runtime:File.append_string",
    "runtime:File.exists",
    "runtime:File.open",
    "runtime:File.open_read",
    "runtime:File.open_write",
    "runtime:File.read_all",
    "runtime:File.read_all_string",
    "runtime:File.read_bytes",
    "runtime:File.read_into",
    "runtime:File.read_string",
    "runtime:File.remove",
    "runtime:File.write",
    "runtime:File.write_atomic",
    "runtime:File.write_bytes",
    "runtime:File.write_bytes_view",
    "runtime:File.write_buffer",
    "runtime:File.write_buffer_view",
    "runtime:File.write_string",
    "runtime:File.write_string_to_path",
    "runtime:FileError.message",
    "runtime:Diff.unified",
    "runtime:Bytes.concat",
    "runtime:Bytes.consume",
    "runtime:Bytes.from_buffer",
    "runtime:Bytes.from_string",
    "runtime:Bytes.is_empty",
    "runtime:Bytes.len",
    "runtime:Bytes.slice",
    "runtime:Bytes.view",
    "runtime:BytesView.is_empty",
    "runtime:BytesView.len",
    "runtime:BytesView.slice",
    "runtime:BytesView.starts_with",
    "runtime:BytesView.to_bytes",
    "runtime:Int.to_string",
    "runtime:Json.array",
    "runtime:Json.array_bools",
    "runtime:Json.array_contains_prefix",
    "runtime:Json.array_contains_string",
    "runtime:Json.array_contains_substring",
    "runtime:Json.array_get",
    "runtime:Json.array_ints",
    "runtime:Json.array_len",
    "runtime:Json.array_strings",
    "runtime:Json.as_bool",
    "runtime:Json.as_int",
    "runtime:Json.as_string",
    "runtime:Json.at",
    "runtime:Json.at_bool",
    "runtime:Json.at_bool_or",
    "runtime:Json.at_int",
    "runtime:Json.at_int_or",
    "runtime:Json.at_optional",
    "runtime:Json.at_optional_bool",
    "runtime:Json.at_optional_int",
    "runtime:Json.at_optional_string",
    "runtime:Json.at_or",
    "runtime:Json.at_string",
    "runtime:Json.at_string_or",
    "runtime:Json.at_to_string",
    "runtime:Json.at_to_string_or",
    "runtime:Json.bool_at",
    "runtime:Json.bool_at_or",
    "runtime:Json.bool_field",
    "runtime:Json.clone",
    "runtime:Json.field",
    "runtime:Json.field_bool",
    "runtime:Json.field_int",
    "runtime:Json.field_optional",
    "runtime:Json.field_optional_bool",
    "runtime:Json.field_optional_int",
    "runtime:Json.field_optional_string",
    "runtime:Json.field_string",
    "runtime:Json.is_array",
    "runtime:Json.is_null",
    "runtime:Json.is_object",
    "runtime:Json.json_parse",
    "runtime:Json.json_bool_at_or",
    "runtime:Json.json_int_at_or",
    "runtime:Json.json_string_at_or",
    "runtime:Json.kind",
    "runtime:Json.int_at",
    "runtime:Json.int_at_or",
    "runtime:Json.int_field",
    "runtime:Json.object",
    "runtime:Json.object_keys",
    "runtime:Json.object_len",
    "runtime:Json.parse",
    "runtime:Json.parse_file",
    "runtime:Json.quote_string",
    "runtime:Json.raw_field",
    "runtime:Json.string_array",
    "runtime:Json.string_at",
    "runtime:Json.string_at_or",
    "runtime:Json.strings",
    "runtime:Json.string_field",
    "runtime:Json.to_string",
    "runtime:Json.to_string_at",
    "runtime:Json.to_string_at_or",
    "runtime:Json.value",
    "runtime:Json.value_at",
    "runtime:Json.values",
    "runtime:JsonError.message",
    "runtime:Instant.elapsed",
    "runtime:Hash.sha256_bytes",
    "runtime:Hash.sha256_file",
    "runtime:Hash.sha256_string",
    "runtime:Http.get",
    "runtime:Http.post_form",
    "runtime:Http.post_json",
    "runtime:HttpError.message",
    "runtime:HttpRequest.json",
    "runtime:HttpRequest.with_header",
    "runtime:HttpRequest.with_retry",
    "runtime:HttpRequest.with_timeout",
    "runtime:ImageCache.len",
    "runtime:ImageCache.new",
    "runtime:List.append",
    "runtime:List.clear",
    "runtime:List.contains_value",
    "runtime:List.consume",
    "runtime:List.first",
    "runtime:List.get",
    "runtime:List.is_empty",
    "runtime:List.join",
    "runtime:List.last",
    "runtime:List.len",
    "runtime:List.new",
    "runtime:List.pop",
    "runtime:List.push",
    "runtime:List.reverse",
    "runtime:List.remove_at",
    "runtime:List.set",
    "runtime:List.skip",
    "runtime:List.slice",
    "runtime:List.sort",
    "runtime:List.take",
    "runtime:List.to_json_strings",
    "runtime:List.to_json_values",
    "runtime:Log.error",
    "runtime:Log.error_json",
    "runtime:Log.trace",
    "runtime:Log.write",
    "runtime:Log.write_json",
    "runtime:Map.clear",
    "runtime:Map.contains_key",
    "runtime:Map.get",
    "runtime:Map.get_or_default",
    "runtime:Map.insert",
    "runtime:Map.insert_old",
    "runtime:Map.is_empty",
    "runtime:Map.keys",
    "runtime:Map.len",
    "runtime:Map.new",
    "runtime:Map.remove",
    "runtime:Map.values",
    "runtime:PersistentMap.clear",
    "runtime:PersistentMap.contains_key",
    "runtime:PersistentMap.get",
    "runtime:PersistentMap.insert",
    "runtime:PersistentMap.is_empty",
    "runtime:PersistentMap.len",
    "runtime:PersistentMap.new",
    "runtime:PersistentMap.remove",
    "runtime:Regex.captures",
    "runtime:Regex.compile",
    "runtime:Regex.find",
    "runtime:Regex.is_match",
    "runtime:Regex.replace_all",
    "runtime:Regex.split",
    "runtime:RegexError.message",
    "runtime:Request.new",
    "runtime:Request.path",
    "runtime:Response.body",
    "runtime:Response.ok",
    "runtime:Response.status",
    "runtime:Option.is_none",
    "runtime:Option.is_some",
    "runtime:Option.ok_or",
    "runtime:Option.or",
    "runtime:Option.unwrap_or",
    "runtime:Ord.compare",
    "runtime:Path.extension",
    "runtime:Path.file_name",
    "runtime:Path.from_string",
    "runtime:Path.is_absolute",
    "runtime:Path.exists",
    "runtime:Path.is_dir",
    "runtime:Path.is_file",
    "runtime:Path.join",
    "runtime:Path.list_files",
    "runtime:Path.list_paths",
    "runtime:Path.normalize",
    "runtime:Path.parent",
    "runtime:Path.read_string",
    "runtime:Path.resolve_relative",
    "runtime:Path.safe_relative",
    "runtime:Path.starts_with",
    "runtime:Path.to_string",
    "runtime:Path.with_extension",
    "runtime:Path.write_string",
    "runtime:Patch.apply_text",
    "runtime:Result.err",
    "runtime:Result.err_message",
    "runtime:Result.is_err",
    "runtime:Result.is_ok",
    "runtime:Result.ok",
    "runtime:Result.unwrap_or",
    "runtime:Row.field_string",
    "runtime:RowBuffer.new",
    "runtime:RuleLoader.load_rules",
    "runtime:Set.clear",
    "runtime:Set.contains",
    "runtime:Set.difference",
    "runtime:Set.insert",
    "runtime:Set.intersection",
    "runtime:Set.is_empty",
    "runtime:Set.is_subset",
    "runtime:Set.len",
    "runtime:Set.new",
    "runtime:Set.remove",
    "runtime:Set.to_list",
    "runtime:Set.union",
    "runtime:SortedSet.clear",
    "runtime:SortedSet.contains",
    "runtime:SortedSet.insert",
    "runtime:SortedSet.is_empty",
    "runtime:SortedSet.len",
    "runtime:SortedSet.new",
    "runtime:SortedSet.remove",
    "runtime:SortedSet.to_list",
    "runtime:SortedMap.clear",
    "runtime:SortedMap.contains_key",
    "runtime:SortedMap.get",
    "runtime:SortedMap.insert",
    "runtime:SortedMap.is_empty",
    "runtime:SortedMap.keys",
    "runtime:SortedMap.len",
    "runtime:SortedMap.new",
    "runtime:SortedMap.remove",
    "runtime:SortedMap.values",
    "runtime:String.after",
    "runtime:String.before",
    "runtime:String.chars",
    "runtime:String.concat",
    "runtime:String.contains",
    "runtime:String.copy",
    "runtime:String.ends_with",
    "runtime:String.env",
    "runtime:String.env_or",
    "runtime:String.from_bool",
    "runtime:String.from_int",
    "runtime:String.index_of",
    "runtime:String.is_empty",
    "runtime:String.len",
    "runtime:String.lines",
    "runtime:String.parse_int",
    "runtime:String.repeat",
    "runtime:String.replace",
    "runtime:String.slice",
    "runtime:String.split",
    "runtime:String.starts_with",
    "runtime:String.strip_prefix",
    "runtime:String.safe_relative",
    "runtime:String.join",
    "runtime:String.to_bytes",
    "runtime:String.to_path",
    "runtime:String.to_url",
    "runtime:String.to_lowercase",
    "runtime:String.to_uppercase",
    "runtime:String.trim",
    "runtime:String.view",
    "runtime:StringView.after",
    "runtime:StringView.before",
    "runtime:StringView.contains",
    "runtime:StringView.is_empty",
    "runtime:StringView.len",
    "runtime:StringView.slice",
    "runtime:StringView.starts_with",
    "runtime:StringView.to_string",
    "runtime:StringBuilder.finish",
    "runtime:StringBuilder.new",
    "runtime:StringBuilder.push",
    "runtime:GlobalConfig.new",
    "runtime:GlobalConfig.replace",
    "runtime:GlobalConfig.rule_count",
    "runtime:TempDir.new",
    "runtime:TempDir.new_in",
    "runtime:TempDir.path",
    "runtime:Toml.parse_file",
    "runtime:Url.from_string",
    "runtime:Url.to_string",
    "runtime:Workspace.resolve",
    "runtime:Yaml.parse",
    "runtime:Yaml.parse_file",
    "value:Bool",
    "value:Bytes",
    "value:Char",
    "value:Int",
    "value:Json",
    "value:List",
    "value:Map",
    "value:String",
    "value:Struct",
    "value:Unit",
    "value:Variant",
];

pub fn interpreter_coverage_report() -> InterpreterCoverageReport {
    let runtime_all = runtime_abi::runtime_intrinsic_signatures();
    let mut runtime_supported = runtime_abi::runtime_intrinsic_signatures_with_interpreter()
        .into_iter()
        .collect::<BTreeSet<_>>();
    runtime_supported.extend(
        INTERPRETER_HANDWRITTEN_INTRINSICS
            .iter()
            .map(|(namespace, name)| format!("{namespace}.{name}")),
    );

    let runtime_intrinsics = coverage_bucket_from_owned(runtime_all, runtime_supported);
    let hir_statements =
        coverage_bucket(HIR_STMT_VARIANTS, INTERPRETER_SUPPORTED_HIR_STMT_VARIANTS);
    let hir_expressions =
        coverage_bucket(HIR_EXPR_VARIANTS, INTERPRETER_SUPPORTED_HIR_EXPR_VARIANTS);
    let value_types = coverage_bucket(VALUE_TYPES, INTERPRETER_SUPPORTED_VALUE_TYPES);
    let function_kinds = coverage_bucket(FUNCTION_KINDS, INTERPRETER_SUPPORTED_FUNCTION_KINDS);
    let required_parity_features = required_parity_features(
        &runtime_intrinsics,
        &hir_statements,
        &hir_expressions,
        &value_types,
        &function_kinds,
    );

    InterpreterCoverageReport {
        parity_features: coverage_bucket_from_owned(
            required_parity_features,
            INTERPRETER_PARITY_FEATURES
                .iter()
                .map(|feature| (*feature).to_string())
                .collect(),
        ),
        runtime_intrinsics,
        hir_statements,
        hir_expressions,
        value_types,
        function_kinds,
    }
}

fn required_parity_features(
    runtime_intrinsics: &CoverageBucket,
    hir_statements: &CoverageBucket,
    hir_expressions: &CoverageBucket,
    value_types: &CoverageBucket,
    function_kinds: &CoverageBucket,
) -> Vec<String> {
    let mut features = Vec::new();
    features.extend(
        runtime_intrinsics
            .supported
            .iter()
            .map(|item| format!("runtime:{item}")),
    );
    features.extend(
        hir_statements
            .supported
            .iter()
            .map(|item| format!("hir_stmt:{item}")),
    );
    features.extend(
        hir_expressions
            .supported
            .iter()
            .map(|item| format!("hir_expr:{item}")),
    );
    features.extend(
        value_types
            .supported
            .iter()
            .map(|item| format!("value:{item}")),
    );
    features.extend(
        function_kinds
            .supported
            .iter()
            .map(|item| format!("function:{item}")),
    );
    features
}

fn coverage_bucket(all: &[&str], supported: &[&str]) -> CoverageBucket {
    coverage_bucket_from_owned(
        all.iter().map(|item| (*item).to_string()).collect(),
        supported.iter().map(|item| (*item).to_string()).collect(),
    )
}

fn coverage_bucket_from_owned(mut all: Vec<String>, supported: BTreeSet<String>) -> CoverageBucket {
    all.sort();
    all.dedup();
    let all_set = all.iter().cloned().collect::<BTreeSet<_>>();
    let mut supported = supported
        .into_iter()
        .filter(|item| all_set.contains(item))
        .collect::<Vec<_>>();
    supported.sort();
    let supported_set = supported.iter().cloned().collect::<BTreeSet<_>>();
    let missing = all
        .iter()
        .filter(|item| !supported_set.contains(*item))
        .cloned()
        .collect();
    CoverageBucket {
        all,
        supported,
        missing,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Unit,
    Int(i64),
    Bool(bool),
    String(String),
    Char(char),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Json(serde_json::Value),
    Struct {
        name: String,
        fields: BTreeMap<String, Value>,
    },
    Variant {
        name: String,
        fields: BTreeMap<String, Value>,
    },
}

impl Value {
    fn display(&self) -> String {
        match self {
            Self::Unit => "Unit".to_string(),
            Self::Int(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::String(value) => value.clone(),
            Self::Char(value) => value.to_string(),
            Self::Bytes(value) => format!("{value:?}"),
            Self::List(items) => {
                let items = items
                    .iter()
                    .map(Value::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{items}]")
            }
            Self::Map(entries) => {
                let entries = entries
                    .iter()
                    .map(|(key, value)| format!("{}: {}", key.display(), value.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{entries}}}")
            }
            Self::Json(value) => {
                serde_json::to_string(value).unwrap_or_else(|_| "<json>".to_string())
            }
            Self::Struct { name, fields } | Self::Variant { name, fields } => {
                if fields.is_empty() {
                    return name.clone();
                }
                let fields = fields
                    .iter()
                    .map(|(field, value)| format!("{field}: {}", value.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name} {{ {fields} }}")
            }
        }
    }
}

enum Control {
    Continue,
    Return(Value),
    Break,
    LoopContinue,
}

pub fn eval_source_main(file: &str, source: &str) -> Result<EvalOutput, EvalError> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    eval_source_main_with_args(file, source, args)
}

pub fn eval_source_main_with_args(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    let interfaces = standard_package_interfaces().collect::<Vec<_>>();
    let diagnostics = analyze_source_with_interfaces(file, source, &interfaces);
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity.is_error())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(EvalError::Diagnostics(errors));
    }

    // The interpreter executes the same checked HIR the Rust backend lowers, so
    // both stay aligned on one semantic representation. The parsed AST is kept
    // only for the two places HIR does not retain what execution needs: struct
    // field declaration order and receiver-call receiver expressions.
    let program = parse_source(file, source);
    let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
    let mut interpreter =
        Interpreter::new(&hir, &program, args.into_iter().map(Into::into).collect());
    let value = interpreter.call_function("main", Vec::new())?;
    Ok(EvalOutput {
        value: value.display(),
        stdout: interpreter.stdout,
        stderr: interpreter.stderr,
    })
}

struct Interpreter<'a> {
    hir: &'a Hir,
    program: &'a Program,
    scopes: Vec<HashMap<String, Value>>,
    pending_return: Option<Value>,
    stdout: String,
    stderr: String,
    args: Vec<String>,
}

impl<'a> Interpreter<'a> {
    fn new(hir: &'a Hir, program: &'a Program, args: Vec<String>) -> Self {
        Self {
            hir,
            program,
            scopes: Vec::new(),
            pending_return: None,
            stdout: String::new(),
            stderr: String::new(),
            args,
        }
    }

    fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, EvalError> {
        let hir = self.hir;
        let Some(signature) = hir.resolve_function(None, name) else {
            return Err(EvalError::Runtime(format!(
                "interpreter cannot call unsupported function `{name}`."
            )));
        };
        let Some(body) = hir.function_body(name).and_then(|body| body.block.as_ref()) else {
            return Err(EvalError::Runtime(format!(
                "interpreter P0 does not execute async/native function `{name}`."
            )));
        };
        if signature.is_async {
            return Err(EvalError::Runtime(format!(
                "interpreter P0 does not execute async/native function `{name}`."
            )));
        }
        if args.len() != signature.params.len() {
            return Err(EvalError::Runtime(format!(
                "function `{name}` expected {} arguments, got {}.",
                signature.params.len(),
                args.len()
            )));
        }
        self.scopes.push(HashMap::new());
        for (param, value) in signature.params.iter().zip(args) {
            self.bind(param.name.clone(), value);
        }
        let result = match self.eval_block(body)? {
            Control::Return(value) => value,
            Control::Continue => Value::Unit,
            Control::Break | Control::LoopContinue => {
                self.scopes.pop();
                return Err(EvalError::Runtime(format!(
                    "function `{name}` ended with a loop control statement."
                )));
            }
        };
        self.scopes.pop();
        Ok(result)
    }

    fn eval_block(&mut self, block: &HirBlock) -> Result<Control, EvalError> {
        self.scopes.push(HashMap::new());
        for statement in &block.statements {
            match self.eval_stmt(statement)? {
                Control::Continue => {}
                control => {
                    self.scopes.pop();
                    return Ok(control);
                }
            }
        }
        self.scopes.pop();
        Ok(Control::Continue)
    }

    fn eval_stmt(&mut self, statement: &HirStmt) -> Result<Control, EvalError> {
        match statement {
            HirStmt::Let { name, value, .. } => {
                let value = match value {
                    Some(value) => self.eval_expr(value)?,
                    None => Value::Unit,
                };
                self.bind(name.clone(), value);
                if let Some(value) = self.pending_return.take() {
                    return Ok(Control::Return(value));
                }
                Ok(Control::Continue)
            }
            HirStmt::Assign { target, value, .. } => {
                let value = self.eval_expr(value)?;
                if let Some(value) = self.pending_return.take() {
                    return Ok(Control::Return(value));
                }
                match target {
                    HirExpr::Ident { name, .. } => {
                        self.assign(name, value)?;
                        Ok(Control::Continue)
                    }
                    HirExpr::Index { base, index, .. } => {
                        let HirExpr::Ident { name, .. } = base.as_ref() else {
                            return Err(EvalError::Runtime(
                                "interpreter P0 only supports index assignment rooted at local variables."
                                    .to_string(),
                            ));
                        };
                        let index = expect_int(self.eval_expr(index)?)?;
                        self.assign_list_index(name, index, value)?;
                        Ok(Control::Continue)
                    }
                    _ => Err(EvalError::Runtime(
                        "interpreter P0 only supports assignment to local variables.".to_string(),
                    )),
                }
            }
            HirStmt::Return { value, .. } => {
                let value = value
                    .as_ref()
                    .map(|value| self.eval_expr(value))
                    .transpose()?
                    .unwrap_or(Value::Unit);
                if let Some(value) = self.pending_return.take() {
                    return Ok(Control::Return(value));
                }
                Ok(Control::Return(value))
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                let resource = self.eval_expr(resource)?;
                if let Some(value) = self.pending_return.take() {
                    return Ok(Control::Return(value));
                }
                self.scopes
                    .push(HashMap::from([(binding.clone(), resource)]));
                let control = self.eval_block(body)?;
                self.scopes.pop();
                Ok(control)
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                if expect_bool(self.eval_expr(condition)?)? {
                    self.eval_block(then_body)
                } else if let Some(else_body) = else_body {
                    self.eval_block(else_body)
                } else {
                    Ok(Control::Continue)
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                loop {
                    if let Some(condition) = condition
                        && !expect_bool(self.eval_expr(condition)?)?
                    {
                        break;
                    }
                    match self.eval_block(body)? {
                        Control::Continue | Control::LoopContinue => {}
                        Control::Break => break,
                        control @ Control::Return(_) => return Ok(control),
                    }
                }
                Ok(Control::Continue)
            }
            HirStmt::For {
                binding,
                iterable,
                is_async,
                body,
                ..
            } => {
                if *is_async {
                    return Err(EvalError::Runtime(
                        "interpreter P0 does not support async for.".to_string(),
                    ));
                }
                let Value::List(items) = self.eval_expr(iterable)? else {
                    return Err(EvalError::Runtime(
                        "for loop expects a List value.".to_string(),
                    ));
                };
                for item in items {
                    self.scopes.push(HashMap::from([(binding.clone(), item)]));
                    let control = self.eval_block(body)?;
                    self.scopes.pop();
                    match control {
                        Control::Continue | Control::LoopContinue => {}
                        Control::Break => break,
                        control @ Control::Return(_) => return Ok(control),
                    }
                }
                Ok(Control::Continue)
            }
            HirStmt::Match { value, arms, .. } => {
                let value = self.eval_expr(value)?;
                self.eval_match(value, arms)
            }
            HirStmt::Break(_) => Ok(Control::Break),
            HirStmt::Continue(_) => Ok(Control::LoopContinue),
            HirStmt::Expr(expr) => {
                self.eval_expr(expr)?;
                if let Some(value) = self.pending_return.take() {
                    return Ok(Control::Return(value));
                }
                Ok(Control::Continue)
            }
            unsupported => Err(EvalError::Runtime(format!(
                "interpreter P0 does not support statement `{}`.",
                hir_stmt_name(unsupported)
            ))),
        }
    }

    fn eval_expr(&mut self, expr: &HirExpr) -> Result<Value, EvalError> {
        match expr {
            HirExpr::Ident { name, .. } => self.eval_ident(name),
            HirExpr::Number { value, .. } => value
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|error| EvalError::Runtime(format!("invalid integer `{value}`: {error}"))),
            HirExpr::String { value, .. } => Ok(Value::String(decode_string_token(value))),
            HirExpr::ObjectLiteral { fields, .. } => {
                let mut object = serde_json::Map::new();
                for field in fields {
                    let value = self.eval_expr(&field.value)?;
                    object.insert(field.name.clone(), value_to_json_literal(value)?);
                }
                Ok(Value::Json(serde_json::Value::Object(object)))
            }
            HirExpr::MapLiteral { entries, .. } => {
                let entries = entries
                    .iter()
                    .map(|entry| Ok((self.eval_expr(&entry.key)?, self.eval_expr(&entry.value)?)))
                    .collect::<Result<Vec<_>, EvalError>>()?;
                Ok(Value::Map(entries))
            }
            HirExpr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.eval_expr(item))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::List(items))
            }
            HirExpr::Binary {
                op, left, right, ..
            } => {
                let left = self.eval_expr(left)?;
                let right = self.eval_expr(right)?;
                eval_binary(*op, left, right)
            }
            HirExpr::Field { base, name, .. } => {
                let base = self.eval_expr(base)?;
                read_field(&base, name)
            }
            HirExpr::Index { base, index, .. } => {
                let base = self.eval_expr(base)?;
                let index = self.eval_expr(index)?;
                eval_index(base, index)
            }
            HirExpr::Call { callee, args, .. } => self.eval_call(callee, args),
            HirExpr::Effect { value, .. } | HirExpr::Manage { value, .. } => self.eval_expr(value),
            HirExpr::Try { value, .. } => {
                let value = self.eval_expr(value)?;
                match value {
                    Value::Variant { name, fields } if name == "Ok" => fields
                        .get("value")
                        .cloned()
                        .ok_or_else(|| EvalError::Runtime("Ok value is missing.".to_string())),
                    Value::Variant { name, fields } if name == "Err" => {
                        let error = fields.get("value").cloned().ok_or_else(|| {
                            EvalError::Runtime("Err value is missing.".to_string())
                        })?;
                        self.pending_return = Some(value_err(error));
                        Ok(Value::Unit)
                    }
                    other => Err(EvalError::Runtime(format!(
                        "? expects a Result value, got `{}`.",
                        other.display()
                    ))),
                }
            }
            HirExpr::Match { value, arms, .. } => {
                let value = self.eval_expr(value)?;
                match self.eval_match(value, arms)? {
                    Control::Return(value) => Ok(value),
                    Control::Continue => Ok(Value::Unit),
                    Control::Break | Control::LoopContinue => Err(EvalError::Runtime(
                        "loop control cannot escape a match expression.".to_string(),
                    )),
                }
            }
            unsupported => Err(EvalError::Runtime(format!(
                "interpreter P0 does not support expression `{}`.",
                hir_expr_name(unsupported)
            ))),
        }
    }

    fn eval_call(&mut self, callee: &Callee, args: &[HirCallArg]) -> Result<Value, EvalError> {
        match callee {
            Callee::Name(name) => {
                if self.hir.function_body(name).is_some() {
                    let values = args
                        .iter()
                        .map(|arg| self.eval_expr(&arg.value))
                        .collect::<Result<Vec<_>, _>>()?;
                    return self.call_function(name, values);
                }
                if let Some(value) = self.construct_value(name, args)? {
                    return Ok(value);
                }
                Err(EvalError::Runtime(format!(
                    "interpreter cannot resolve call `{name}`."
                )))
            }
            Callee::Qualified { namespace, name } => {
                let namespace = strip_generic_args(namespace);
                let name = strip_generic_args(name);
                if let Some(intrinsic) = runtime_abi::lookup_runtime_intrinsic(namespace, name) {
                    return self.eval_runtime_intrinsic(
                        namespace,
                        name,
                        intrinsic.interpreter,
                        args,
                    );
                }
                Err(EvalError::Runtime(format!(
                    "interpreter P0 does not support qualified call `{namespace}.{name}`."
                )))
            }
            Callee::ReceiverCall {
                receiver, method, ..
            } => {
                // HIR reuses the AST `Callee`, so the receiver is still an AST
                // expression (only the call arguments are lowered). Evaluate it
                // through the small AST bridge below.
                let receiver = self.eval_ast_expr(receiver)?;
                self.eval_receiver_call(receiver, method, args)
            }
        }
    }

    fn eval_runtime_intrinsic(
        &mut self,
        namespace: &str,
        name: &str,
        intrinsic: Option<InterpreterIntrinsic>,
        args: &[HirCallArg],
    ) -> Result<Value, EvalError> {
        match intrinsic {
            Some(intrinsic) => eval_generated_runtime_intrinsic(self, intrinsic, args),
            None => self.eval_interpreter_core_intrinsic(namespace, name, args),
        }
    }

    fn eval_interpreter_core_intrinsic(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[HirCallArg],
    ) -> Result<Value, EvalError> {
        match (namespace, name) {
            ("Args", "all") => Ok(Value::List(
                self.args.iter().cloned().map(Value::String).collect(),
            )),
            ("Args", "count") => Ok(Value::Int(self.args.len() as i64)),
            ("Args", "get") => {
                let index = self.eval_first_arg(args)?;
                let index = expect_int(index)?;
                let value = if index < 0 {
                    None
                } else {
                    self.args.get(index as usize).cloned()
                };
                Ok(value_option(value, Value::String))
            }
            ("Args", "get_or_default") => {
                let index = self.eval_named_or_positional_arg(args, "index", 0)?;
                let default = self.eval_named_or_positional_arg(args, "default", 1)?;
                let index = expect_int(index)?;
                let default = expect_string(default)?;
                Ok(Value::String(if index < 0 {
                    default
                } else {
                    self.args.get(index as usize).cloned().unwrap_or(default)
                }))
            }
            ("Assert", "equal") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                let left = expect_string(left)?;
                let right = expect_string(right)?;
                if left != right {
                    return Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` did not equal right `{right}`"
                    )));
                }
                Ok(Value::Unit)
            }
            ("Assert", "equal_int") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                let left = expect_int(left)?;
                let right = expect_int(right)?;
                if left != right {
                    return Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` did not equal right `{right}`"
                    )));
                }
                Ok(Value::Unit)
            }
            ("Assert", "equal_bool") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                let left = expect_bool(left)?;
                let right = expect_bool(right)?;
                if left != right {
                    return Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` did not equal right `{right}`"
                    )));
                }
                Ok(Value::Unit)
            }
            ("Deque", "new") => Ok(Value::List(Vec::new())),
            ("Deque", "push_back") => {
                let deque_name = self.mut_arg_local_name(args, "deque", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let mut deque = expect_list(self.lookup(deque_name)?)?;
                deque.push(value);
                self.assign(deque_name, Value::List(deque))?;
                Ok(Value::Unit)
            }
            ("Deque", "push_front") => {
                let deque_name = self.mut_arg_local_name(args, "deque", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let mut deque = expect_list(self.lookup(deque_name)?)?;
                deque.insert(0, value);
                self.assign(deque_name, Value::List(deque))?;
                Ok(Value::Unit)
            }
            ("Deque", "pop_back") => {
                let deque_name = self.mut_arg_local_name(args, "deque", 0)?;
                let mut deque = expect_list(self.lookup(deque_name)?)?;
                let value = deque.pop();
                self.assign(deque_name, Value::List(deque))?;
                Ok(value_option(value, |value| value))
            }
            ("Deque", "pop_front") => {
                let deque_name = self.mut_arg_local_name(args, "deque", 0)?;
                let mut deque = expect_list(self.lookup(deque_name)?)?;
                let value = if deque.is_empty() {
                    None
                } else {
                    Some(deque.remove(0))
                };
                self.assign(deque_name, Value::List(deque))?;
                Ok(value_option(value, |value| value))
            }
            ("Deque", "clear") => {
                let deque_name = self.mut_arg_local_name(args, "deque", 0)?;
                self.assign(deque_name, Value::List(Vec::new()))?;
                Ok(Value::Unit)
            }
            ("Deque", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_list(value)?.len() as i64))
            }
            ("Deque", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_list(value)?.is_empty()))
            }
            ("Deque", "to_list") => self.eval_first_arg(args),
            ("Directory", "exists") | ("Path", "exists") | ("File", "exists") => {
                let path = self.eval_first_arg(args)?;
                Ok(Value::Bool(Path::new(&expect_string(path)?).exists()))
            }
            ("Directory", "is_file") | ("Path", "is_file") => {
                let path = self.eval_first_arg(args)?;
                Ok(Value::Bool(Path::new(&expect_string(path)?).is_file()))
            }
            ("Directory", "is_dir") | ("Path", "is_dir") => {
                let path = self.eval_first_arg(args)?;
                Ok(Value::Bool(Path::new(&expect_string(path)?).is_dir()))
            }
            ("Directory", "create_all") | ("Directory", "create_dir_all") => {
                let path = self.eval_first_arg(args)?;
                Ok(file_result_unit(std::fs::create_dir_all(expect_string(
                    path,
                )?)))
            }
            ("Directory", "create") => {
                let path = self.eval_first_arg(args)?;
                Ok(file_result_unit(std::fs::create_dir(expect_string(path)?)))
            }
            ("Directory", "list_files") | ("Path", "list_files") => {
                let path = self.eval_first_arg(args)?;
                Ok(result_value(
                    directory_list_files(&PathBuf::from(expect_string(path)?))
                        .map(|files| Value::List(files.into_iter().map(Value::String).collect()))
                        .map_err(file_error),
                ))
            }
            ("Directory", "list_paths") | ("Path", "list_paths") => {
                let path = self.eval_first_arg(args)?;
                Ok(result_value(
                    directory_list_paths(&PathBuf::from(expect_string(path)?))
                        .map(|paths| {
                            Value::List(
                                paths
                                    .into_iter()
                                    .map(|path| Value::String(path.to_string_lossy().to_string()))
                                    .collect(),
                            )
                        })
                        .map_err(file_error),
                ))
            }
            ("Directory", "metadata") => {
                let path = self.eval_first_arg(args)?;
                Ok(result_value(
                    std::fs::metadata(expect_string(path)?)
                        .map(|metadata| Value::Struct {
                            name: "FileMetadata".to_string(),
                            fields: BTreeMap::from([
                                ("is_file".to_string(), Value::Bool(metadata.is_file())),
                                ("is_dir".to_string(), Value::Bool(metadata.is_dir())),
                                ("len".to_string(), Value::Int(metadata.len() as i64)),
                            ]),
                        })
                        .map_err(file_error),
                ))
            }
            ("Directory", "read_string") | ("Path", "read_string") | ("File", "read_string") => {
                let path = self.eval_first_arg(args)?;
                Ok(result_value(
                    std::fs::read_to_string(expect_string(path)?)
                        .map(Value::String)
                        .map_err(file_error),
                ))
            }
            ("File", "open") | ("File", "open_read") => {
                let path = self.eval_first_arg(args)?;
                let path = expect_string(path)?;
                Ok(result_value(
                    std::fs::File::open(&path)
                        .map(|_| file_value(path, "read", 0))
                        .map_err(file_error),
                ))
            }
            ("File", "open_write") => {
                let path = self.eval_first_arg(args)?;
                let path = expect_string(path)?;
                Ok(result_value(
                    std::fs::File::create(&path)
                        .map(|_| file_value(path, "write", 0))
                        .map_err(file_error),
                ))
            }
            ("File", "read_all") => {
                let file_name = self.mut_arg_local_name(args, "file", 0)?;
                let mut file = expect_file(self.lookup(file_name)?)?;
                let result = file_read_remaining(&mut file)
                    .map(Value::Bytes)
                    .map_err(file_error);
                self.assign(file_name, file.to_value())?;
                Ok(result_value(result))
            }
            ("File", "read_all_string") => {
                let file_name = self.mut_arg_local_name(args, "file", 0)?;
                let mut file = expect_file(self.lookup(file_name)?)?;
                let result = file_read_remaining(&mut file)
                    .and_then(|bytes| {
                        String::from_utf8(bytes).map_err(|error| {
                            std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                        })
                    })
                    .map(Value::String)
                    .map_err(file_error);
                self.assign(file_name, file.to_value())?;
                Ok(result_value(result))
            }
            ("File", "read_into") => {
                let file_name = self.mut_arg_local_name(args, "file", 0)?;
                let buffer_name = self.mut_arg_local_name(args, "buffer", 1)?;
                let mut file = expect_file(self.lookup(file_name)?)?;
                let result = file_read_remaining(&mut file)
                    .map(|bytes| {
                        let did_read = !bytes.is_empty();
                        (Value::Bytes(bytes), Value::Bool(did_read))
                    })
                    .map_err(file_error);
                self.assign(file_name, file.to_value())?;
                Ok(match result {
                    Ok((buffer, did_read)) => {
                        self.assign(buffer_name, buffer)?;
                        value_ok(did_read)
                    }
                    Err(error) => value_err(error),
                })
            }
            ("File", "read_bytes") => {
                let path = self.eval_first_arg(args)?;
                Ok(result_value(
                    std::fs::read(expect_string(path)?)
                        .map(Value::Bytes)
                        .map_err(file_error),
                ))
            }
            ("File", "write_bytes") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let data = self.eval_named_or_positional_arg(args, "data", 1)?;
                Ok(file_result_unit(std::fs::write(
                    expect_string(path)?,
                    expect_bytes(data)?,
                )))
            }
            ("File", "append_bytes") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let data = self.eval_named_or_positional_arg(args, "data", 1)?;
                Ok(file_append_result(
                    expect_string(path)?,
                    &expect_bytes(data)?,
                ))
            }
            ("File", "append_string") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let text = self.eval_named_or_positional_arg(args, "text", 1)?;
                Ok(file_append_result(
                    expect_string(path)?,
                    expect_string(text)?.as_bytes(),
                ))
            }
            ("File", "write")
            | ("File", "write_bytes_view")
            | ("File", "write_buffer")
            | ("File", "write_buffer_view") => {
                let file_name = self.mut_arg_local_name(args, "file", 0)?;
                let data = self
                    .eval_named_or_positional_arg(args, "data", 1)
                    .or_else(|_| self.eval_named_or_positional_arg(args, "buffer", 1))?;
                let mut file = expect_file(self.lookup(file_name)?)?;
                let result = file_write_at_cursor(&mut file, &expect_bytes(data)?);
                self.assign(file_name, file.to_value())?;
                Ok(file_result_unit(result))
            }
            ("File", "write_string") => {
                let file_name = self.mut_arg_local_name(args, "file", 0)?;
                let text = self.eval_named_or_positional_arg(args, "text", 1)?;
                let mut file = expect_file(self.lookup(file_name)?)?;
                let result = file_write_at_cursor(&mut file, expect_string(text)?.as_bytes());
                self.assign(file_name, file.to_value())?;
                Ok(file_result_unit(result))
            }
            ("Directory", "remove_file") => {
                let path = self.eval_first_arg(args)?;
                Ok(file_result_unit(std::fs::remove_file(expect_string(path)?)))
            }
            ("File", "remove") => {
                let path = self.eval_first_arg(args)?;
                Ok(file_result_unit(std::fs::remove_file(expect_string(path)?)))
            }
            ("Directory", "remove_dir_all") => {
                let path = self.eval_first_arg(args)?;
                Ok(file_result_unit(std::fs::remove_dir_all(expect_string(
                    path,
                )?)))
            }
            ("Directory", "copy_file") => {
                let from = self.eval_named_or_positional_arg(args, "from", 0)?;
                let to = self.eval_named_or_positional_arg(args, "to", 1)?;
                Ok(file_result_unit(
                    std::fs::copy(expect_string(from)?, expect_string(to)?).map(|_| ()),
                ))
            }
            ("Directory", "rename") => {
                let from = self.eval_named_or_positional_arg(args, "from", 0)?;
                let to = self.eval_named_or_positional_arg(args, "to", 1)?;
                Ok(file_result_unit(std::fs::rename(
                    expect_string(from)?,
                    expect_string(to)?,
                )))
            }
            ("Directory", "write_string") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let content = self.eval_named_or_positional_arg(args, "content", 1)?;
                Ok(file_result_unit(std::fs::write(
                    expect_string(path)?,
                    expect_string(content)?,
                )))
            }
            ("Path", "write_string") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let text = self.eval_named_or_positional_arg(args, "text", 1)?;
                Ok(file_result_unit(std::fs::write(
                    expect_string(path)?,
                    expect_string(text)?,
                )))
            }
            ("File", "write_string_to_path") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let text = self.eval_named_or_positional_arg(args, "text", 1)?;
                Ok(file_result_unit(std::fs::write(
                    expect_string(path)?,
                    expect_string(text)?,
                )))
            }
            ("File", "write_atomic") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let text = self.eval_named_or_positional_arg(args, "text", 1)?;
                Ok(file_atomic_write_result(
                    PathBuf::from(expect_string(path)?),
                    &expect_string(text)?,
                ))
            }
            ("FileError", "message") => {
                let value = self.eval_first_arg(args)?;
                read_field(&value, "message")
            }
            ("RowBuffer", "new") => {
                let size = self.eval_named_or_positional_arg(args, "size", 0)?;
                let capacity = expect_int(size)?.max(0) as usize;
                Ok(row_buffer_value(Vec::with_capacity(capacity)))
            }
            ("Csv", "open_read") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let path = expect_string(path)?;
                Ok(result_value(
                    std::fs::File::open(&path)
                        .map(|_| file_value(path, "read", 0))
                        .map_err(csv_error_from_io),
                ))
            }
            ("Csv", "read_into") => {
                let file_name = self.mut_arg_local_name(args, "file", 0)?;
                let buffer_name = self.mut_arg_local_name(args, "buffer", 1)?;
                let mut file = expect_file(self.lookup(file_name)?)?;
                let result = file_read_remaining(&mut file).map_err(csv_error_from_io);
                self.assign(file_name, file.to_value())?;
                Ok(match result {
                    Ok(bytes) => {
                        self.assign(buffer_name, row_buffer_value(bytes))?;
                        value_ok(Value::Unit)
                    }
                    Err(error) => value_err(error),
                })
            }
            ("Csv", "parse_row") => {
                let buffer = self.eval_named_or_positional_arg(args, "buffer", 0)?;
                Ok(result_value(csv_parse_row_value(&expect_row_buffer_bytes(
                    buffer,
                )?)))
            }
            ("Row", "field_string") => {
                let row = self.eval_named_or_positional_arg(args, "row", 0)?;
                let index = self.eval_named_or_positional_arg(args, "index", 1)?;
                Ok(result_value(row_field_string_value(
                    expect_row_fields(row)?,
                    expect_int(index)?,
                )))
            }
            ("Duration", "ms") => self.eval_first_arg(args),
            ("Duration", "seconds") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_int(value)? * 1000))
            }
            ("Duration", "as_ms") => self.eval_first_arg(args),
            ("Duration", "as_seconds") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_int(value)? / 1000))
            }
            ("Duration", "add") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                Ok(Value::Int(expect_int(left)? + expect_int(right)?))
            }
            ("Config", "load") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                Ok(result_value(
                    std::fs::read_to_string(expect_string(path)?)
                        .map(|text| config_value(config_name_from_text(&text)))
                        .map_err(config_error),
                ))
            }
            ("Config", "name") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                Ok(Value::String(expect_config_value_name(value)?))
            }
            ("ConfigStore", "new") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                Ok(config_store_value(expect_config_value_name(value)?))
            }
            ("ConfigStore", "replace") => {
                let store_name = self.mut_arg_local_name(args, "store", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                self.assign(
                    store_name,
                    config_store_value(expect_config_value_name(value)?),
                )?;
                Ok(Value::Unit)
            }
            ("ConfigStore", "name") => {
                let store = self.eval_named_or_positional_arg(args, "store", 0)?;
                Ok(Value::String(expect_config_store_name(store)?))
            }
            ("RuleLoader", "load_rules") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                Ok(result_value(
                    std::fs::read_to_string(expect_string(path)?)
                        .map(|text| Value::List(rules_from_text(&text)))
                        .map_err(config_error),
                ))
            }
            ("Config", "new") => {
                let name = self.eval_named_or_positional_arg(args, "name", 0)?;
                let rules = self.eval_named_or_positional_arg(args, "rules", 1)?;
                Ok(config_rules_value(
                    expect_string(name)?,
                    expect_list(rules)?.len() as i64,
                ))
            }
            ("Config", "rule_count") => {
                let config = self.eval_named_or_positional_arg(args, "config", 0)?;
                Ok(Value::Int(expect_config_rule_count(config)?))
            }
            ("GlobalConfig", "new") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                Ok(global_config_value(expect_config_rule_count(value)?))
            }
            ("GlobalConfig", "replace") => {
                let global_name = self.mut_arg_local_name(args, "global", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                self.assign(
                    global_name,
                    global_config_value(expect_config_rule_count(value)?),
                )?;
                Ok(Value::Unit)
            }
            ("GlobalConfig", "rule_count") => {
                let global = self.eval_named_or_positional_arg(args, "global", 0)?;
                Ok(Value::Int(expect_global_config_rule_count(global)?))
            }
            ("Ord", "compare") => {
                let left = self.eval_named_or_positional_arg(args, "self", 0)?;
                let right = self.eval_named_or_positional_arg(args, "other", 1)?;
                Ok(Value::Int(match value_cmp(&left, &right)? {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                }))
            }
            ("Deadline", "after_ms") => {
                let ms = self.eval_named_or_positional_arg(args, "ms", 0)?;
                Ok(deadline_value(deadline_after_ms(expect_int(ms)?)))
            }
            ("Deadline", "after") => {
                let duration = self.eval_named_or_positional_arg(args, "duration", 0)?;
                Ok(deadline_value(deadline_after_ms(expect_int(duration)?)))
            }
            ("Deadline", "is_expired") => {
                let deadline = self.eval_named_or_positional_arg(args, "deadline", 0)?;
                Ok(Value::Bool(
                    clock_system_unix_ms() >= expect_deadline_unix_ms(deadline)?,
                ))
            }
            ("Deadline", "remaining_ms") => {
                let deadline = self.eval_named_or_positional_arg(args, "deadline", 0)?;
                Ok(Value::Int(
                    expect_deadline_unix_ms(deadline)?
                        .saturating_sub(clock_system_unix_ms())
                        .max(0),
                ))
            }
            ("Counter", "new") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                Ok(counter_value(expect_int(value)?))
            }
            ("Counter", "add") => {
                let counter_name = self.mut_arg_local_name(args, "counter", 0)?;
                let amount = self.eval_named_or_positional_arg(args, "amount", 1)?;
                let value = expect_counter_value(self.lookup(counter_name)?)? + expect_int(amount)?;
                self.assign(counter_name, counter_value(value))?;
                Ok(Value::Unit)
            }
            ("Counter", "value") => {
                let counter = self.eval_named_or_positional_arg(args, "counter", 0)?;
                Ok(Value::Int(expect_counter_value(counter)?))
            }
            ("Env", "current_dir") => Ok(result_value(
                std::env::current_dir()
                    .map(|path| Value::String(path_to_string(&path)))
                    .map_err(file_error),
            )),
            ("Env", "get") => {
                let name = self.eval_first_arg(args)?;
                Ok(value_option(
                    std::env::var(expect_string(name)?).ok(),
                    |value| Value::String(value),
                ))
            }
            ("Env", "get_or_default") => {
                let name = self.eval_named_or_positional_arg(args, "name", 0)?;
                let default = self.eval_named_or_positional_arg(args, "default", 1)?;
                Ok(Value::String(
                    std::env::var(expect_string(name)?).unwrap_or(expect_string(default)?),
                ))
            }
            ("Env", "home_dir") => Ok(value_option(
                std::env::var("HOME").ok().filter(|value| !value.is_empty()),
                Value::String,
            )),
            ("Env", "run_workspace_root") => Ok(Value::String(path_to_string(Path::new(env!(
                "CARGO_MANIFEST_DIR"
            ))))),
            ("Env", "set") => {
                let _name = self.eval_named_or_positional_arg(args, "name", 0)?;
                let _value = self.eval_named_or_positional_arg(args, "value", 1)?;
                self.stderr
                    .push_str("[rsscript] warning: Env.set is a no-op in the safe runtime\n");
                Ok(Value::Unit)
            }
            ("Env", "temp_dir") => Ok(Value::String(path_to_string(&std::env::temp_dir()))),
            ("Diff", "unified") => {
                let old = self.eval_named_or_positional_arg(args, "old", 0)?;
                let new = self.eval_named_or_positional_arg(args, "new", 1)?;
                Ok(Value::String(diff_unified_string(
                    &expect_string(old)?,
                    &expect_string(new)?,
                )))
            }
            ("Buffer", "new") => {
                let size = self.eval_first_arg(args)?;
                let capacity = expect_int(size)?.max(0) as usize;
                Ok(Value::Bytes(Vec::with_capacity(capacity)))
            }
            ("Buffer", "len") | ("BufferView", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_bytes(value)?.len() as i64))
            }
            ("Buffer", "is_empty") | ("BufferView", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_bytes(value)?.is_empty()))
            }
            ("Buffer", "clear") => {
                let buffer_name = self.mut_arg_local_name(args, "buffer", 0)?;
                self.assign(buffer_name, Value::Bytes(Vec::new()))?;
                Ok(Value::Unit)
            }
            ("Buffer", "view") | ("BufferView", "slice") => {
                let value = self
                    .eval_named_or_positional_arg(args, "value", 0)
                    .or_else(|_| self.eval_named_or_positional_arg(args, "buffer", 0))?;
                let start = self.eval_named_or_positional_arg(args, "start", 1)?;
                let len = self.eval_named_or_positional_arg(args, "len", 2)?;
                Ok(Value::Bytes(bytes_slice(
                    expect_bytes(value)?,
                    expect_int(start)?,
                    expect_int(len)?,
                )))
            }
            ("Buffer", "consume") => {
                self.eval_first_arg(args)?;
                Ok(Value::Unit)
            }
            ("BufferView", "to_bytes") | ("Bytes", "from_buffer") => self.eval_first_arg(args),
            ("Cache", "new") => Ok(Value::Map(Vec::new())),
            ("Cache", "insert") => {
                let cache_name = self.mut_arg_local_name(args, "cache", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let value = self.eval_named_or_positional_arg(args, "value", 2)?;
                let mut cache = expect_map(self.lookup(cache_name)?)?;
                map_insert(&mut cache, key, value);
                self.assign(cache_name, Value::Map(cache))?;
                Ok(Value::Unit)
            }
            ("Cache", "lookup") => {
                let cache = self.eval_named_or_positional_arg(args, "cache", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                Ok(match map_get(&expect_map(cache)?, &key) {
                    Some(value) => value,
                    None => Value::String(String::new()),
                })
            }
            ("Clock", "now") => Ok(instant_value(clock_system_unix_ms())),
            ("Clock", "system_unix_ms") => Ok(Value::Int(clock_system_unix_ms())),
            ("Instant", "elapsed") => {
                let start = self.eval_first_arg(args)?;
                Ok(Value::Int(
                    clock_system_unix_ms()
                        .saturating_sub(expect_instant_unix_ms(start)?)
                        .max(0),
                ))
            }
            ("List", "push") => {
                let list_name = self.mut_arg_local_name(args, "list", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let mut list = expect_list(self.lookup(list_name)?)?;
                list.push(value);
                self.assign(list_name, Value::List(list))?;
                Ok(Value::Unit)
            }
            ("List", "set") => {
                let list_name = self.mut_arg_local_name(args, "list", 0)?;
                let index = self.eval_named_or_positional_arg(args, "index", 1)?;
                let value = self.eval_named_or_positional_arg(args, "value", 2)?;
                self.assign_list_index(list_name, expect_int(index)?, value)?;
                Ok(Value::Unit)
            }
            ("List", "pop") => {
                let list_name = self.mut_arg_local_name(args, "list", 0)?;
                let mut list = expect_list(self.lookup(list_name)?)?;
                let value = list.pop();
                self.assign(list_name, Value::List(list))?;
                Ok(value_option(value, |value| value))
            }
            ("List", "clear") => {
                let list_name = self.mut_arg_local_name(args, "list", 0)?;
                self.assign(list_name, Value::List(Vec::new()))?;
                Ok(Value::Unit)
            }
            ("List", "append") => {
                let list_name = self.mut_arg_local_name(args, "list", 0)?;
                let values = self.eval_named_or_positional_arg(args, "values", 1)?;
                let mut list = expect_list(self.lookup(list_name)?)?;
                list.extend(expect_list(values)?);
                self.assign(list_name, Value::List(list))?;
                Ok(Value::Unit)
            }
            ("List", "remove_at") => {
                let list_name = self.mut_arg_local_name(args, "list", 0)?;
                let index = self.eval_named_or_positional_arg(args, "index", 1)?;
                let mut list = expect_list(self.lookup(list_name)?)?;
                let index = expect_int(index)?;
                let value = if index < 0 || index as usize >= list.len() {
                    None
                } else {
                    Some(list.remove(index as usize))
                };
                self.assign(list_name, Value::List(list))?;
                Ok(value_option(value, |value| value))
            }
            ("List", "sort") => {
                let list_name = self.mut_arg_local_name(args, "list", 0)?;
                let mut list = expect_list(self.lookup(list_name)?)?;
                sort_values(&mut list)?;
                self.assign(list_name, Value::List(list))?;
                Ok(Value::Unit)
            }
            ("List", "consume") => {
                self.eval_first_arg(args)?;
                Ok(Value::Unit)
            }
            ("Bytes", "from_string") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bytes(expect_string(value)?.into_bytes()))
            }
            ("Bytes", "consume") => {
                self.eval_first_arg(args)?;
                Ok(Value::Unit)
            }
            ("Bytes", "len") | ("BytesView", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_bytes(value)?.len() as i64))
            }
            ("Bytes", "is_empty") | ("BytesView", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_bytes(value)?.is_empty()))
            }
            ("Bytes", "concat") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                let mut bytes = expect_bytes(left)?;
                bytes.extend(expect_bytes(right)?);
                Ok(Value::Bytes(bytes))
            }
            ("Bytes", "slice") | ("Bytes", "view") | ("BytesView", "slice") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let start = self.eval_named_or_positional_arg(args, "start", 1)?;
                let len = self.eval_named_or_positional_arg(args, "len", 2)?;
                Ok(Value::Bytes(bytes_slice(
                    expect_bytes(value)?,
                    expect_int(start)?,
                    expect_int(len)?,
                )))
            }
            ("BytesView", "to_bytes") => self.eval_first_arg(args),
            ("BytesView", "starts_with") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let prefix = self.eval_named_or_positional_arg(args, "prefix", 1)?;
                Ok(Value::Bool(
                    expect_bytes(value)?.starts_with(&expect_bytes(prefix)?),
                ))
            }
            ("Hash", "sha256_string") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::String(sha256_digest(
                    expect_string(value)?.as_bytes(),
                )))
            }
            ("Hash", "sha256_bytes") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::String(sha256_digest(&expect_bytes(value)?)))
            }
            ("Hash", "sha256_file") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                Ok(result_value(
                    std::fs::read(expect_string(path)?)
                        .map(|bytes| Value::String(sha256_digest(&bytes)))
                        .map_err(file_error),
                ))
            }
            ("ImageCache", "new") => {
                let capacity = self.eval_named_or_positional_arg(args, "capacity", 0)?;
                Ok(image_cache_value(expect_int(capacity)?.max(0), 0))
            }
            ("ImageCache", "len") => {
                let cache = self.eval_named_or_positional_arg(args, "cache", 0)?;
                Ok(Value::Int(expect_image_cache_len(cache)?))
            }
            ("List", "new") => Ok(Value::List(Vec::new())),
            ("List", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_list(value)?.len() as i64))
            }
            ("List", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_list(value)?.is_empty()))
            }
            ("List", "get") => {
                let list = self.eval_named_or_positional_arg(args, "list", 0)?;
                let index = self.eval_named_or_positional_arg(args, "index", 1)?;
                list_get(expect_list(list)?, expect_int(index)?)
            }
            ("List", "first") => {
                let value = self.eval_first_arg(args)?;
                Ok(value_option(
                    expect_list(value)?.into_iter().next(),
                    |value| value,
                ))
            }
            ("List", "last") => {
                let value = self.eval_first_arg(args)?;
                Ok(value_option(
                    expect_list(value)?.into_iter().last(),
                    |value| value,
                ))
            }
            ("List", "contains_value") => {
                let list = self.eval_named_or_positional_arg(args, "list", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                Ok(Value::Bool(
                    expect_list(list)?.iter().any(|item| item == &value),
                ))
            }
            ("List", "join") => {
                let list = self.eval_named_or_positional_arg(args, "list", 0)?;
                let separator = self.eval_named_or_positional_arg(args, "separator", 1)?;
                let strings = expect_list(list)?
                    .into_iter()
                    .map(expect_string)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::String(strings.join(&expect_string(separator)?)))
            }
            ("List", "reverse") => {
                let list = self.eval_first_arg(args)?;
                let mut list = expect_list(list)?;
                list.reverse();
                Ok(Value::List(list))
            }
            ("List", "skip") => {
                let list = self.eval_named_or_positional_arg(args, "list", 0)?;
                let count = self.eval_named_or_positional_arg(args, "count", 1)?;
                let count = expect_int(count)?.max(0) as usize;
                Ok(Value::List(
                    expect_list(list)?.into_iter().skip(count).collect(),
                ))
            }
            ("List", "take") => {
                let list = self.eval_named_or_positional_arg(args, "list", 0)?;
                let count = self.eval_named_or_positional_arg(args, "count", 1)?;
                let count = expect_int(count)?.max(0) as usize;
                Ok(Value::List(
                    expect_list(list)?.into_iter().take(count).collect(),
                ))
            }
            ("List", "slice") => {
                let list = self.eval_named_or_positional_arg(args, "list", 0)?;
                let start = self.eval_named_or_positional_arg(args, "start", 1)?;
                let len = self.eval_named_or_positional_arg(args, "len", 2)?;
                Ok(Value::List(list_slice(
                    expect_list(list)?,
                    expect_int(start)?,
                    expect_int(len)?,
                )))
            }
            ("List", "to_json_strings") => {
                let list = self.eval_first_arg(args)?;
                let strings = expect_list(list)?
                    .into_iter()
                    .map(expect_string)
                    .map(|value| value.map(serde_json::Value::String))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::Json(serde_json::Value::Array(strings)))
            }
            ("List", "to_json_values") => {
                let list = self.eval_first_arg(args)?;
                let values = expect_list(list)?
                    .into_iter()
                    .map(expect_json)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::Json(serde_json::Value::Array(values)))
            }
            ("StringBuilder", "new") => Ok(Value::String(String::new())),
            ("StringBuilder", "push") => {
                let builder_name = self.mut_arg_local_name(args, "builder", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let mut builder = expect_string(self.lookup(builder_name)?)?;
                builder.push_str(&expect_string(value)?);
                self.assign(builder_name, Value::String(builder))?;
                Ok(Value::Unit)
            }
            ("StringBuilder", "finish") => self.eval_first_arg(args),
            ("Map", "new") => Ok(Value::Map(Vec::new())),
            ("Map", "insert") => {
                let map_name = self.mut_arg_local_name(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let value = self.eval_named_or_positional_arg(args, "value", 2)?;
                let mut map = expect_map(self.lookup(map_name)?)?;
                map_insert(&mut map, key, value);
                self.assign(map_name, Value::Map(map))?;
                Ok(Value::Unit)
            }
            ("Map", "insert_old") => {
                let map_name = self.mut_arg_local_name(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let value = self.eval_named_or_positional_arg(args, "value", 2)?;
                let mut map = expect_map(self.lookup(map_name)?)?;
                let old = map_insert(&mut map, key, value);
                self.assign(map_name, Value::Map(map))?;
                Ok(value_option(old, |value| value))
            }
            ("Map", "remove") => {
                let map_name = self.mut_arg_local_name(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let mut map = expect_map(self.lookup(map_name)?)?;
                let old = map_remove(&mut map, &key);
                self.assign(map_name, Value::Map(map))?;
                Ok(value_option(old, |value| value))
            }
            ("Map", "clear") => {
                let map_name = self.mut_arg_local_name(args, "map", 0)?;
                self.assign(map_name, Value::Map(Vec::new()))?;
                Ok(Value::Unit)
            }
            ("Map", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_map(value)?.len() as i64))
            }
            ("Map", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_map(value)?.is_empty()))
            }
            ("Map", "contains_key") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                Ok(Value::Bool(map_get(&expect_map(map)?, &key).is_some()))
            }
            ("Map", "get") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                Ok(value_option(map_get(&expect_map(map)?, &key), |value| {
                    value
                }))
            }
            ("Map", "get_or_default") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let default = self.eval_named_or_positional_arg(args, "default", 2)?;
                Ok(map_get(&expect_map(map)?, &key).unwrap_or(default))
            }
            ("Map", "keys") => {
                let map = self.eval_first_arg(args)?;
                Ok(Value::List(
                    expect_map(map)?.into_iter().map(|(key, _)| key).collect(),
                ))
            }
            ("Map", "values") => {
                let map = self.eval_first_arg(args)?;
                Ok(Value::List(
                    expect_map(map)?
                        .into_iter()
                        .map(|(_, value)| value)
                        .collect(),
                ))
            }
            ("PersistentMap", "new") => Ok(Value::Map(Vec::new())),
            ("PersistentMap", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_map(value)?.len() as i64))
            }
            ("PersistentMap", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_map(value)?.is_empty()))
            }
            ("PersistentMap", "contains_key") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                Ok(Value::Bool(map_get(&expect_map(map)?, &key).is_some()))
            }
            ("PersistentMap", "get") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                Ok(value_option(map_get(&expect_map(map)?, &key), |value| {
                    value
                }))
            }
            ("PersistentMap", "insert") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let value = self.eval_named_or_positional_arg(args, "value", 2)?;
                let mut map = expect_map(map)?;
                map_insert(&mut map, key, value);
                Ok(Value::Map(map))
            }
            ("PersistentMap", "remove") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let mut map = expect_map(map)?;
                map_remove(&mut map, &key);
                Ok(Value::Map(map))
            }
            ("PersistentMap", "clear") => {
                self.eval_first_arg(args)?;
                Ok(Value::Map(Vec::new()))
            }
            ("Regex", "compile") => {
                let pattern = self.eval_first_arg(args)?;
                let pattern = expect_string(pattern)?;
                Ok(result_value(
                    regex::Regex::new(&pattern)
                        .map(|_| regex_value(pattern))
                        .map_err(|error| regex_error(error.to_string())),
                ))
            }
            ("Regex", "is_match") => {
                let regex = self.eval_named_or_positional_arg(args, "regex", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let regex = expect_regex(regex)?;
                Ok(Value::Bool(regex.is_match(&expect_string(value)?)))
            }
            ("Regex", "find") => {
                let regex = self.eval_named_or_positional_arg(args, "regex", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let regex = expect_regex(regex)?;
                let value = expect_string(value)?;
                Ok(value_option(
                    regex.find(&value).map(|m| m.as_str().to_string()),
                    Value::String,
                ))
            }
            ("Regex", "captures") => {
                let regex = self.eval_named_or_positional_arg(args, "regex", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let regex = expect_regex(regex)?;
                let captures = regex
                    .captures(&expect_string(value)?)
                    .map(|captures| {
                        captures
                            .iter()
                            .filter_map(|matched| {
                                matched.map(|matched| Value::String(matched.as_str().to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Value::List(captures))
            }
            ("Regex", "replace_all") => {
                let regex = self.eval_named_or_positional_arg(args, "regex", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let replacement = self.eval_named_or_positional_arg(args, "replacement", 2)?;
                let regex = expect_regex(regex)?;
                Ok(Value::String(
                    regex
                        .replace_all(&expect_string(value)?, &expect_string(replacement)?)
                        .to_string(),
                ))
            }
            ("Regex", "split") => {
                let regex = self.eval_named_or_positional_arg(args, "regex", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let regex = expect_regex(regex)?;
                let parts = regex
                    .split(&expect_string(value)?)
                    .map(|part| Value::String(part.to_string()))
                    .collect();
                Ok(Value::List(parts))
            }
            ("RegexError", "message") => {
                let value = self.eval_first_arg(args)?;
                read_field(&value, "message")
            }
            ("Request", "new") => {
                let path = self.eval_first_arg(args)?;
                Ok(Value::Struct {
                    name: "Request".to_string(),
                    fields: BTreeMap::from([(
                        "path".to_string(),
                        Value::String(expect_string(path)?),
                    )]),
                })
            }
            ("Request", "path") => {
                let request = self.eval_first_arg(args)?;
                read_field(&request, "path")
            }
            ("Response", "ok") => {
                let body = self.eval_first_arg(args)?;
                Ok(value_ok(Value::Struct {
                    name: "Response".to_string(),
                    fields: BTreeMap::from([
                        ("status".to_string(), Value::Int(200)),
                        ("body".to_string(), Value::String(expect_string(body)?)),
                    ]),
                }))
            }
            ("Response", "status") => {
                let response = self.eval_first_arg(args)?;
                read_field(&response, "status")
            }
            ("Response", "body") => {
                let response = self.eval_first_arg(args)?;
                read_field(&response, "body")
            }
            ("Http", "get") => {
                let url = self.eval_named_or_positional_arg(args, "url", 0)?;
                Ok(value_err(http_error(format!(
                    "HTTP client runtime is not configured for GET {}",
                    expect_string(url)?
                ))))
            }
            ("Http", "post_json") => {
                let url = self.eval_named_or_positional_arg(args, "url", 0)?;
                let _body = self.eval_named_or_positional_arg(args, "body", 1)?;
                Ok(value_err(http_error(format!(
                    "HTTP client runtime is not configured for POST JSON {}",
                    expect_string(url)?
                ))))
            }
            ("Http", "post_form") => {
                let url = self.eval_named_or_positional_arg(args, "url", 0)?;
                let _body = self.eval_named_or_positional_arg(args, "body", 1)?;
                Ok(value_err(http_error(format!(
                    "HTTP client runtime is not configured for POST form {}",
                    expect_string(url)?
                ))))
            }
            ("HttpError", "message") => {
                let error = self.eval_named_or_positional_arg(args, "error", 0)?;
                read_field(&error, "message")
            }
            ("HttpRequest", "json") => {
                let url = self.eval_named_or_positional_arg(args, "url", 0)?;
                let body = self.eval_named_or_positional_arg(args, "body", 1)?;
                Ok(http_request_value(
                    "POST",
                    expect_string(url)?,
                    expect_string(body)?,
                    0,
                    1,
                    0,
                    0,
                ))
            }
            ("HttpRequest", "with_timeout") => {
                let request = self.eval_named_or_positional_arg(args, "request", 0)?;
                let timeout_ms = self.eval_named_or_positional_arg(args, "timeout_ms", 1)?;
                let mut request = expect_http_request(request)?;
                request.timeout_ms = expect_int(timeout_ms)?;
                Ok(request.to_value())
            }
            ("HttpRequest", "with_retry") => {
                let request = self.eval_named_or_positional_arg(args, "request", 0)?;
                let attempts = self.eval_named_or_positional_arg(args, "attempts", 1)?;
                let backoff_ms = self.eval_named_or_positional_arg(args, "backoff_ms", 2)?;
                let mut request = expect_http_request(request)?;
                request.attempts = expect_int(attempts)?;
                request.backoff_ms = expect_int(backoff_ms)?;
                Ok(request.to_value())
            }
            ("HttpRequest", "with_header") => {
                let request = self.eval_named_or_positional_arg(args, "request", 0)?;
                let _name = self.eval_named_or_positional_arg(args, "name", 1)?;
                let _value = self.eval_named_or_positional_arg(args, "value", 2)?;
                let mut request = expect_http_request(request)?;
                request.header_count += 1;
                Ok(request.to_value())
            }
            ("Option", "is_some") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(matches!(
                    value,
                    Value::Variant { name, .. } if name == "Some"
                )))
            }
            ("Option", "is_none") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(matches!(
                    value,
                    Value::Variant { name, .. } if name == "None"
                )))
            }
            ("Option", "unwrap_or") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let default = self.eval_named_or_positional_arg(args, "default", 1)?;
                Ok(option_payload(value)?.unwrap_or(default))
            }
            ("Option", "ok_or") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let error = self.eval_named_or_positional_arg(args, "error", 1)?;
                Ok(match option_payload(value)? {
                    Some(value) => value_ok(value),
                    None => value_err(error),
                })
            }
            ("Option", "or") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let fallback = self.eval_named_or_positional_arg(args, "fallback", 1)?;
                Ok(match option_payload(value)? {
                    Some(value) => value_some(value),
                    None => fallback,
                })
            }
            ("String", "join") => {
                let parts = self.eval_named_or_positional_arg(args, "parts", 0)?;
                let separator = self.eval_named_or_positional_arg(args, "separator", 1)?;
                Ok(Value::String(
                    expect_string_list(parts)?.join(&expect_string(separator)?),
                ))
            }
            ("String", "env") => {
                let name = self.eval_first_arg(args)?;
                Ok(value_option(
                    std::env::var(expect_string(name)?).ok(),
                    |value| Value::String(value),
                ))
            }
            ("String", "env_or") => {
                let name = self.eval_named_or_positional_arg(args, "value", 0)?;
                let default = self.eval_named_or_positional_arg(args, "default", 1)?;
                Ok(Value::String(
                    std::env::var(expect_string(name)?).unwrap_or(expect_string(default)?),
                ))
            }
            ("TempDir", "new") => Ok(result_value(tempdir_new_value(std::env::temp_dir()))),
            ("TempDir", "new_in") => {
                let parent = self.eval_named_or_positional_arg(args, "parent", 0)?;
                Ok(result_value(tempdir_new_value(PathBuf::from(
                    expect_string(parent)?,
                ))))
            }
            ("TempDir", "path") => {
                let dir = self
                    .eval_named_or_positional_arg(args, "dir", 0)
                    .or_else(|_| self.eval_first_arg(args))?;
                Ok(Value::String(expect_tempdir_path(dir)?))
            }
            ("Path", "from_string")
            | ("Path", "to_string")
            | ("String", "to_path")
            | ("String", "to_url")
            | ("Url", "from_string")
            | ("Url", "to_string") => self.eval_first_arg(args),
            ("Path", "join") => {
                let base = self.eval_named_or_positional_arg(args, "base", 0)?;
                let child = self.eval_named_or_positional_arg(args, "child", 1)?;
                Ok(Value::String(path_join_string(
                    &expect_string(base)?,
                    &expect_string(child)?,
                )))
            }
            ("Path", "normalize") => {
                let path = self.eval_first_arg(args)?;
                Ok(Value::String(path_normalize_string(&expect_string(path)?)))
            }
            ("Path", "parent") => {
                let path = self.eval_first_arg(args)?;
                Ok(value_option(
                    Path::new(&expect_string(path)?)
                        .parent()
                        .map(|parent| Value::String(parent.to_string_lossy().to_string())),
                    |value| value,
                ))
            }
            ("Path", "file_name") => {
                let path = self.eval_first_arg(args)?;
                Ok(value_option(
                    Path::new(&expect_string(path)?)
                        .file_name()
                        .map(|name| Value::String(name.to_string_lossy().to_string())),
                    |value| value,
                ))
            }
            ("Path", "extension") => {
                let path = self.eval_first_arg(args)?;
                Ok(value_option(
                    Path::new(&expect_string(path)?)
                        .extension()
                        .map(|extension| Value::String(extension.to_string_lossy().to_string())),
                    |value| value,
                ))
            }
            ("Path", "is_absolute") => {
                let path = self.eval_first_arg(args)?;
                Ok(Value::Bool(Path::new(&expect_string(path)?).is_absolute()))
            }
            ("Path", "with_extension") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let extension = self.eval_named_or_positional_arg(args, "extension", 1)?;
                let mut path = PathBuf::from(expect_string(path)?);
                path.set_extension(expect_string(extension)?);
                Ok(Value::String(path.to_string_lossy().to_string()))
            }
            ("Path", "starts_with") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                let base = self.eval_named_or_positional_arg(args, "base", 1)?;
                Ok(Value::Bool(
                    Path::new(&expect_string(path)?).starts_with(Path::new(&expect_string(base)?)),
                ))
            }
            ("Path", "safe_relative") | ("String", "safe_relative") => {
                let value = self.eval_first_arg(args)?;
                Ok(result_value(
                    path_safe_relative_string(&expect_string(value)?)
                        .map(Value::String)
                        .map_err(Value::String),
                ))
            }
            ("Path", "resolve_relative") | ("Workspace", "resolve") => {
                let root = self.eval_named_or_positional_arg(args, "root", 0)?;
                let relative = self.eval_named_or_positional_arg(args, "relative", 1)?;
                Ok(result_value(
                    path_resolve_relative_string(&expect_string(root)?, &expect_string(relative)?)
                        .map(Value::String)
                        .map_err(Value::String),
                ))
            }
            ("Patch", "apply_text") => {
                let original = self.eval_named_or_positional_arg(args, "original", 0)?;
                let patch = self.eval_named_or_positional_arg(args, "patch", 1)?;
                Ok(result_value(
                    patch_apply_text_string(&expect_string(original)?, &expect_string(patch)?)
                        .map(Value::String)
                        .map_err(Value::String),
                ))
            }
            ("Result", "is_ok") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(matches!(
                    value,
                    Value::Variant { name, .. } if name == "Ok"
                )))
            }
            ("Result", "is_err") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(matches!(
                    value,
                    Value::Variant { name, .. } if name == "Err"
                )))
            }
            ("Result", "unwrap_or") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let default = self.eval_named_or_positional_arg(args, "default", 1)?;
                Ok(result_payload(value)?.map_or(default, |value| value))
            }
            ("Result", "ok") => {
                let value = self.eval_first_arg(args)?;
                Ok(match result_payload(value)? {
                    Ok(value) => value_some(value),
                    Err(_) => value_none(),
                })
            }
            ("Result", "err") => {
                let value = self.eval_first_arg(args)?;
                Ok(match result_payload(value)? {
                    Ok(_) => value_none(),
                    Err(error) => value_some(error),
                })
            }
            ("Result", "err_message") => {
                let value = self.eval_first_arg(args)?;
                Ok(match result_payload(value)? {
                    Ok(_) => value_none(),
                    Err(error) => value_some(Value::String(error.display())),
                })
            }
            ("Set", "new") => Ok(Value::List(Vec::new())),
            ("Set", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_list(value)?.len() as i64))
            }
            ("Set", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_list(value)?.is_empty()))
            }
            ("Set", "contains") => {
                let set = self.eval_named_or_positional_arg(args, "set", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                Ok(Value::Bool(
                    expect_list(set)?.iter().any(|item| item == &value),
                ))
            }
            ("Set", "insert") => {
                let set_name = self.mut_arg_local_name(args, "set", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let mut set = expect_list(self.lookup(set_name)?)?;
                let inserted = set_insert(&mut set, value);
                self.assign(set_name, Value::List(set))?;
                Ok(Value::Bool(inserted))
            }
            ("Set", "remove") => {
                let set_name = self.mut_arg_local_name(args, "set", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let mut set = expect_list(self.lookup(set_name)?)?;
                let removed = set_remove(&mut set, &value);
                self.assign(set_name, Value::List(set))?;
                Ok(Value::Bool(removed))
            }
            ("Set", "clear") => {
                let set_name = self.mut_arg_local_name(args, "set", 0)?;
                self.assign(set_name, Value::List(Vec::new()))?;
                Ok(Value::Unit)
            }
            ("Set", "to_list") => self.eval_first_arg(args),
            ("Set", "union") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                let mut set = expect_list(left)?;
                for value in expect_list(right)? {
                    set_insert(&mut set, value);
                }
                Ok(Value::List(set))
            }
            ("Set", "intersection") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                let right = expect_list(right)?;
                Ok(Value::List(
                    expect_list(left)?
                        .into_iter()
                        .filter(|value| right.iter().any(|item| item == value))
                        .collect(),
                ))
            }
            ("Set", "difference") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                let right = expect_list(right)?;
                Ok(Value::List(
                    expect_list(left)?
                        .into_iter()
                        .filter(|value| !right.iter().any(|item| item == value))
                        .collect(),
                ))
            }
            ("Set", "is_subset") => {
                let left = self.eval_named_or_positional_arg(args, "left", 0)?;
                let right = self.eval_named_or_positional_arg(args, "right", 1)?;
                let right = expect_list(right)?;
                Ok(Value::Bool(
                    expect_list(left)?
                        .iter()
                        .all(|value| right.iter().any(|item| item == value)),
                ))
            }
            ("SortedSet", "new") => Ok(Value::List(Vec::new())),
            ("SortedSet", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_list(value)?.len() as i64))
            }
            ("SortedSet", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_list(value)?.is_empty()))
            }
            ("SortedSet", "contains") => {
                let set = self.eval_named_or_positional_arg(args, "set", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                Ok(Value::Bool(
                    expect_list(set)?.iter().any(|item| item == &value),
                ))
            }
            ("SortedSet", "insert") => {
                let set_name = self.mut_arg_local_name(args, "set", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let mut set = expect_list(self.lookup(set_name)?)?;
                let inserted = set_insert(&mut set, value);
                sort_values(&mut set)?;
                self.assign(set_name, Value::List(set))?;
                Ok(Value::Bool(inserted))
            }
            ("SortedSet", "remove") => {
                let set_name = self.mut_arg_local_name(args, "set", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                let mut set = expect_list(self.lookup(set_name)?)?;
                let removed = set_remove(&mut set, &value);
                self.assign(set_name, Value::List(set))?;
                Ok(Value::Bool(removed))
            }
            ("SortedSet", "clear") => {
                let set_name = self.mut_arg_local_name(args, "set", 0)?;
                self.assign(set_name, Value::List(Vec::new()))?;
                Ok(Value::Unit)
            }
            ("SortedSet", "to_list") => self.eval_first_arg(args),
            ("SortedMap", "new") => Ok(Value::Map(Vec::new())),
            ("SortedMap", "len") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Int(expect_map(value)?.len() as i64))
            }
            ("SortedMap", "is_empty") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_map(value)?.is_empty()))
            }
            ("SortedMap", "contains_key") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                Ok(Value::Bool(map_get(&expect_map(map)?, &key).is_some()))
            }
            ("SortedMap", "get") => {
                let map = self.eval_named_or_positional_arg(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                Ok(value_option(map_get(&expect_map(map)?, &key), |value| {
                    value
                }))
            }
            ("SortedMap", "insert") => {
                let map_name = self.mut_arg_local_name(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let value = self.eval_named_or_positional_arg(args, "value", 2)?;
                let mut map = expect_map(self.lookup(map_name)?)?;
                map_insert(&mut map, key, value);
                sort_map_entries(&mut map)?;
                self.assign(map_name, Value::Map(map))?;
                Ok(Value::Unit)
            }
            ("SortedMap", "remove") => {
                let map_name = self.mut_arg_local_name(args, "map", 0)?;
                let key = self.eval_named_or_positional_arg(args, "key", 1)?;
                let mut map = expect_map(self.lookup(map_name)?)?;
                let old = map_remove(&mut map, &key);
                self.assign(map_name, Value::Map(map))?;
                Ok(value_option(old, |value| value))
            }
            ("SortedMap", "clear") => {
                let map_name = self.mut_arg_local_name(args, "map", 0)?;
                self.assign(map_name, Value::Map(Vec::new()))?;
                Ok(Value::Unit)
            }
            ("SortedMap", "keys") => {
                let map = self.eval_first_arg(args)?;
                Ok(Value::List(
                    expect_map(map)?.into_iter().map(|(key, _)| key).collect(),
                ))
            }
            ("SortedMap", "values") => {
                let map = self.eval_first_arg(args)?;
                Ok(Value::List(
                    expect_map(map)?
                        .into_iter()
                        .map(|(_, value)| value)
                        .collect(),
                ))
            }
            ("Json", "value") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Json(value_to_json_literal(value)?))
            }
            ("Json", "parse") | ("Json", "json_parse") => {
                let value = self.eval_first_arg(args)?;
                Ok(result_value(
                    serde_json::from_str(&expect_string(value)?)
                        .map(Value::Json)
                        .map_err(|error| json_error(error.to_string())),
                ))
            }
            ("Json", "parse_file") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                Ok(json_result(
                    std::fs::read_to_string(expect_string(path)?)
                        .map_err(|error| error.to_string())
                        .and_then(|text| {
                            serde_json::from_str(&text).map_err(|error| error.to_string())
                        }),
                ))
            }
            ("Toml", "parse_file") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                Ok(json_result(
                    std::fs::read_to_string(expect_string(path)?)
                        .map_err(|error| error.to_string())
                        .and_then(|text| {
                            text.parse::<toml::Value>()
                                .map_err(|error| error.to_string())
                        })
                        .and_then(|value| {
                            serde_json::to_value(value).map_err(|error| error.to_string())
                        }),
                ))
            }
            ("Yaml", "parse") => {
                let text = self.eval_named_or_positional_arg(args, "text", 0)?;
                Ok(yaml_parse_result(&expect_string(text)?))
            }
            ("Yaml", "parse_file") => {
                let path = self.eval_named_or_positional_arg(args, "path", 0)?;
                Ok(json_result(
                    std::fs::read_to_string(expect_string(path)?)
                        .map_err(|error| error.to_string())
                        .and_then(|text| yaml_parse_json(&text)),
                ))
            }
            ("Json", "to_string") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::String(expect_json(value)?.to_string()))
            }
            ("Json", "quote_string") => {
                let value = self.eval_first_arg(args)?;
                serde_json::to_string(&expect_string(value)?)
                    .map(Value::String)
                    .map_err(|error| EvalError::Runtime(error.to_string()))
            }
            ("Json", "clone") => self.eval_first_arg(args),
            ("Json", "string_field") => {
                let name = self.eval_named_or_positional_arg(args, "name", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                Ok(Value::String(format!(
                    "{}:{}",
                    json_quote_string(&expect_string(name)?)?,
                    json_quote_string(&expect_string(value)?)?
                )))
            }
            ("Json", "int_field") => {
                let name = self.eval_named_or_positional_arg(args, "name", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                Ok(Value::String(format!(
                    "{}:{}",
                    json_quote_string(&expect_string(name)?)?,
                    expect_int(value)?
                )))
            }
            ("Json", "bool_field") => {
                let name = self.eval_named_or_positional_arg(args, "name", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                Ok(Value::String(format!(
                    "{}:{}",
                    json_quote_string(&expect_string(name)?)?,
                    expect_bool(value)?
                )))
            }
            ("Json", "raw_field") => {
                let name = self.eval_named_or_positional_arg(args, "name", 0)?;
                let value = self.eval_named_or_positional_arg(args, "value", 1)?;
                Ok(Value::String(format!(
                    "{}:{}",
                    json_quote_string(&expect_string(name)?)?,
                    expect_string(value)?
                )))
            }
            ("Json", "object") => {
                let fields = self.eval_first_arg(args)?;
                Ok(Value::String(format!(
                    "{{{}}}",
                    expect_string_list(fields)?.join(",")
                )))
            }
            ("Json", "array") => {
                let items = self.eval_first_arg(args)?;
                Ok(Value::String(format!(
                    "[{}]",
                    expect_string_list(items)?.join(",")
                )))
            }
            ("Json", "string_array") => {
                let items = self.eval_first_arg(args)?;
                let quoted = expect_string_list(items)?
                    .into_iter()
                    .map(|item| json_quote_string(&item))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::String(format!("[{}]", quoted.join(","))))
            }
            ("Json", "strings") => {
                let items = self.eval_first_arg(args)?;
                Ok(Value::Json(serde_json::Value::Array(
                    expect_string_list(items)?
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                )))
            }
            ("Json", "values") => {
                let items = self.eval_first_arg(args)?;
                Ok(Value::Json(serde_json::Value::Array(expect_json_list(
                    items,
                )?)))
            }
            ("Json", "is_null") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_json(value)?.is_null()))
            }
            ("Json", "is_array") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_json(value)?.is_array()))
            }
            ("Json", "is_object") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::Bool(expect_json(value)?.is_object()))
            }
            ("Json", "kind") => {
                let value = self.eval_first_arg(args)?;
                Ok(Value::String(json_kind(&expect_json(value)?).to_string()))
            }
            ("Json", "value_at") | ("Json", "at") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(json_value_at(
                    expect_json(value)?,
                    &expect_string(path)?,
                )))
            }
            ("Json", "at_or") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let fallback = self.eval_named_or_positional_arg(args, "fallback", 2)?;
                Ok(json_value_at(expect_json(value)?, &expect_string(path)?)
                    .unwrap_or(Value::Json(expect_json(fallback)?)))
            }
            ("Json", "at_string") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(
                    json_value_at(expect_json(value)?, &expect_string(path)?)
                        .and_then(value_json)
                        .and_then(json_as_string_value),
                ))
            }
            ("Json", "at_int") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(
                    json_value_at(expect_json(value)?, &expect_string(path)?)
                        .and_then(value_json)
                        .and_then(json_as_int_value),
                ))
            }
            ("Json", "at_bool") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(
                    json_value_at(expect_json(value)?, &expect_string(path)?)
                        .and_then(value_json)
                        .and_then(json_as_bool_value),
                ))
            }
            ("Json", "at_optional") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(Ok(json_optional_path(
                    expect_json(value)?,
                    &expect_string(path)?,
                    Value::Json,
                ))))
            }
            ("Json", "at_optional_string") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(json_optional_typed_path(
                    expect_json(value)?,
                    &expect_string(path)?,
                    json_as_string_value,
                )))
            }
            ("Json", "at_optional_int") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(json_optional_typed_path(
                    expect_json(value)?,
                    &expect_string(path)?,
                    json_as_int_value,
                )))
            }
            ("Json", "at_optional_bool") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(json_optional_typed_path(
                    expect_json(value)?,
                    &expect_string(path)?,
                    json_as_bool_value,
                )))
            }
            ("Json", "at_string_or") | ("Json", "at_to_string_or") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let fallback = self.eval_named_or_positional_arg(args, "fallback", 2)?;
                let fallback = expect_string(fallback)?;
                let found = json_value_at(expect_json(value)?, &expect_string(path)?)
                    .and_then(value_json)
                    .and_then(|json| {
                        if name == "at_to_string_or" {
                            Ok(Value::String(json.to_string()))
                        } else {
                            json_as_string_value(json)
                        }
                    });
                Ok(found.unwrap_or(Value::String(fallback)))
            }
            ("Json", "at_int_or") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let fallback = self.eval_named_or_positional_arg(args, "fallback", 2)?;
                let found = json_value_at(expect_json(value)?, &expect_string(path)?)
                    .and_then(value_json)
                    .and_then(json_as_int_value);
                Ok(found.unwrap_or(Value::Int(expect_int(fallback)?)))
            }
            ("Json", "at_bool_or") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let fallback = self.eval_named_or_positional_arg(args, "fallback", 2)?;
                let found = json_value_at(expect_json(value)?, &expect_string(path)?)
                    .and_then(value_json)
                    .and_then(json_as_bool_value);
                Ok(found.unwrap_or(Value::Bool(expect_bool(fallback)?)))
            }
            ("Json", "at_to_string") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                Ok(result_value(
                    json_value_at(expect_json(value)?, &expect_string(path)?)
                        .and_then(value_json)
                        .map(|json| Value::String(json.to_string())),
                ))
            }
            ("Json", "string_at") => {
                let text = self.eval_named_or_positional_arg(args, "text", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let text = expect_string(text)?;
                let path = expect_string(path)?;
                Ok(result_value(
                    parse_json_value(&text)
                        .and_then(|json| json_value_at(json, &path))
                        .and_then(value_json)
                        .and_then(json_as_string_value),
                ))
            }
            ("Json", "int_at") => {
                let text = self.eval_named_or_positional_arg(args, "text", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let text = expect_string(text)?;
                let path = expect_string(path)?;
                Ok(result_value(
                    parse_json_value(&text)
                        .and_then(|json| json_value_at(json, &path))
                        .and_then(value_json)
                        .and_then(json_as_int_value),
                ))
            }
            ("Json", "bool_at") => {
                let text = self.eval_named_or_positional_arg(args, "text", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let text = expect_string(text)?;
                let path = expect_string(path)?;
                Ok(result_value(
                    parse_json_value(&text)
                        .and_then(|json| json_value_at(json, &path))
                        .and_then(value_json)
                        .and_then(json_as_bool_value),
                ))
            }
            ("Json", "to_string_at") => {
                let text = self.eval_named_or_positional_arg(args, "text", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let text = expect_string(text)?;
                let path = expect_string(path)?;
                Ok(result_value(
                    parse_json_value(&text)
                        .and_then(|json| json_value_at(json, &path))
                        .and_then(value_json)
                        .map(|json| Value::String(json.to_string())),
                ))
            }
            ("Json", "string_at_or")
            | ("Json", "json_string_at_or")
            | ("Json", "to_string_at_or") => {
                let text = self.eval_named_or_positional_arg(args, "text", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let fallback = self.eval_named_or_positional_arg(args, "fallback", 2)?;
                let text = expect_string(text)?;
                let path = expect_string(path)?;
                let fallback = expect_string(fallback)?;
                let found = parse_json_value(&text)
                    .and_then(|json| json_value_at(json, &path))
                    .and_then(value_json)
                    .and_then(|json| {
                        if name == "to_string_at_or" {
                            Ok(Value::String(json.to_string()))
                        } else {
                            json_as_string_value(json)
                        }
                    });
                Ok(found.unwrap_or(Value::String(fallback)))
            }
            ("Json", "int_at_or") | ("Json", "json_int_at_or") => {
                let text = self.eval_named_or_positional_arg(args, "text", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let fallback = self.eval_named_or_positional_arg(args, "fallback", 2)?;
                let text = expect_string(text)?;
                let path = expect_string(path)?;
                let found = parse_json_value(&text)
                    .and_then(|json| json_value_at(json, &path))
                    .and_then(value_json)
                    .and_then(json_as_int_value);
                Ok(found.unwrap_or(Value::Int(expect_int(fallback)?)))
            }
            ("Json", "bool_at_or") | ("Json", "json_bool_at_or") => {
                let text = self.eval_named_or_positional_arg(args, "text", 0)?;
                let path = self.eval_named_or_positional_arg(args, "path", 1)?;
                let fallback = self.eval_named_or_positional_arg(args, "fallback", 2)?;
                let text = expect_string(text)?;
                let path = expect_string(path)?;
                let found = parse_json_value(&text)
                    .and_then(|json| json_value_at(json, &path))
                    .and_then(value_json)
                    .and_then(json_as_bool_value);
                Ok(found.unwrap_or(Value::Bool(expect_bool(fallback)?)))
            }
            ("Json", "field") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let name = self.eval_named_or_positional_arg(args, "name", 1)?;
                Ok(result_value(json_field(
                    expect_json(value)?,
                    &expect_string(name)?,
                )))
            }
            ("Json", "field_string") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let name = self.eval_named_or_positional_arg(args, "name", 1)?;
                Ok(result_value(
                    json_field(expect_json(value)?, &expect_string(name)?)
                        .and_then(value_json)
                        .and_then(json_as_string_value),
                ))
            }
            ("Json", "field_int") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let name = self.eval_named_or_positional_arg(args, "name", 1)?;
                Ok(result_value(
                    json_field(expect_json(value)?, &expect_string(name)?)
                        .and_then(value_json)
                        .and_then(json_as_int_value),
                ))
            }
            ("Json", "field_bool") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let name = self.eval_named_or_positional_arg(args, "name", 1)?;
                Ok(result_value(
                    json_field(expect_json(value)?, &expect_string(name)?)
                        .and_then(value_json)
                        .and_then(json_as_bool_value),
                ))
            }
            ("Json", "field_optional") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let name = self.eval_named_or_positional_arg(args, "name", 1)?;
                Ok(result_value(Ok(json_optional_field(
                    expect_json(value)?,
                    &expect_string(name)?,
                    Value::Json,
                ))))
            }
            ("Json", "field_optional_string") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let name = self.eval_named_or_positional_arg(args, "name", 1)?;
                Ok(result_value(json_optional_typed_field(
                    expect_json(value)?,
                    &expect_string(name)?,
                    json_as_string_value,
                )))
            }
            ("Json", "field_optional_int") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let name = self.eval_named_or_positional_arg(args, "name", 1)?;
                Ok(result_value(json_optional_typed_field(
                    expect_json(value)?,
                    &expect_string(name)?,
                    json_as_int_value,
                )))
            }
            ("Json", "field_optional_bool") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let name = self.eval_named_or_positional_arg(args, "name", 1)?;
                Ok(result_value(json_optional_typed_field(
                    expect_json(value)?,
                    &expect_string(name)?,
                    json_as_bool_value,
                )))
            }
            ("Json", "array_len") => {
                let value = self.eval_first_arg(args)?;
                let json = expect_json(value)?;
                Ok(result_value(
                    json_array(&json).map(|items| Value::Int(items.len() as i64)),
                ))
            }
            ("Json", "array_get") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let index = self.eval_named_or_positional_arg(args, "index", 1)?;
                Ok(result_value(json_array_get_result(
                    expect_json(value)?,
                    expect_int(index)?,
                )))
            }
            ("Json", "array_strings") => {
                let value = self.eval_first_arg(args)?;
                Ok(result_value(
                    json_array_strings(expect_json(value)?).map(Value::List),
                ))
            }
            ("Json", "array_ints") => {
                let value = self.eval_first_arg(args)?;
                Ok(result_value(
                    json_array_ints(expect_json(value)?).map(Value::List),
                ))
            }
            ("Json", "array_bools") => {
                let value = self.eval_first_arg(args)?;
                Ok(result_value(
                    json_array_bools(expect_json(value)?).map(Value::List),
                ))
            }
            ("Json", "array_contains_string") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let item = self.eval_named_or_positional_arg(args, "item", 1)?;
                Ok(result_value(json_array_contains_string(
                    expect_json(value)?,
                    &expect_string(item)?,
                    JsonArrayStringMatch::Exact,
                )))
            }
            ("Json", "array_contains_substring") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let text = self.eval_named_or_positional_arg(args, "text", 1)?;
                Ok(result_value(json_array_contains_string(
                    expect_json(value)?,
                    &expect_string(text)?,
                    JsonArrayStringMatch::Substring,
                )))
            }
            ("Json", "array_contains_prefix") => {
                let value = self.eval_named_or_positional_arg(args, "value", 0)?;
                let prefix = self.eval_named_or_positional_arg(args, "prefix", 1)?;
                Ok(result_value(json_array_contains_string(
                    expect_json(value)?,
                    &expect_string(prefix)?,
                    JsonArrayStringMatch::Prefix,
                )))
            }
            ("Json", "object_len") => {
                let value = self.eval_first_arg(args)?;
                let json = expect_json(value)?;
                Ok(result_value(
                    json_object(&json).map(|fields| Value::Int(fields.len() as i64)),
                ))
            }
            ("Json", "object_keys") => {
                let value = self.eval_first_arg(args)?;
                let json = expect_json(value)?;
                Ok(result_value(json_object(&json).map(|fields| {
                    let mut keys = fields
                        .keys()
                        .cloned()
                        .map(Value::String)
                        .collect::<Vec<_>>();
                    keys.sort_by_key(Value::display);
                    Value::List(keys)
                })))
            }
            ("Json", "as_string") => {
                let value = self.eval_first_arg(args)?;
                Ok(result_value(json_as_string_value(expect_json(value)?)))
            }
            ("Json", "as_int") => {
                let value = self.eval_first_arg(args)?;
                Ok(result_value(json_as_int_value(expect_json(value)?)))
            }
            ("Json", "as_bool") => {
                let value = self.eval_first_arg(args)?;
                Ok(result_value(json_as_bool_value(expect_json(value)?)))
            }
            ("JsonError", "message") => {
                let value = self.eval_first_arg(args)?;
                read_field(&value, "message")
            }
            _ => Err(EvalError::Runtime(format!(
                "interpreter P0 does not support runtime intrinsic `{namespace}.{name}`."
            ))),
        }
    }

    fn eval_receiver_call(
        &mut self,
        receiver: Value,
        method: &str,
        args: &[HirCallArg],
    ) -> Result<Value, EvalError> {
        match (receiver, method) {
            (Value::Int(value), "to_string") => Ok(Value::String(value.to_string())),
            (Value::String(value), "len") => Ok(Value::Int(value.len() as i64)),
            (Value::String(value), "is_empty") => Ok(Value::Bool(value.is_empty())),
            (Value::String(value), "concat") => {
                let right = self.eval_named_or_positional_arg(args, "right", 0)?;
                Ok(Value::String(format!("{value}{}", expect_string(right)?)))
            }
            (Value::String(value), "to_path") => Ok(Value::String(value)),
            (Value::String(value), "to_url") => Ok(Value::String(value)),
            (Value::String(value), "env") => Ok(value_option(std::env::var(value).ok(), |value| {
                Value::String(value)
            })),
            (Value::String(value), "env_or") => {
                let default = self.eval_named_or_positional_arg(args, "default", 0)?;
                Ok(Value::String(
                    std::env::var(value).unwrap_or(expect_string(default)?),
                ))
            }
            (Value::String(value), "safe_relative") => Ok(result_value(
                path_safe_relative_string(&value)
                    .map(Value::String)
                    .map_err(Value::String),
            )),
            (Value::List(value), "len") => Ok(Value::Int(value.len() as i64)),
            (Value::List(value), "is_empty") => Ok(Value::Bool(value.is_empty())),
            (Value::List(value), "get") => {
                let index = self.eval_named_or_positional_arg(args, "index", 0)?;
                list_get(value, expect_int(index)?)
            }
            (Value::List(value), "first") => {
                Ok(value_option(value.into_iter().next(), |value| value))
            }
            (Value::List(value), "last") => {
                Ok(value_option(value.into_iter().last(), |value| value))
            }
            (_, method) => Err(EvalError::Runtime(format!(
                "interpreter P0 does not support receiver method `{method}`."
            ))),
        }
    }

    /// Minimal evaluator for the AST receiver expression of a receiver call.
    /// HIR does not lower the receiver (it lives inside the reused AST
    /// `Callee::ReceiverCall`), so this bridges the pure subset of receivers
    /// that appear in checked programs and fails closed on anything else.
    fn eval_ast_expr(&mut self, expr: &Expr) -> Result<Value, EvalError> {
        match expr {
            Expr::Ident(name, _) if name == "Unit" => Ok(Value::Unit),
            Expr::Ident(name, _) if name == "true" => Ok(Value::Bool(true)),
            Expr::Ident(name, _) if name == "false" => Ok(Value::Bool(false)),
            Expr::Ident(name, _) => self.lookup(name),
            Expr::Number(value, _) => value
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|error| EvalError::Runtime(format!("invalid integer `{value}`: {error}"))),
            Expr::String(value, _) => Ok(Value::String(decode_string_token(value))),
            Expr::MultilineString(value, _) => Ok(Value::String(value.clone())),
            Expr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.eval_ast_expr(item))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::List(items))
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => self.eval_ast_expr(value),
            Expr::Field { base, name, .. } => {
                let base = self.eval_ast_expr(base)?;
                read_field(&base, name)
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let left = self.eval_ast_expr(left)?;
                let right = self.eval_ast_expr(right)?;
                eval_binary(*op, left, right)
            }
            _ => Err(EvalError::Runtime(
                "interpreter P0 does not support this receiver expression.".to_string(),
            )),
        }
    }

    fn eval_first_arg(&mut self, args: &[HirCallArg]) -> Result<Value, EvalError> {
        self.eval_named_or_positional_arg(args, "value", 0)
    }

    fn eval_named_or_positional_arg(
        &mut self,
        args: &[HirCallArg],
        name: &str,
        index: usize,
    ) -> Result<Value, EvalError> {
        let arg = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some(name))
            .or_else(|| args.get(index))
            .ok_or_else(|| EvalError::Runtime(format!("missing argument `{name}`.")))?;
        self.eval_expr(&arg.value)
    }

    fn mut_arg_local_name<'b>(
        &self,
        args: &'b [HirCallArg],
        name: &str,
        index: usize,
    ) -> Result<&'b str, EvalError> {
        let arg = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some(name))
            .or_else(|| args.get(index))
            .ok_or_else(|| EvalError::Runtime(format!("missing argument `{name}`.")))?;
        match &arg.value {
            HirExpr::Effect { effect, value, .. } if *effect == crate::hir::ParamEffect::Mut => {
                match value.as_ref() {
                    HirExpr::Ident { name, .. } => Ok(name),
                    _ => Err(EvalError::Runtime(format!(
                        "interpreter P0 only supports mutable `{name}` arguments rooted at local variables."
                    ))),
                }
            }
            HirExpr::Ident { name, .. } => Ok(name),
            _ => Err(EvalError::Runtime(format!(
                "interpreter P0 only supports mutable `{name}` arguments rooted at local variables."
            ))),
        }
    }

    fn construct_value(
        &mut self,
        name: &str,
        args: &[HirCallArg],
    ) -> Result<Option<Value>, EvalError> {
        if matches!(name, "Some" | "Ok" | "Err") {
            let value = self.eval_first_arg(args)?;
            return Ok(Some(Value::Variant {
                name: name.to_string(),
                fields: BTreeMap::from([("value".to_string(), value)]),
            }));
        }
        if name == "None" {
            return Ok(Some(Value::Variant {
                name: "None".to_string(),
                fields: BTreeMap::new(),
            }));
        }
        // Struct field declaration order is not retained by HIR, so the ordered
        // field list comes from the parsed AST declarations.
        for item in &self.program.items {
            match item {
                Item::Type(type_decl) if type_decl.name == name => {
                    return Ok(Some(Value::Struct {
                        name: name.to_string(),
                        fields: self.eval_constructor_fields(&type_decl.fields, args)?,
                    }));
                }
                Item::SumType(sum) => {
                    if let Some(variant) = sum.variants.iter().find(|variant| variant.name == name)
                    {
                        return Ok(Some(Value::Variant {
                            name: name.to_string(),
                            fields: self.eval_constructor_fields(&variant.fields, args)?,
                        }));
                    }
                }
                _ => {}
            }
        }
        Ok(None)
    }

    fn eval_constructor_fields(
        &mut self,
        fields: &[crate::syntax::ast::FieldDecl],
        args: &[HirCallArg],
    ) -> Result<BTreeMap<String, Value>, EvalError> {
        let mut values = BTreeMap::new();
        for (index, field) in fields.iter().enumerate() {
            let arg = args
                .iter()
                .find(|arg| arg.name.as_deref() == Some(field.name.as_str()))
                .or_else(|| args.get(index))
                .ok_or_else(|| EvalError::Runtime(format!("missing field `{}`.", field.name)))?;
            values.insert(field.name.clone(), self.eval_expr(&arg.value)?);
        }
        Ok(values)
    }

    fn eval_match(&mut self, value: Value, arms: &[HirMatchArm]) -> Result<Control, EvalError> {
        for arm in arms {
            let mut bindings = HashMap::new();
            if pattern_matches(&arm.pattern, &value, &mut bindings) {
                self.scopes.push(bindings);
                let guard_matches = arm
                    .guard
                    .as_ref()
                    .map(|guard| self.eval_expr(guard).and_then(expect_bool))
                    .transpose()?
                    .unwrap_or(true);
                if guard_matches {
                    let result = self.eval_block(&arm.body);
                    self.scopes.pop();
                    return result;
                }
                self.scopes.pop();
            }
        }
        Err(EvalError::Runtime(
            "match reached no arm; checker should have rejected this.".to_string(),
        ))
    }

    fn bind(&mut self, name: String, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    fn assign(&mut self, name: &str, value: Value) -> Result<(), EvalError> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return Ok(());
            }
        }
        Err(EvalError::Runtime(format!("unknown local `{name}`.")))
    }

    fn assign_list_index(&mut self, name: &str, index: i64, value: Value) -> Result<(), EvalError> {
        let mut list = expect_list(self.lookup(name)?)?;
        if index < 0 {
            return Err(EvalError::Runtime(format!(
                "List index `{index}` is negative."
            )));
        }
        let Some(slot) = list.get_mut(index as usize) else {
            return Err(EvalError::Runtime(format!(
                "List index `{index}` is out of bounds."
            )));
        };
        *slot = value;
        self.assign(name, Value::List(list))
    }

    /// Evaluate a bare identifier. Built-in literals first, then a local
    /// binding (which shadows), then a nullary constructor — `None`, an empty
    /// sum variant, or a fieldless struct are written without parentheses, so
    /// they reach here as identifiers rather than calls.
    fn eval_ident(&mut self, name: &str) -> Result<Value, EvalError> {
        match name {
            "Unit" => return Ok(Value::Unit),
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            _ => {}
        }
        if let Some(value) = self.lookup_opt(name) {
            return Ok(value);
        }
        if let Some(value) = self.construct_value(name, &[])? {
            return Ok(value);
        }
        Err(EvalError::Runtime(format!("unknown local `{name}`.")))
    }

    fn lookup(&self, name: &str) -> Result<Value, EvalError> {
        self.lookup_opt(name)
            .ok_or_else(|| EvalError::Runtime(format!("unknown local `{name}`.")))
    }

    fn lookup_opt(&self, name: &str) -> Option<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Some(value.clone());
            }
        }
        None
    }
}

fn read_field(base: &Value, name: &str) -> Result<Value, EvalError> {
    match base {
        Value::Struct { fields, .. } | Value::Variant { fields, .. } => fields
            .get(name)
            .cloned()
            .ok_or_else(|| EvalError::Runtime(format!("unknown field `{name}`."))),
        _ => Err(EvalError::Runtime(format!(
            "cannot read field `{name}` from scalar value."
        ))),
    }
}

fn eval_index(base: Value, index: Value) -> Result<Value, EvalError> {
    match base {
        Value::List(items) => list_get(items, expect_int(index)?),
        Value::Json(json) => json_array_get(json, expect_int(index)?),
        _ => Err(EvalError::Runtime(
            "index expression expects a List or Json value.".to_string(),
        )),
    }
}

fn list_get(items: Vec<Value>, index: i64) -> Result<Value, EvalError> {
    if index < 0 {
        return Err(EvalError::Runtime(format!(
            "List index `{index}` is negative."
        )));
    }
    items
        .get(index as usize)
        .cloned()
        .ok_or_else(|| EvalError::Runtime(format!("List index `{index}` is out of bounds.")))
}

fn list_slice(items: Vec<Value>, start: i64, len: i64) -> Vec<Value> {
    let start = start.max(0) as usize;
    if start >= items.len() {
        return Vec::new();
    }
    let end = start.saturating_add(len.max(0) as usize).min(items.len());
    items[start..end].to_vec()
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn diff_unified_string(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let old_lines = split_text_lines(old);
    let new_lines = split_text_lines(new);
    let mut out = Vec::new();
    out.push("--- old".to_string());
    out.push("+++ new".to_string());
    out.push(format!(
        "@@ -1,{} +1,{} @@",
        old_lines.len(),
        new_lines.len()
    ));
    for line in &old_lines {
        out.push(format!("-{line}"));
    }
    for line in &new_lines {
        out.push(format!("+{line}"));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn patch_apply_text_string(original: &str, patch: &str) -> Result<String, String> {
    let original_had_trailing_newline = original.ends_with('\n');
    let original_lines = split_text_lines(original);
    let patch_lines = split_text_lines(patch);
    if patch_lines.is_empty() {
        return Ok(original.to_string());
    }

    let mut output = Vec::new();
    let mut original_index = 0usize;
    let mut patch_index = 0usize;

    while patch_index < patch_lines.len() {
        let line = &patch_lines[patch_index];
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            patch_index += 1;
            continue;
        }
        if !line.starts_with("@@ ") {
            return Err(format!("expected unified diff hunk header, got `{line}`"));
        }
        let old_start = parse_patch_hunk_old_start(line)?;
        let target_index = old_start.saturating_sub(1);
        if target_index < original_index || target_index > original_lines.len() {
            return Err("patch hunk applies outside the original text".to_string());
        }
        while original_index < target_index {
            output.push(original_lines[original_index].clone());
            original_index += 1;
        }
        patch_index += 1;
        while patch_index < patch_lines.len() {
            let hunk_line = &patch_lines[patch_index];
            if hunk_line.starts_with("@@ ") {
                break;
            }
            if hunk_line.starts_with("--- ") || hunk_line.starts_with("+++ ") {
                break;
            }
            let Some(prefix) = hunk_line.chars().next() else {
                return Err("empty patch hunk line".to_string());
            };
            let value = hunk_line[1..].to_string();
            match prefix {
                ' ' => {
                    let Some(original_line) = original_lines.get(original_index) else {
                        return Err("patch context extends past original text".to_string());
                    };
                    if original_line != &value {
                        return Err(format!(
                            "patch context mismatch: expected `{}`, got `{value}`",
                            original_line
                        ));
                    }
                    output.push(original_line.clone());
                    original_index += 1;
                }
                '-' => {
                    let Some(original_line) = original_lines.get(original_index) else {
                        return Err("patch removal extends past original text".to_string());
                    };
                    if original_line != &value {
                        return Err(format!(
                            "patch removal mismatch: expected `{}`, got `{value}`",
                            original_line
                        ));
                    }
                    original_index += 1;
                }
                '+' => output.push(value),
                '\\' => {}
                _ => return Err(format!("unsupported patch hunk prefix `{prefix}`")),
            }
            patch_index += 1;
        }
    }

    while original_index < original_lines.len() {
        output.push(original_lines[original_index].clone());
        original_index += 1;
    }

    let mut text = output.join("\n");
    if original_had_trailing_newline || patch.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn split_text_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let trimmed = value.strip_suffix('\n').unwrap_or(value);
    trimmed.split('\n').map(ToString::to_string).collect()
}

fn parse_patch_hunk_old_start(line: &str) -> Result<usize, String> {
    let mut parts = line.split_whitespace();
    let Some("@@") = parts.next() else {
        return Err("malformed hunk header".to_string());
    };
    let Some(old_part) = parts.next() else {
        return Err("hunk header missing old range".to_string());
    };
    if !old_part.starts_with('-') {
        return Err("hunk header old range must start with `-`".to_string());
    }
    old_part[1..]
        .split(',')
        .next()
        .unwrap_or("1")
        .parse::<usize>()
        .map_err(|_| "hunk header old range start must be an integer".to_string())
}

fn map_get(entries: &[(Value, Value)], key: &Value) -> Option<Value> {
    entries
        .iter()
        .find_map(|(entry_key, value)| (entry_key == key).then(|| value.clone()))
}

fn map_insert(entries: &mut Vec<(Value, Value)>, key: Value, value: Value) -> Option<Value> {
    if let Some((_, existing)) = entries.iter_mut().find(|(entry_key, _)| entry_key == &key) {
        return Some(std::mem::replace(existing, value));
    }
    entries.push((key, value));
    None
}

fn map_remove(entries: &mut Vec<(Value, Value)>, key: &Value) -> Option<Value> {
    let index = entries.iter().position(|(entry_key, _)| entry_key == key)?;
    Some(entries.remove(index).1)
}

fn path_join_string(base: &str, child: &str) -> String {
    Path::new(base).join(child).to_string_lossy().to_string()
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn path_normalize_string(path: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized.to_string_lossy().to_string()
}

fn path_safe_relative_string(value: &str) -> Result<String, String> {
    let path = Path::new(value);
    if value.is_empty() {
        return Err("path must be non-empty".to_string());
    }
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                return Err("parent-directory traversal is not allowed".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths are not allowed".to_string());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("path must name a relative file or directory".to_string());
    }
    Ok(normalized.to_string_lossy().to_string())
}

fn path_resolve_relative_string(root: &str, relative: &str) -> Result<String, String> {
    let relative = path_safe_relative_string(relative)?;
    let resolved = path_normalize_string(&path_join_string(root, &relative));
    if !Path::new(&resolved).starts_with(Path::new(root)) {
        return Err("resolved path escapes the workspace root".to_string());
    }
    Ok(resolved)
}

fn sort_map_entries(entries: &mut [(Value, Value)]) -> Result<(), EvalError> {
    for (key, _) in entries.iter() {
        value_cmp(key, key)?;
    }
    let mut sorted = entries.to_vec();
    sorted.sort_by(|(left, _), (right, _)| value_cmp(left, right).unwrap_or(Ordering::Equal));
    for pair in sorted.windows(2) {
        value_cmp(&pair[0].0, &pair[1].0)?;
    }
    entries.clone_from_slice(&sorted);
    Ok(())
}

fn set_insert(items: &mut Vec<Value>, value: Value) -> bool {
    if items.iter().any(|item| item == &value) {
        return false;
    }
    items.push(value);
    true
}

fn set_remove(items: &mut Vec<Value>, value: &Value) -> bool {
    let Some(index) = items.iter().position(|item| item == value) else {
        return false;
    };
    items.remove(index);
    true
}

fn sort_values(items: &mut [Value]) -> Result<(), EvalError> {
    for item in items.iter() {
        value_cmp(item, item)?;
    }
    let mut sorted = items.to_vec();
    sorted.sort_by(|left, right| value_cmp(left, right).unwrap_or(Ordering::Equal));
    for pair in sorted.windows(2) {
        value_cmp(&pair[0], &pair[1])?;
    }
    items.clone_from_slice(&sorted);
    Ok(())
}

fn value_cmp(left: &Value, right: &Value) -> Result<Ordering, EvalError> {
    match (left, right) {
        (Value::Unit, Value::Unit) => Ok(Ordering::Equal),
        (Value::Int(left), Value::Int(right)) => Ok(left.cmp(right)),
        (Value::Bool(left), Value::Bool(right)) => Ok(left.cmp(right)),
        (Value::String(left), Value::String(right)) => Ok(left.cmp(right)),
        (Value::Char(left), Value::Char(right)) => Ok(left.cmp(right)),
        _ => Err(EvalError::Runtime(format!(
            "SortedSet value is not orderable: `{}`.",
            left.display()
        ))),
    }
}

fn json_array_get(json: serde_json::Value, index: i64) -> Result<Value, EvalError> {
    if index < 0 {
        return Err(EvalError::Runtime(format!(
            "JSON array index `{index}` is negative."
        )));
    }
    let serde_json::Value::Array(items) = json else {
        return Err(EvalError::Runtime(
            "JSON value is not an array.".to_string(),
        ));
    };
    items
        .get(index as usize)
        .cloned()
        .map(Value::Json)
        .ok_or_else(|| EvalError::Runtime(format!("JSON array index `{index}` is out of bounds.")))
}

fn json_array_get_result(json: serde_json::Value, index: i64) -> Result<Value, Value> {
    if index < 0 {
        return Err(json_error(format!(
            "JSON array index `{index}` is negative"
        )));
    }
    let serde_json::Value::Array(items) = json else {
        return Err(json_error("JSON value is not an array"));
    };
    items
        .get(index as usize)
        .cloned()
        .map(Value::Json)
        .ok_or_else(|| json_error(format!("JSON array index `{index}` is out of bounds")))
}

fn json_field(json: serde_json::Value, name: &str) -> Result<Value, Value> {
    let serde_json::Value::Object(fields) = json else {
        return Err(json_error("JSON value is not an object"));
    };
    fields
        .get(name)
        .cloned()
        .map(Value::Json)
        .ok_or_else(|| json_error(format!("missing JSON field `{name}`")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonPathPart {
    Field(String),
    Index(usize),
}

fn parse_json_path(path: &str) -> Result<Vec<JsonPathPart>, Value> {
    let original_path = path;
    let path = path.strip_prefix("$.").unwrap_or(path);
    let path = path.strip_prefix('$').unwrap_or(path);
    if path.is_empty() {
        return Ok(Vec::new());
    }

    let chars = path.chars().collect::<Vec<_>>();
    let mut parts = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '.' {
            index += 1;
            continue;
        }

        if chars[index] == '[' {
            index += 1;
            let start = index;
            while index < chars.len() && chars[index] != ']' {
                index += 1;
            }
            if index == chars.len() {
                return Err(json_error(format!(
                    "JSON path `{path}` has an unterminated array index"
                )));
            }
            let raw_index = chars[start..index].iter().collect::<String>();
            let item_index = raw_index.parse::<usize>().map_err(|_| {
                json_error(format!(
                    "JSON path `{path}` has invalid array index `{raw_index}`"
                ))
            })?;
            parts.push(JsonPathPart::Index(item_index));
            index += 1;
            continue;
        }

        let start = index;
        while index < chars.len() && chars[index] != '.' && chars[index] != '[' {
            index += 1;
        }
        let field = chars[start..index].iter().collect::<String>();
        if field.is_empty() {
            return Err(json_error(format!(
                "JSON path `{path}` contains an empty field"
            )));
        }
        parts.push(JsonPathPart::Field(field));
    }

    let _ = original_path;
    Ok(parts)
}

fn json_value_at(json: serde_json::Value, path: &str) -> Result<Value, Value> {
    let mut current = &json;
    for part in parse_json_path(path)? {
        match part {
            JsonPathPart::Field(name) => {
                let Some(next) = current.get(&name) else {
                    return Err(json_error(format!(
                        "missing JSON field `{name}` at path `{path}`"
                    )));
                };
                current = next;
            }
            JsonPathPart::Index(index) => {
                let Some(items) = current.as_array() else {
                    return Err(json_error(format!(
                        "JSON path `{path}` expected an array before index `{index}`"
                    )));
                };
                let Some(next) = items.get(index) else {
                    return Err(json_error(format!(
                        "JSON array index `{index}` is out of bounds at path `{path}`"
                    )));
                };
                current = next;
            }
        }
    }
    Ok(Value::Json(current.clone()))
}

fn json_optional_path(
    json: serde_json::Value,
    path: &str,
    wrap: impl FnOnce(serde_json::Value) -> Value,
) -> Value {
    match json_value_at(json, path).and_then(value_json) {
        Ok(value) if value.is_null() => value_none(),
        Ok(value) => value_some(wrap(value)),
        Err(_) => value_none(),
    }
}

fn json_optional_typed_path(
    json: serde_json::Value,
    path: &str,
    convert: impl FnOnce(serde_json::Value) -> Result<Value, Value>,
) -> Result<Value, Value> {
    match json_value_at(json, path).and_then(value_json) {
        Ok(value) if value.is_null() => Ok(value_none()),
        Ok(value) => convert(value).map(value_some),
        Err(_) => Ok(value_none()),
    }
}

fn json_optional_field(
    json: serde_json::Value,
    name: &str,
    wrap: impl FnOnce(serde_json::Value) -> Value,
) -> Value {
    let serde_json::Value::Object(fields) = json else {
        return value_none();
    };
    match fields.get(name) {
        Some(value) if value.is_null() => value_none(),
        Some(value) => value_some(wrap(value.clone())),
        None => value_none(),
    }
}

fn json_optional_typed_field(
    json: serde_json::Value,
    name: &str,
    convert: impl FnOnce(serde_json::Value) -> Result<Value, Value>,
) -> Result<Value, Value> {
    let serde_json::Value::Object(fields) = json else {
        return Err(json_error("JSON value is not an object"));
    };
    match fields.get(name) {
        Some(value) if value.is_null() => Ok(value_none()),
        Some(value) => convert(value.clone()).map(value_some),
        None => Ok(value_none()),
    }
}

fn json_array(json: &serde_json::Value) -> Result<&Vec<serde_json::Value>, Value> {
    json.as_array()
        .ok_or_else(|| json_error("JSON value is not an array"))
}

fn json_array_strings(json: serde_json::Value) -> Result<Vec<Value>, Value> {
    let serde_json::Value::Array(items) = json else {
        return Err(json_error("JSON value is not an array"));
    };
    let mut strings = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let serde_json::Value::String(text) = item else {
            return Err(json_error(format!(
                "JSON array item `{index}` is not a string"
            )));
        };
        strings.push(Value::String(text));
    }
    Ok(strings)
}

fn json_array_ints(json: serde_json::Value) -> Result<Vec<Value>, Value> {
    let serde_json::Value::Array(items) = json else {
        return Err(json_error("JSON value is not an array"));
    };
    let mut numbers = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let Some(number) = item.as_i64() else {
            return Err(json_error(format!(
                "JSON array item `{index}` is not an integer"
            )));
        };
        numbers.push(Value::Int(number));
    }
    Ok(numbers)
}

fn json_array_bools(json: serde_json::Value) -> Result<Vec<Value>, Value> {
    let serde_json::Value::Array(items) = json else {
        return Err(json_error("JSON value is not an array"));
    };
    let mut flags = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let Some(flag) = item.as_bool() else {
            return Err(json_error(format!(
                "JSON array item `{index}` is not a boolean"
            )));
        };
        flags.push(Value::Bool(flag));
    }
    Ok(flags)
}

#[derive(Debug, Clone, Copy)]
enum JsonArrayStringMatch {
    Exact,
    Substring,
    Prefix,
}

fn json_array_contains_string(
    json: serde_json::Value,
    needle: &str,
    mode: JsonArrayStringMatch,
) -> Result<Value, Value> {
    let serde_json::Value::Array(items) = json else {
        return Err(json_error("JSON value is not an array"));
    };
    Ok(Value::Bool(items.iter().any(|value| {
        value.as_str().is_some_and(|item| match mode {
            JsonArrayStringMatch::Exact => item == needle,
            JsonArrayStringMatch::Substring => item.contains(needle),
            JsonArrayStringMatch::Prefix => item.starts_with(needle),
        })
    })))
}

fn json_object(
    json: &serde_json::Value,
) -> Result<&serde_json::Map<String, serde_json::Value>, Value> {
    json.as_object()
        .ok_or_else(|| json_error("JSON value is not an object"))
}

fn json_as_string_value(json: serde_json::Value) -> Result<Value, Value> {
    json.as_str()
        .map(|value| Value::String(value.to_string()))
        .ok_or_else(|| json_error("JSON value is not a string"))
}

fn json_as_int_value(json: serde_json::Value) -> Result<Value, Value> {
    json.as_i64()
        .map(Value::Int)
        .ok_or_else(|| json_error("JSON value is not an integer"))
}

fn json_as_bool_value(json: serde_json::Value) -> Result<Value, Value> {
    json.as_bool()
        .map(Value::Bool)
        .ok_or_else(|| json_error("JSON value is not a boolean"))
}

fn value_json(value: Value) -> Result<serde_json::Value, Value> {
    match value {
        Value::Json(value) => Ok(value),
        other => Err(json_error(format!(
            "expected JsonValue, got `{}`",
            other.display()
        ))),
    }
}

fn parse_json_value(text: &str) -> Result<serde_json::Value, Value> {
    serde_json::from_str(text).map_err(|error| json_error(error.to_string()))
}

fn value_to_json_literal(value: Value) -> Result<serde_json::Value, EvalError> {
    match value {
        Value::Unit => Ok(serde_json::Value::Null),
        Value::Int(value) => Ok(serde_json::Value::Number(value.into())),
        Value::Bool(value) => Ok(serde_json::Value::Bool(value)),
        Value::String(value) => Ok(serde_json::Value::String(value)),
        Value::Json(value) => Ok(value),
        Value::List(items) => items
            .into_iter()
            .map(value_to_json_literal)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        Value::Map(entries) => {
            let mut object = serde_json::Map::new();
            for (key, value) in entries {
                let key = expect_string(key)?;
                object.insert(key, value_to_json_literal(value)?);
            }
            Ok(serde_json::Value::Object(object))
        }
        other => Err(EvalError::Runtime(format!(
            "cannot convert `{}` to a JSON literal.",
            other.display()
        ))),
    }
}

fn json_quote_string(value: &str) -> Result<String, EvalError> {
    serde_json::to_string(value).map_err(|error| EvalError::Runtime(error.to_string()))
}

fn bytes_slice(value: Vec<u8>, start: i64, len: i64) -> Vec<u8> {
    let start = start.max(0) as usize;
    if start >= value.len() {
        return Vec::new();
    }
    let len = len.max(0) as usize;
    let end = start.saturating_add(len).min(value.len());
    value[start..end].to_vec()
}

fn string_slice(value: String, start: i64, len: i64) -> String {
    let byte_start = clamp_to_char_boundary(&value, start.max(0) as usize);
    let requested_end = byte_start.saturating_add(len.max(0) as usize);
    let byte_end = clamp_to_char_boundary(&value, requested_end.min(value.len()));
    value[byte_start..byte_end].to_string()
}

fn clamp_to_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn strip_generic_args(value: &str) -> &str {
    value.split_once('<').map_or(value, |(name, _)| name)
}

fn decode_string_token(value: &str) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            Some('\\') => decoded.push('\\'),
            Some('"') => decoded.push('"'),
            Some('0') => decoded.push('\0'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

fn json_error(message: impl Into<String>) -> Value {
    Value::Struct {
        name: "JsonError".to_string(),
        fields: BTreeMap::from([("message".to_string(), Value::String(message.into()))]),
    }
}

fn json_result(value: Result<serde_json::Value, String>) -> Value {
    result_value(value.map(Value::Json).map_err(json_error))
}

fn yaml_parse_result(text: &str) -> Value {
    json_result(yaml_parse_json(text))
}

fn yaml_parse_json(text: &str) -> Result<serde_json::Value, String> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|error| error.to_string())?;
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn regex_value(pattern: impl Into<String>) -> Value {
    Value::Struct {
        name: "Regex".to_string(),
        fields: BTreeMap::from([("pattern".to_string(), Value::String(pattern.into()))]),
    }
}

fn regex_error(message: impl Into<String>) -> Value {
    Value::Struct {
        name: "RegexError".to_string(),
        fields: BTreeMap::from([("message".to_string(), Value::String(message.into()))]),
    }
}

fn http_error(message: impl Into<String>) -> Value {
    Value::Struct {
        name: "HttpError".to_string(),
        fields: BTreeMap::from([("message".to_string(), Value::String(message.into()))]),
    }
}

struct HttpRequestState {
    method: String,
    url: String,
    body: String,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
    header_count: i64,
}

impl HttpRequestState {
    fn to_value(&self) -> Value {
        http_request_value(
            self.method.clone(),
            self.url.clone(),
            self.body.clone(),
            self.timeout_ms,
            self.attempts,
            self.backoff_ms,
            self.header_count,
        )
    }
}

fn http_request_value(
    method: impl Into<String>,
    url: impl Into<String>,
    body: impl Into<String>,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
    header_count: i64,
) -> Value {
    Value::Struct {
        name: "HttpRequest".to_string(),
        fields: BTreeMap::from([
            ("method".to_string(), Value::String(method.into())),
            ("url".to_string(), Value::String(url.into())),
            ("body".to_string(), Value::String(body.into())),
            ("timeout_ms".to_string(), Value::Int(timeout_ms)),
            ("attempts".to_string(), Value::Int(attempts)),
            ("backoff_ms".to_string(), Value::Int(backoff_ms)),
            ("header_count".to_string(), Value::Int(header_count)),
        ]),
    }
}

fn config_error(error: std::io::Error) -> Value {
    Value::Struct {
        name: "ConfigError".to_string(),
        fields: BTreeMap::from([("message".to_string(), Value::String(error.to_string()))]),
    }
}

fn config_name_from_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("default")
        .to_string()
}

fn rules_from_text(text: &str) -> Vec<Value> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|name| Value::Struct {
            name: "Rule".to_string(),
            fields: BTreeMap::from([("name".to_string(), Value::String(name.to_string()))]),
        })
        .collect()
}

fn config_value(name: impl Into<String>) -> Value {
    Value::Struct {
        name: "ConfigValue".to_string(),
        fields: BTreeMap::from([("name".to_string(), Value::String(name.into()))]),
    }
}

fn config_store_value(name: impl Into<String>) -> Value {
    Value::Struct {
        name: "ConfigStore".to_string(),
        fields: BTreeMap::from([("name".to_string(), Value::String(name.into()))]),
    }
}

fn image_cache_value(capacity: i64, len: i64) -> Value {
    Value::Struct {
        name: "ImageCache".to_string(),
        fields: BTreeMap::from([
            ("capacity".to_string(), Value::Int(capacity)),
            ("len".to_string(), Value::Int(len)),
        ]),
    }
}

fn tempdir_value(path: impl Into<String>) -> Value {
    Value::Struct {
        name: "TempDir".to_string(),
        fields: BTreeMap::from([("path".to_string(), Value::String(path.into()))]),
    }
}

fn tempdir_new_value(parent: PathBuf) -> Result<Value, Value> {
    let seed = clock_system_unix_ms();
    for attempt in 0..100 {
        let path = parent.join(format!("rsscript-{}-{seed}-{attempt}", std::process::id()));
        match std::fs::create_dir_all(&path) {
            Ok(()) => return Ok(tempdir_value(path_to_string(&path))),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(file_error(error)),
        }
    }
    Err(file_error(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate unique TempDir path",
    )))
}

fn config_rules_value(name: impl Into<String>, rule_count: i64) -> Value {
    Value::Struct {
        name: "Config".to_string(),
        fields: BTreeMap::from([
            ("name".to_string(), Value::String(name.into())),
            ("rule_count".to_string(), Value::Int(rule_count)),
        ]),
    }
}

fn global_config_value(rule_count: i64) -> Value {
    Value::Struct {
        name: "GlobalConfig".to_string(),
        fields: BTreeMap::from([("rule_count".to_string(), Value::Int(rule_count))]),
    }
}

fn clock_system_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_millis() as i64
}

fn instant_value(unix_ms: i64) -> Value {
    Value::Struct {
        name: "Instant".to_string(),
        fields: BTreeMap::from([("unix_ms".to_string(), Value::Int(unix_ms))]),
    }
}

fn deadline_after_ms(ms: i64) -> i64 {
    clock_system_unix_ms().saturating_add(ms.max(0))
}

fn deadline_value(unix_ms: i64) -> Value {
    Value::Struct {
        name: "Deadline".to_string(),
        fields: BTreeMap::from([("unix_ms".to_string(), Value::Int(unix_ms))]),
    }
}

fn counter_value(value: i64) -> Value {
    Value::Struct {
        name: "Counter".to_string(),
        fields: BTreeMap::from([("value".to_string(), Value::Int(value))]),
    }
}

fn expect_counter_value(value: Value) -> Result<i64, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "Counter" => fields
            .remove("value")
            .ok_or_else(|| EvalError::Runtime("Counter value is missing.".to_string()))
            .and_then(expect_int),
        other => Err(EvalError::Runtime(format!(
            "expected Counter, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_instant_unix_ms(value: Value) -> Result<i64, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "Instant" => fields
            .remove("unix_ms")
            .ok_or_else(|| EvalError::Runtime("Instant value is missing unix_ms.".to_string()))
            .and_then(expect_int),
        other => Err(EvalError::Runtime(format!(
            "expected Instant, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_deadline_unix_ms(value: Value) -> Result<i64, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "Deadline" => fields
            .remove("unix_ms")
            .ok_or_else(|| EvalError::Runtime("Deadline value is missing unix_ms.".to_string()))
            .and_then(expect_int),
        other => Err(EvalError::Runtime(format!(
            "expected Deadline, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_config_value_name(value: Value) -> Result<String, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "ConfigValue" => fields
            .remove("name")
            .ok_or_else(|| EvalError::Runtime("ConfigValue name is missing.".to_string()))
            .and_then(expect_string),
        other => Err(EvalError::Runtime(format!(
            "expected ConfigValue, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_config_store_name(value: Value) -> Result<String, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "ConfigStore" => fields
            .remove("name")
            .ok_or_else(|| EvalError::Runtime("ConfigStore name is missing.".to_string()))
            .and_then(expect_string),
        other => Err(EvalError::Runtime(format!(
            "expected ConfigStore, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_image_cache_len(value: Value) -> Result<i64, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "ImageCache" => fields
            .remove("len")
            .ok_or_else(|| EvalError::Runtime("ImageCache len is missing.".to_string()))
            .and_then(expect_int),
        other => Err(EvalError::Runtime(format!(
            "expected ImageCache, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_tempdir_path(value: Value) -> Result<String, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "TempDir" => fields
            .remove("path")
            .ok_or_else(|| EvalError::Runtime("TempDir path is missing.".to_string()))
            .and_then(expect_string),
        other => Err(EvalError::Runtime(format!(
            "expected TempDir, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_http_request(value: Value) -> Result<HttpRequestState, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "HttpRequest" => Ok(HttpRequestState {
            method: fields
                .remove("method")
                .ok_or_else(|| EvalError::Runtime("HttpRequest method is missing.".to_string()))
                .and_then(expect_string)?,
            url: fields
                .remove("url")
                .ok_or_else(|| EvalError::Runtime("HttpRequest url is missing.".to_string()))
                .and_then(expect_string)?,
            body: fields
                .remove("body")
                .ok_or_else(|| EvalError::Runtime("HttpRequest body is missing.".to_string()))
                .and_then(expect_string)?,
            timeout_ms: fields
                .remove("timeout_ms")
                .ok_or_else(|| EvalError::Runtime("HttpRequest timeout_ms is missing.".to_string()))
                .and_then(expect_int)?,
            attempts: fields
                .remove("attempts")
                .ok_or_else(|| EvalError::Runtime("HttpRequest attempts is missing.".to_string()))
                .and_then(expect_int)?,
            backoff_ms: fields
                .remove("backoff_ms")
                .ok_or_else(|| EvalError::Runtime("HttpRequest backoff_ms is missing.".to_string()))
                .and_then(expect_int)?,
            header_count: fields
                .remove("header_count")
                .ok_or_else(|| {
                    EvalError::Runtime("HttpRequest header_count is missing.".to_string())
                })
                .and_then(expect_int)?,
        }),
        other => Err(EvalError::Runtime(format!(
            "expected HttpRequest, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_config_rule_count(value: Value) -> Result<i64, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "Config" => fields
            .remove("rule_count")
            .ok_or_else(|| EvalError::Runtime("Config rule_count is missing.".to_string()))
            .and_then(expect_int),
        other => Err(EvalError::Runtime(format!(
            "expected Config, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_global_config_rule_count(value: Value) -> Result<i64, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "GlobalConfig" => fields
            .remove("rule_count")
            .ok_or_else(|| EvalError::Runtime("GlobalConfig rule_count is missing.".to_string()))
            .and_then(expect_int),
        other => Err(EvalError::Runtime(format!(
            "expected GlobalConfig, got `{}`.",
            other.display()
        ))),
    }
}

fn file_error(error: std::io::Error) -> Value {
    Value::Struct {
        name: "FileError".to_string(),
        fields: BTreeMap::from([
            (
                "kind".to_string(),
                Value::String(format!("{:?}", error.kind())),
            ),
            ("message".to_string(), Value::String(error.to_string())),
        ]),
    }
}

fn csv_error(message: impl Into<String>) -> Value {
    Value::Struct {
        name: "CsvError".to_string(),
        fields: BTreeMap::from([("message".to_string(), Value::String(message.into()))]),
    }
}

fn csv_error_from_io(error: std::io::Error) -> Value {
    csv_error(error.to_string())
}

fn row_buffer_value(bytes: Vec<u8>) -> Value {
    Value::Struct {
        name: "RowBuffer".to_string(),
        fields: BTreeMap::from([("bytes".to_string(), Value::Bytes(bytes))]),
    }
}

fn expect_row_buffer_bytes(value: Value) -> Result<Vec<u8>, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "RowBuffer" => fields
            .remove("bytes")
            .ok_or_else(|| EvalError::Runtime("RowBuffer value is missing bytes.".to_string()))
            .and_then(expect_bytes),
        other => Err(EvalError::Runtime(format!(
            "expected RowBuffer, got `{}`.",
            other.display()
        ))),
    }
}

fn row_value(fields: Vec<String>) -> Value {
    Value::Struct {
        name: "Row".to_string(),
        fields: BTreeMap::from([(
            "fields".to_string(),
            Value::List(fields.into_iter().map(Value::String).collect()),
        )]),
    }
}

fn expect_row_fields(value: Value) -> Result<Vec<String>, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "Row" => fields
            .remove("fields")
            .ok_or_else(|| EvalError::Runtime("Row value is missing fields.".to_string()))
            .and_then(expect_string_list),
        other => Err(EvalError::Runtime(format!(
            "expected Row, got `{}`.",
            other.display()
        ))),
    }
}

fn csv_parse_row_value(bytes: &[u8]) -> Result<Value, Value> {
    let text = std::str::from_utf8(bytes).map_err(|error| csv_error(error.to_string()))?;
    let Some(line) = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .nth(1)
        .or_else(|| text.lines().map(str::trim).find(|line| !line.is_empty()))
    else {
        return Err(csv_error("CSV buffer is empty"));
    };
    Ok(row_value(
        line.split(',')
            .map(|field| field.trim().to_string())
            .collect(),
    ))
}

fn row_field_string_value(fields: Vec<String>, index: i64) -> Result<Value, Value> {
    let index = usize::try_from(index).map_err(|_| csv_error("negative CSV field index"))?;
    fields
        .get(index)
        .cloned()
        .map(Value::String)
        .ok_or_else(|| csv_error(format!("CSV field index `{index}` is out of bounds")))
}

struct FileState {
    path: String,
    mode: String,
    cursor: u64,
}

impl FileState {
    fn to_value(&self) -> Value {
        file_value(self.path.clone(), self.mode.clone(), self.cursor)
    }
}

fn file_value(path: impl Into<String>, mode: impl Into<String>, cursor: u64) -> Value {
    Value::Struct {
        name: "File".to_string(),
        fields: BTreeMap::from([
            ("path".to_string(), Value::String(path.into())),
            ("mode".to_string(), Value::String(mode.into())),
            ("cursor".to_string(), Value::Int(cursor as i64)),
        ]),
    }
}

fn expect_file(value: Value) -> Result<FileState, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "File" => {
            let path = fields
                .remove("path")
                .ok_or_else(|| EvalError::Runtime("File value is missing path.".to_string()))?;
            let mode = fields
                .remove("mode")
                .ok_or_else(|| EvalError::Runtime("File value is missing mode.".to_string()))?;
            let cursor = fields
                .remove("cursor")
                .ok_or_else(|| EvalError::Runtime("File value is missing cursor.".to_string()))?;
            let cursor = expect_int(cursor)?;
            Ok(FileState {
                path: expect_string(path)?,
                mode: expect_string(mode)?,
                cursor: cursor.max(0) as u64,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected File, got `{}`.",
            other.display()
        ))),
    }
}

fn file_read_remaining(file: &mut FileState) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    if file.mode != "read" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file is not open for reading",
        ));
    }
    let mut handle = std::fs::File::open(&file.path)?;
    handle.seek(SeekFrom::Start(file.cursor))?;
    let mut bytes = Vec::new();
    handle.read_to_end(&mut bytes)?;
    file.cursor = file.cursor.saturating_add(bytes.len() as u64);
    Ok(bytes)
}

fn file_write_at_cursor(file: &mut FileState, data: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};

    if file.mode != "write" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file is not open for writing",
        ));
    }
    let mut handle = std::fs::OpenOptions::new().write(true).open(&file.path)?;
    handle.seek(SeekFrom::Start(file.cursor))?;
    handle.write_all(data)?;
    file.cursor = file.cursor.saturating_add(data.len() as u64);
    Ok(())
}

fn file_result_unit(value: std::io::Result<()>) -> Value {
    result_value(value.map(|_| Value::Unit).map_err(file_error))
}

fn file_append_result(path: String, data: &[u8]) -> Value {
    use std::io::Write;

    result_value(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(data))
            .map(|_| Value::Unit)
            .map_err(file_error),
    )
}

fn file_atomic_write_result(path: PathBuf, text: &str) -> Value {
    use std::io::Write;

    let result = (|| -> std::io::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "rsscript-atomic-write".to_string());
        let temp_path = parent.join(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp_path)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        match std::fs::rename(&temp_path, &path) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = std::fs::remove_file(&temp_path);
                Err(error)
            }
        }
    })();
    file_result_unit(result)
}

fn directory_list_files(root: &Path) -> std::io::Result<Vec<String>> {
    let mut files = Vec::new();
    collect_directory_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_directory_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<String>,
) -> std::io::Result<()> {
    if current.is_file() {
        files.push(relative_runtime_path(root, current));
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_directory_files(root, &path, files)?;
        } else if path.is_file() {
            files.push(relative_runtime_path(root, &path));
        }
    }
    Ok(())
}

fn directory_list_paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(root)? {
        paths.push(entry?.path());
    }
    paths.sort();
    Ok(paths)
}

fn relative_runtime_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn result_value(value: Result<Value, Value>) -> Value {
    match value {
        Ok(value) => value_ok(value),
        Err(error) => value_err(error),
    }
}

fn result_payload(value: Value) -> Result<Result<Value, Value>, EvalError> {
    match value {
        Value::Variant { name, mut fields } if name == "Ok" => fields
            .remove("value")
            .map(Ok)
            .ok_or_else(|| EvalError::Runtime("Ok value is missing.".to_string())),
        Value::Variant { name, mut fields } if name == "Err" => fields
            .remove("value")
            .map(Err)
            .ok_or_else(|| EvalError::Runtime("Err value is missing.".to_string())),
        other => Err(EvalError::Runtime(format!(
            "expected Result, got `{}`.",
            other.display()
        ))),
    }
}

fn value_ok(value: Value) -> Value {
    Value::Variant {
        name: "Ok".to_string(),
        fields: BTreeMap::from([("value".to_string(), value)]),
    }
}

fn value_err(value: Value) -> Value {
    Value::Variant {
        name: "Err".to_string(),
        fields: BTreeMap::from([("value".to_string(), value)]),
    }
}

/// Wrap a value as `Some(value)`, matching how `Some(...)` is constructed
/// elsewhere (payload stored under the `value` key).
fn value_some(value: Value) -> Value {
    Value::Variant {
        name: "Some".to_string(),
        fields: BTreeMap::from([("value".to_string(), value)]),
    }
}

fn value_none() -> Value {
    Value::Variant {
        name: "None".to_string(),
        fields: BTreeMap::new(),
    }
}

fn option_payload(value: Value) -> Result<Option<Value>, EvalError> {
    match value {
        Value::Variant { name, mut fields } if name == "Some" => fields
            .remove("value")
            .map(Some)
            .ok_or_else(|| EvalError::Runtime("Some value is missing.".to_string())),
        Value::Variant { name, .. } if name == "None" => Ok(None),
        other => Err(EvalError::Runtime(format!(
            "expected Option, got `{}`.",
            other.display()
        ))),
    }
}

/// Marshal a Rust `Option` (the shape the runtime helpers return) into the
/// interpreter's `Option` value, so intrinsic bodies stay one-liners.
fn value_option<T>(value: Option<T>, wrap: impl FnOnce(T) -> Value) -> Value {
    match value {
        Some(value) => value_some(wrap(value)),
        None => value_none(),
    }
}

fn pattern_matches(
    pattern: &MatchPattern,
    value: &Value,
    bindings: &mut HashMap<String, Value>,
) -> bool {
    match pattern {
        MatchPattern::Binding { name, .. } => {
            bindings.insert(name.clone(), value.clone());
            true
        }
        MatchPattern::Wildcard(_) => true,
        MatchPattern::Literal { value: literal, .. } => match (literal, value) {
            (MatchLiteral::Int(expected), Value::Int(actual)) => expected
                .parse::<i64>()
                .is_ok_and(|expected| expected == *actual),
            (MatchLiteral::String(expected), Value::String(actual)) => {
                decode_string_token(expected) == *actual
            }
            (MatchLiteral::Bool(expected), Value::Bool(actual)) => expected == actual,
            _ => false,
        },
        MatchPattern::Variant { name, binding, .. } => {
            let Value::Variant {
                name: actual_name,
                fields,
            } = value
            else {
                return false;
            };
            if name != actual_name {
                return false;
            }
            if let Some(binding) = binding {
                fields
                    .values()
                    .next()
                    .is_some_and(|value| pattern_matches(binding, value, bindings))
            } else {
                true
            }
        }
        MatchPattern::Struct { name, fields, .. } => {
            let (actual_name, actual_fields) = match value {
                Value::Struct { name, fields } | Value::Variant { name, fields } => (name, fields),
                _ => return false,
            };
            if name != actual_name {
                return false;
            }
            for field in fields {
                let Some(value) = actual_fields.get(&field.name) else {
                    return false;
                };
                if field.ignored {
                    continue;
                }
                if let Some(pattern) = &field.pattern {
                    if !pattern_matches(pattern, value, bindings) {
                        return false;
                    }
                } else if let Some(binding) = &field.binding {
                    bindings.insert(binding.clone(), value.clone());
                }
            }
            true
        }
    }
}

fn eval_binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, EvalError> {
    match op {
        BinaryOp::Add => match (left, right) {
            (Value::Int(left), Value::Int(right)) => Ok(Value::Int(left + right)),
            (Value::String(left), Value::String(right)) => {
                Ok(Value::String(format!("{left}{right}")))
            }
            _ => Err(EvalError::Runtime(
                "operator `+` expects matching Int or String operands.".to_string(),
            )),
        },
        BinaryOp::Subtract => Ok(Value::Int(expect_int(left)? - expect_int(right)?)),
        BinaryOp::Multiply => Ok(Value::Int(expect_int(left)? * expect_int(right)?)),
        BinaryOp::Divide => Ok(Value::Int(expect_int(left)? / expect_int(right)?)),
        BinaryOp::Equal => Ok(Value::Bool(left == right)),
        BinaryOp::NotEqual => Ok(Value::Bool(left != right)),
        BinaryOp::Less => Ok(Value::Bool(expect_int(left)? < expect_int(right)?)),
        BinaryOp::LessEqual => Ok(Value::Bool(expect_int(left)? <= expect_int(right)?)),
        BinaryOp::Greater => Ok(Value::Bool(expect_int(left)? > expect_int(right)?)),
        BinaryOp::GreaterEqual => Ok(Value::Bool(expect_int(left)? >= expect_int(right)?)),
        BinaryOp::LogicalAnd => Ok(Value::Bool(expect_bool(left)? && expect_bool(right)?)),
        BinaryOp::LogicalOr => Ok(Value::Bool(expect_bool(left)? || expect_bool(right)?)),
    }
}

fn expect_int(value: Value) -> Result<i64, EvalError> {
    match value {
        Value::Int(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected Int, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_bool(value: Value) -> Result<bool, EvalError> {
    match value {
        Value::Bool(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected Bool, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_string(value: Value) -> Result<String, EvalError> {
    match value {
        Value::String(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected String, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_char(value: Value) -> Result<char, EvalError> {
    match value {
        Value::Char(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected Char, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_bytes(value: Value) -> Result<Vec<u8>, EvalError> {
    match value {
        Value::Bytes(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected Bytes, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_list(value: Value) -> Result<Vec<Value>, EvalError> {
    match value {
        Value::List(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected List, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_string_list(value: Value) -> Result<Vec<String>, EvalError> {
    expect_list(value)?
        .into_iter()
        .map(expect_string)
        .collect::<Result<Vec<_>, _>>()
}

fn expect_json_list(value: Value) -> Result<Vec<serde_json::Value>, EvalError> {
    expect_list(value)?
        .into_iter()
        .map(expect_json)
        .collect::<Result<Vec<_>, _>>()
}

fn expect_map(value: Value) -> Result<Vec<(Value, Value)>, EvalError> {
    match value {
        Value::Map(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected Map, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_json(value: Value) -> Result<serde_json::Value, EvalError> {
    match value {
        Value::Json(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected JsonValue, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_regex(value: Value) -> Result<regex::Regex, EvalError> {
    match value {
        Value::Struct { name, mut fields } if name == "Regex" => {
            let pattern = fields
                .remove("pattern")
                .ok_or_else(|| EvalError::Runtime("Regex value is missing pattern.".to_string()))?;
            regex::Regex::new(&expect_string(pattern)?)
                .map_err(|error| EvalError::Runtime(error.to_string()))
        }
        other => Err(EvalError::Runtime(format!(
            "expected Regex, got `{}`.",
            other.display()
        ))),
    }
}

fn hir_stmt_name(stmt: &HirStmt) -> &'static str {
    match stmt {
        HirStmt::With { .. } => "with",
        HirStmt::For { .. } => "for",
        HirStmt::Select { .. } => "select",
        HirStmt::Unknown(_) => "unknown",
        _ => "statement",
    }
}

fn hir_expr_name(expr: &HirExpr) -> &'static str {
    match expr {
        HirExpr::ObjectLiteral { .. } => "object-literal",
        HirExpr::MapLiteral { .. } => "map-literal",
        HirExpr::ArrayLiteral { .. } => "array-literal",
        HirExpr::Index { .. } => "index",
        HirExpr::Spawn { .. } => "spawn",
        HirExpr::Await { .. } => "await",
        HirExpr::Try { .. } => "try",
        HirExpr::Closure { .. } => "closure",
        HirExpr::Unknown(_) => "unknown",
        _ => "expression",
    }
}
