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
    BufferClear,
    BufferConsume,
    BufferIsEmpty,
    BufferLen,
    BufferNew,
    BufferView,
    BufferViewToBytes,
    BytesConcat,
    BytesConsume,
    BytesFromBuffer,
    BytesFromString,
    BytesIsEmpty,
    BytesLen,
    BytesSlice,
    BytesViewStartsWith,
    BytesViewToBytes,
    CacheNew,
    ChannelErrorMessage,
    CharCompare,
    CharFromCode,
    CharIsAlphanumeric,
    CharIsAlpha,
    CharIsDigit,
    CharIsWhitespace,
    CharToCode,
    CharToString,
    DequeClear,
    DequeIsEmpty,
    DequeLen,
    DequeNew,
    DequePopBack,
    DequePopFront,
    DequePushBack,
    DequePushFront,
    DequeToList,
    DiffUnified,
    CounterAdd,
    CounterNew,
    CounterValue,
    DurationAdd,
    DurationAsMs,
    DurationAsSeconds,
    DurationMs,
    DurationSeconds,
    FileErrorMessage,
    HashSha256Bytes,
    HashSha256String,
    PathExtension,
    PathFileName,
    PathFromString,
    PathIsAbsolute,
    PathJoin,
    PathNormalize,
    PathParent,
    PathResolveRelative,
    PathSafeRelative,
    PathStartsWith,
    PathToString,
    PathWithExtension,
    IntToString,
    ListAppend,
    ListClear,
    ListConsume,
    ListContainsValue,
    ListFirst,
    ListGet,
    ListIsEmpty,
    ListJoin,
    ListLast,
    ListLen,
    ListNew,
    ListPop,
    ListPush,
    ListRemoveAt,
    ListReverse,
    ListSet,
    ListSkip,
    ListSlice,
    ListSort,
    ListTake,
    ListToJsonStrings,
    ListToJsonValues,
    MapClear,
    MapContainsKey,
    MapGet,
    MapGetOrDefault,
    MapInsert,
    MapInsertOld,
    MapIsEmpty,
    MapKeys,
    MapLen,
    MapNew,
    MapRemove,
    MapValues,
    HttpErrorMessage,
    HttpResponseIsSuccess,
    HttpResponseLines,
    HttpResponseStatus,
    HttpResponseText,
    OptionIsNone,
    OptionIsSome,
    OptionOkOr,
    OptionOr,
    OptionUnwrapOr,
    OsClose,
    PersistentMapClear,
    PersistentMapContainsKey,
    PersistentMapGet,
    PersistentMapInsert,
    PersistentMapIsEmpty,
    PersistentMapLen,
    PersistentMapNew,
    PersistentMapRemove,
    PoolErrorMessage,
    PoolStatsAvailable,
    PoolStatsCapacity,
    PoolStatsCreated,
    PoolStatsInUse,
    RowBufferNew,
    RowFieldString,
    RegexCaptures,
    RegexCompile,
    RegexErrorMessage,
    RegexFind,
    RegexIsMatch,
    RegexReplaceAll,
    RegexSplit,
    RequestNew,
    RequestPath,
    ResponseBody,
    ResponseOk,
    ResponseStatus,
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
    StringBuilderFinish,
    StringBuilderNew,
    StringBuilderPush,
    StringSafeRelative,
    StringToPath,
    StringToUrl,
    TcpErrorMessage,
    UrlFromString,
    UrlToString,
    WebSocketErrorMessage,
    WorkspaceResolve,
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
        namespace: "Buffer",
        name: "clear",
        variant: "BufferClear",
        eval_kind: InterpreterEvalKind::BufferClear,
    },
    InterpreterIntrinsicSpec {
        namespace: "Buffer",
        name: "consume",
        variant: "BufferConsume",
        eval_kind: InterpreterEvalKind::BufferConsume,
    },
    InterpreterIntrinsicSpec {
        namespace: "Buffer",
        name: "is_empty",
        variant: "BufferIsEmpty",
        eval_kind: InterpreterEvalKind::BufferIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "Buffer",
        name: "len",
        variant: "BufferLen",
        eval_kind: InterpreterEvalKind::BufferLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Buffer",
        name: "new",
        variant: "BufferNew",
        eval_kind: InterpreterEvalKind::BufferNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "Buffer",
        name: "view",
        variant: "BufferView",
        eval_kind: InterpreterEvalKind::BufferView,
    },
    InterpreterIntrinsicSpec {
        namespace: "BufferView",
        name: "is_empty",
        variant: "BufferViewIsEmpty",
        eval_kind: InterpreterEvalKind::BufferIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "BufferView",
        name: "len",
        variant: "BufferViewLen",
        eval_kind: InterpreterEvalKind::BufferLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "BufferView",
        name: "slice",
        variant: "BufferViewSlice",
        eval_kind: InterpreterEvalKind::BufferView,
    },
    InterpreterIntrinsicSpec {
        namespace: "BufferView",
        name: "to_bytes",
        variant: "BufferViewToBytes",
        eval_kind: InterpreterEvalKind::BufferViewToBytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "Bytes",
        name: "from_buffer",
        variant: "BytesFromBuffer",
        eval_kind: InterpreterEvalKind::BytesFromBuffer,
    },
    InterpreterIntrinsicSpec {
        namespace: "Bytes",
        name: "concat",
        variant: "BytesConcat",
        eval_kind: InterpreterEvalKind::BytesConcat,
    },
    InterpreterIntrinsicSpec {
        namespace: "Bytes",
        name: "consume",
        variant: "BytesConsume",
        eval_kind: InterpreterEvalKind::BytesConsume,
    },
    InterpreterIntrinsicSpec {
        namespace: "Bytes",
        name: "from_string",
        variant: "BytesFromString",
        eval_kind: InterpreterEvalKind::BytesFromString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Bytes",
        name: "is_empty",
        variant: "BytesIsEmpty",
        eval_kind: InterpreterEvalKind::BytesIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "Bytes",
        name: "len",
        variant: "BytesLen",
        eval_kind: InterpreterEvalKind::BytesLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Bytes",
        name: "slice",
        variant: "BytesSlice",
        eval_kind: InterpreterEvalKind::BytesSlice,
    },
    InterpreterIntrinsicSpec {
        namespace: "Bytes",
        name: "view",
        variant: "BytesView",
        eval_kind: InterpreterEvalKind::BytesSlice,
    },
    InterpreterIntrinsicSpec {
        namespace: "BytesView",
        name: "is_empty",
        variant: "BytesViewIsEmpty",
        eval_kind: InterpreterEvalKind::BytesIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "BytesView",
        name: "len",
        variant: "BytesViewLen",
        eval_kind: InterpreterEvalKind::BytesLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "BytesView",
        name: "slice",
        variant: "BytesViewSlice",
        eval_kind: InterpreterEvalKind::BytesSlice,
    },
    InterpreterIntrinsicSpec {
        namespace: "BytesView",
        name: "starts_with",
        variant: "BytesViewStartsWith",
        eval_kind: InterpreterEvalKind::BytesViewStartsWith,
    },
    InterpreterIntrinsicSpec {
        namespace: "BytesView",
        name: "to_bytes",
        variant: "BytesViewToBytes",
        eval_kind: InterpreterEvalKind::BytesViewToBytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "Cache",
        name: "new",
        variant: "CacheNew",
        eval_kind: InterpreterEvalKind::CacheNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "ChannelError",
        name: "message",
        variant: "ChannelErrorMessage",
        eval_kind: InterpreterEvalKind::ChannelErrorMessage,
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
        namespace: "Deque",
        name: "clear",
        variant: "DequeClear",
        eval_kind: InterpreterEvalKind::DequeClear,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deque",
        name: "is_empty",
        variant: "DequeIsEmpty",
        eval_kind: InterpreterEvalKind::DequeIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deque",
        name: "len",
        variant: "DequeLen",
        eval_kind: InterpreterEvalKind::DequeLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deque",
        name: "new",
        variant: "DequeNew",
        eval_kind: InterpreterEvalKind::DequeNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deque",
        name: "pop_back",
        variant: "DequePopBack",
        eval_kind: InterpreterEvalKind::DequePopBack,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deque",
        name: "pop_front",
        variant: "DequePopFront",
        eval_kind: InterpreterEvalKind::DequePopFront,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deque",
        name: "push_back",
        variant: "DequePushBack",
        eval_kind: InterpreterEvalKind::DequePushBack,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deque",
        name: "push_front",
        variant: "DequePushFront",
        eval_kind: InterpreterEvalKind::DequePushFront,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deque",
        name: "to_list",
        variant: "DequeToList",
        eval_kind: InterpreterEvalKind::DequeToList,
    },
    InterpreterIntrinsicSpec {
        namespace: "Counter",
        name: "add",
        variant: "CounterAdd",
        eval_kind: InterpreterEvalKind::CounterAdd,
    },
    InterpreterIntrinsicSpec {
        namespace: "Counter",
        name: "new",
        variant: "CounterNew",
        eval_kind: InterpreterEvalKind::CounterNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "Counter",
        name: "value",
        variant: "CounterValue",
        eval_kind: InterpreterEvalKind::CounterValue,
    },
    InterpreterIntrinsicSpec {
        namespace: "Diff",
        name: "unified",
        variant: "DiffUnified",
        eval_kind: InterpreterEvalKind::DiffUnified,
    },
    InterpreterIntrinsicSpec {
        namespace: "Duration",
        name: "add",
        variant: "DurationAdd",
        eval_kind: InterpreterEvalKind::DurationAdd,
    },
    InterpreterIntrinsicSpec {
        namespace: "Duration",
        name: "as_ms",
        variant: "DurationAsMs",
        eval_kind: InterpreterEvalKind::DurationAsMs,
    },
    InterpreterIntrinsicSpec {
        namespace: "Duration",
        name: "as_seconds",
        variant: "DurationAsSeconds",
        eval_kind: InterpreterEvalKind::DurationAsSeconds,
    },
    InterpreterIntrinsicSpec {
        namespace: "Duration",
        name: "ms",
        variant: "DurationMs",
        eval_kind: InterpreterEvalKind::DurationMs,
    },
    InterpreterIntrinsicSpec {
        namespace: "Duration",
        name: "seconds",
        variant: "DurationSeconds",
        eval_kind: InterpreterEvalKind::DurationSeconds,
    },
    InterpreterIntrinsicSpec {
        namespace: "FileError",
        name: "message",
        variant: "FileErrorMessage",
        eval_kind: InterpreterEvalKind::FileErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "Hash",
        name: "sha256_bytes",
        variant: "HashSha256Bytes",
        eval_kind: InterpreterEvalKind::HashSha256Bytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "Hash",
        name: "sha256_string",
        variant: "HashSha256String",
        eval_kind: InterpreterEvalKind::HashSha256String,
    },
    InterpreterIntrinsicSpec {
        namespace: "Int",
        name: "to_string",
        variant: "IntToString",
        eval_kind: InterpreterEvalKind::IntToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "extension",
        variant: "PathExtension",
        eval_kind: InterpreterEvalKind::PathExtension,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "file_name",
        variant: "PathFileName",
        eval_kind: InterpreterEvalKind::PathFileName,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "from_string",
        variant: "PathFromString",
        eval_kind: InterpreterEvalKind::PathFromString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "is_absolute",
        variant: "PathIsAbsolute",
        eval_kind: InterpreterEvalKind::PathIsAbsolute,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "join",
        variant: "PathJoin",
        eval_kind: InterpreterEvalKind::PathJoin,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "normalize",
        variant: "PathNormalize",
        eval_kind: InterpreterEvalKind::PathNormalize,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "parent",
        variant: "PathParent",
        eval_kind: InterpreterEvalKind::PathParent,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "resolve_relative",
        variant: "PathResolveRelative",
        eval_kind: InterpreterEvalKind::PathResolveRelative,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "safe_relative",
        variant: "PathSafeRelative",
        eval_kind: InterpreterEvalKind::PathSafeRelative,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "starts_with",
        variant: "PathStartsWith",
        eval_kind: InterpreterEvalKind::PathStartsWith,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "to_string",
        variant: "PathToString",
        eval_kind: InterpreterEvalKind::PathToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "with_extension",
        variant: "PathWithExtension",
        eval_kind: InterpreterEvalKind::PathWithExtension,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "append",
        variant: "ListAppend",
        eval_kind: InterpreterEvalKind::ListAppend,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "clear",
        variant: "ListClear",
        eval_kind: InterpreterEvalKind::ListClear,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "consume",
        variant: "ListConsume",
        eval_kind: InterpreterEvalKind::ListConsume,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "contains_value",
        variant: "ListContainsValue",
        eval_kind: InterpreterEvalKind::ListContainsValue,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "first",
        variant: "ListFirst",
        eval_kind: InterpreterEvalKind::ListFirst,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "get",
        variant: "ListGet",
        eval_kind: InterpreterEvalKind::ListGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "is_empty",
        variant: "ListIsEmpty",
        eval_kind: InterpreterEvalKind::ListIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "join",
        variant: "ListJoin",
        eval_kind: InterpreterEvalKind::ListJoin,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "last",
        variant: "ListLast",
        eval_kind: InterpreterEvalKind::ListLast,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "len",
        variant: "ListLen",
        eval_kind: InterpreterEvalKind::ListLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "new",
        variant: "ListNew",
        eval_kind: InterpreterEvalKind::ListNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "pop",
        variant: "ListPop",
        eval_kind: InterpreterEvalKind::ListPop,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "push",
        variant: "ListPush",
        eval_kind: InterpreterEvalKind::ListPush,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "remove_at",
        variant: "ListRemoveAt",
        eval_kind: InterpreterEvalKind::ListRemoveAt,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "reverse",
        variant: "ListReverse",
        eval_kind: InterpreterEvalKind::ListReverse,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "set",
        variant: "ListSet",
        eval_kind: InterpreterEvalKind::ListSet,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "skip",
        variant: "ListSkip",
        eval_kind: InterpreterEvalKind::ListSkip,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "slice",
        variant: "ListSlice",
        eval_kind: InterpreterEvalKind::ListSlice,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "sort",
        variant: "ListSort",
        eval_kind: InterpreterEvalKind::ListSort,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "take",
        variant: "ListTake",
        eval_kind: InterpreterEvalKind::ListTake,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "to_json_strings",
        variant: "ListToJsonStrings",
        eval_kind: InterpreterEvalKind::ListToJsonStrings,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "to_json_values",
        variant: "ListToJsonValues",
        eval_kind: InterpreterEvalKind::ListToJsonValues,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "clear",
        variant: "MapClear",
        eval_kind: InterpreterEvalKind::MapClear,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "contains_key",
        variant: "MapContainsKey",
        eval_kind: InterpreterEvalKind::MapContainsKey,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "get",
        variant: "MapGet",
        eval_kind: InterpreterEvalKind::MapGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "get_or_default",
        variant: "MapGetOrDefault",
        eval_kind: InterpreterEvalKind::MapGetOrDefault,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "insert",
        variant: "MapInsert",
        eval_kind: InterpreterEvalKind::MapInsert,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "insert_old",
        variant: "MapInsertOld",
        eval_kind: InterpreterEvalKind::MapInsertOld,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "is_empty",
        variant: "MapIsEmpty",
        eval_kind: InterpreterEvalKind::MapIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "keys",
        variant: "MapKeys",
        eval_kind: InterpreterEvalKind::MapKeys,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "len",
        variant: "MapLen",
        eval_kind: InterpreterEvalKind::MapLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "new",
        variant: "MapNew",
        eval_kind: InterpreterEvalKind::MapNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "remove",
        variant: "MapRemove",
        eval_kind: InterpreterEvalKind::MapRemove,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "values",
        variant: "MapValues",
        eval_kind: InterpreterEvalKind::MapValues,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpError",
        name: "message",
        variant: "HttpErrorMessage",
        eval_kind: InterpreterEvalKind::HttpErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpResponse",
        name: "is_success",
        variant: "HttpResponseIsSuccess",
        eval_kind: InterpreterEvalKind::HttpResponseIsSuccess,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpResponse",
        name: "lines",
        variant: "HttpResponseLines",
        eval_kind: InterpreterEvalKind::HttpResponseLines,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpResponse",
        name: "status",
        variant: "HttpResponseStatus",
        eval_kind: InterpreterEvalKind::HttpResponseStatus,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpResponse",
        name: "text",
        variant: "HttpResponseText",
        eval_kind: InterpreterEvalKind::HttpResponseText,
    },
    InterpreterIntrinsicSpec {
        namespace: "Option",
        name: "is_none",
        variant: "OptionIsNone",
        eval_kind: InterpreterEvalKind::OptionIsNone,
    },
    InterpreterIntrinsicSpec {
        namespace: "Option",
        name: "is_some",
        variant: "OptionIsSome",
        eval_kind: InterpreterEvalKind::OptionIsSome,
    },
    InterpreterIntrinsicSpec {
        namespace: "Option",
        name: "ok_or",
        variant: "OptionOkOr",
        eval_kind: InterpreterEvalKind::OptionOkOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Option",
        name: "or",
        variant: "OptionOr",
        eval_kind: InterpreterEvalKind::OptionOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Option",
        name: "unwrap_or",
        variant: "OptionUnwrapOr",
        eval_kind: InterpreterEvalKind::OptionUnwrapOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "OS",
        name: "close",
        variant: "OsClose",
        eval_kind: InterpreterEvalKind::OsClose,
    },
    InterpreterIntrinsicSpec {
        namespace: "PersistentMap",
        name: "clear",
        variant: "PersistentMapClear",
        eval_kind: InterpreterEvalKind::PersistentMapClear,
    },
    InterpreterIntrinsicSpec {
        namespace: "PersistentMap",
        name: "contains_key",
        variant: "PersistentMapContainsKey",
        eval_kind: InterpreterEvalKind::PersistentMapContainsKey,
    },
    InterpreterIntrinsicSpec {
        namespace: "PersistentMap",
        name: "get",
        variant: "PersistentMapGet",
        eval_kind: InterpreterEvalKind::PersistentMapGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "PersistentMap",
        name: "insert",
        variant: "PersistentMapInsert",
        eval_kind: InterpreterEvalKind::PersistentMapInsert,
    },
    InterpreterIntrinsicSpec {
        namespace: "PersistentMap",
        name: "is_empty",
        variant: "PersistentMapIsEmpty",
        eval_kind: InterpreterEvalKind::PersistentMapIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "PersistentMap",
        name: "len",
        variant: "PersistentMapLen",
        eval_kind: InterpreterEvalKind::PersistentMapLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "PersistentMap",
        name: "new",
        variant: "PersistentMapNew",
        eval_kind: InterpreterEvalKind::PersistentMapNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "PersistentMap",
        name: "remove",
        variant: "PersistentMapRemove",
        eval_kind: InterpreterEvalKind::PersistentMapRemove,
    },
    InterpreterIntrinsicSpec {
        namespace: "PoolError",
        name: "message",
        variant: "PoolErrorMessage",
        eval_kind: InterpreterEvalKind::PoolErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "PoolStats",
        name: "available",
        variant: "PoolStatsAvailable",
        eval_kind: InterpreterEvalKind::PoolStatsAvailable,
    },
    InterpreterIntrinsicSpec {
        namespace: "PoolStats",
        name: "capacity",
        variant: "PoolStatsCapacity",
        eval_kind: InterpreterEvalKind::PoolStatsCapacity,
    },
    InterpreterIntrinsicSpec {
        namespace: "PoolStats",
        name: "created",
        variant: "PoolStatsCreated",
        eval_kind: InterpreterEvalKind::PoolStatsCreated,
    },
    InterpreterIntrinsicSpec {
        namespace: "PoolStats",
        name: "in_use",
        variant: "PoolStatsInUse",
        eval_kind: InterpreterEvalKind::PoolStatsInUse,
    },
    InterpreterIntrinsicSpec {
        namespace: "RowBuffer",
        name: "new",
        variant: "RowBufferNew",
        eval_kind: InterpreterEvalKind::RowBufferNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "Row",
        name: "field_string",
        variant: "RowFieldString",
        eval_kind: InterpreterEvalKind::RowFieldString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Regex",
        name: "captures",
        variant: "RegexCaptures",
        eval_kind: InterpreterEvalKind::RegexCaptures,
    },
    InterpreterIntrinsicSpec {
        namespace: "Regex",
        name: "compile",
        variant: "RegexCompile",
        eval_kind: InterpreterEvalKind::RegexCompile,
    },
    InterpreterIntrinsicSpec {
        namespace: "Regex",
        name: "find",
        variant: "RegexFind",
        eval_kind: InterpreterEvalKind::RegexFind,
    },
    InterpreterIntrinsicSpec {
        namespace: "Regex",
        name: "is_match",
        variant: "RegexIsMatch",
        eval_kind: InterpreterEvalKind::RegexIsMatch,
    },
    InterpreterIntrinsicSpec {
        namespace: "Regex",
        name: "replace_all",
        variant: "RegexReplaceAll",
        eval_kind: InterpreterEvalKind::RegexReplaceAll,
    },
    InterpreterIntrinsicSpec {
        namespace: "Regex",
        name: "split",
        variant: "RegexSplit",
        eval_kind: InterpreterEvalKind::RegexSplit,
    },
    InterpreterIntrinsicSpec {
        namespace: "RegexError",
        name: "message",
        variant: "RegexErrorMessage",
        eval_kind: InterpreterEvalKind::RegexErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "Request",
        name: "new",
        variant: "RequestNew",
        eval_kind: InterpreterEvalKind::RequestNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "Request",
        name: "path",
        variant: "RequestPath",
        eval_kind: InterpreterEvalKind::RequestPath,
    },
    InterpreterIntrinsicSpec {
        namespace: "Response",
        name: "body",
        variant: "ResponseBody",
        eval_kind: InterpreterEvalKind::ResponseBody,
    },
    InterpreterIntrinsicSpec {
        namespace: "Response",
        name: "ok",
        variant: "ResponseOk",
        eval_kind: InterpreterEvalKind::ResponseOk,
    },
    InterpreterIntrinsicSpec {
        namespace: "Response",
        name: "status",
        variant: "ResponseStatus",
        eval_kind: InterpreterEvalKind::ResponseStatus,
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
        namespace: "StringBuilder",
        name: "finish",
        variant: "StringBuilderFinish",
        eval_kind: InterpreterEvalKind::StringBuilderFinish,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringBuilder",
        name: "new",
        variant: "StringBuilderNew",
        eval_kind: InterpreterEvalKind::StringBuilderNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "StringBuilder",
        name: "push",
        variant: "StringBuilderPush",
        eval_kind: InterpreterEvalKind::StringBuilderPush,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "safe_relative",
        variant: "StringSafeRelative",
        eval_kind: InterpreterEvalKind::StringSafeRelative,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "to_path",
        variant: "StringToPath",
        eval_kind: InterpreterEvalKind::StringToPath,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "to_url",
        variant: "StringToUrl",
        eval_kind: InterpreterEvalKind::StringToUrl,
    },
    InterpreterIntrinsicSpec {
        namespace: "TcpError",
        name: "message",
        variant: "TcpErrorMessage",
        eval_kind: InterpreterEvalKind::TcpErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "Url",
        name: "from_string",
        variant: "UrlFromString",
        eval_kind: InterpreterEvalKind::UrlFromString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Url",
        name: "to_string",
        variant: "UrlToString",
        eval_kind: InterpreterEvalKind::UrlToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "WebSocketError",
        name: "message",
        variant: "WebSocketErrorMessage",
        eval_kind: InterpreterEvalKind::WebSocketErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "Workspace",
        name: "resolve",
        variant: "WorkspaceResolve",
        eval_kind: InterpreterEvalKind::WorkspaceResolve,
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
        InterpreterEvalKind::BufferClear => {
            "{\n            let buffer_name = interpreter.mut_arg_local_name(args, \"buffer\", 0)?;\n            interpreter.assign(buffer_name, Value::Bytes(Vec::new()))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::BufferConsume => {
            "{\n            interpreter.eval_first_arg(args)?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::BufferIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_bytes(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::BufferLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_bytes(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::BufferNew => {
            "{\n            let size = interpreter.eval_first_arg(args)?;\n            let capacity = expect_int(size)?.max(0) as usize;\n            Ok(Value::Bytes(Vec::with_capacity(capacity)))\n        }"
        }
        InterpreterEvalKind::BufferView => {
            "{\n            let value = interpreter\n                .eval_named_or_positional_arg(args, \"value\", 0)\n                .or_else(|_| interpreter.eval_named_or_positional_arg(args, \"buffer\", 0))?;\n            let start = interpreter.eval_named_or_positional_arg(args, \"start\", 1)?;\n            let len = interpreter.eval_named_or_positional_arg(args, \"len\", 2)?;\n            Ok(Value::Bytes(bytes_slice(\n                expect_bytes(value)?,\n                expect_int(start)?,\n                expect_int(len)?,\n            )))\n        }"
        }
        InterpreterEvalKind::BufferViewToBytes | InterpreterEvalKind::BytesFromBuffer => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::BytesConcat => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let mut bytes = expect_bytes(left)?;\n            bytes.extend(expect_bytes(right)?);\n            Ok(Value::Bytes(bytes))\n        }"
        }
        InterpreterEvalKind::BytesConsume => {
            "{\n            interpreter.eval_first_arg(args)?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::BytesFromString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bytes(expect_string(value)?.into_bytes()))\n        }"
        }
        InterpreterEvalKind::BytesIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_bytes(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::BytesLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_bytes(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::BytesSlice => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let start = interpreter.eval_named_or_positional_arg(args, \"start\", 1)?;\n            let len = interpreter.eval_named_or_positional_arg(args, \"len\", 2)?;\n            Ok(Value::Bytes(bytes_slice(\n                expect_bytes(value)?,\n                expect_int(start)?,\n                expect_int(len)?,\n            )))\n        }"
        }
        InterpreterEvalKind::BytesViewStartsWith => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let prefix = interpreter.eval_named_or_positional_arg(args, \"prefix\", 1)?;\n            Ok(Value::Bool(\n                expect_bytes(value)?.starts_with(&expect_bytes(prefix)?),\n            ))\n        }"
        }
        InterpreterEvalKind::BytesViewToBytes => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::CacheNew => {
            "{\n            Ok(Value::Map(Vec::new()))\n        }"
        }
        InterpreterEvalKind::ChannelErrorMessage => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            read_field(&value, \"message\")\n        }"
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
        InterpreterEvalKind::DequeClear => {
            "{\n            let deque_name = interpreter.mut_arg_local_name(args, \"deque\", 0)?;\n            interpreter.assign(deque_name, Value::List(Vec::new()))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::DequeIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_list(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::DequeLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_list(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::DequeNew => {
            "{\n            Ok(Value::List(Vec::new()))\n        }"
        }
        InterpreterEvalKind::DequePopBack => {
            "{\n            let deque_name = interpreter.mut_arg_local_name(args, \"deque\", 0)?;\n            let mut deque = expect_list(interpreter.lookup(deque_name)?)?;\n            let value = deque.pop();\n            interpreter.assign(deque_name, Value::List(deque))?;\n            Ok(value_option(value, |value| value))\n        }"
        }
        InterpreterEvalKind::DequePopFront => {
            "{\n            let deque_name = interpreter.mut_arg_local_name(args, \"deque\", 0)?;\n            let mut deque = expect_list(interpreter.lookup(deque_name)?)?;\n            let value = if deque.is_empty() {\n                None\n            } else {\n                Some(deque.remove(0))\n            };\n            interpreter.assign(deque_name, Value::List(deque))?;\n            Ok(value_option(value, |value| value))\n        }"
        }
        InterpreterEvalKind::DequePushBack => {
            "{\n            let deque_name = interpreter.mut_arg_local_name(args, \"deque\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let mut deque = expect_list(interpreter.lookup(deque_name)?)?;\n            deque.push(value);\n            interpreter.assign(deque_name, Value::List(deque))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::DequePushFront => {
            "{\n            let deque_name = interpreter.mut_arg_local_name(args, \"deque\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let mut deque = expect_list(interpreter.lookup(deque_name)?)?;\n            deque.insert(0, value);\n            interpreter.assign(deque_name, Value::List(deque))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::DequeToList => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::CounterAdd => {
            "{\n            let counter_name = interpreter.mut_arg_local_name(args, \"counter\", 0)?;\n            let amount = interpreter.eval_named_or_positional_arg(args, \"amount\", 1)?;\n            let value = expect_counter_value(interpreter.lookup(counter_name)?)? + expect_int(amount)?;\n            interpreter.assign(counter_name, counter_value(value))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::CounterNew => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            Ok(counter_value(expect_int(value)?))\n        }"
        }
        InterpreterEvalKind::CounterValue => {
            "{\n            let counter = interpreter.eval_named_or_positional_arg(args, \"counter\", 0)?;\n            Ok(Value::Int(expect_counter_value(counter)?))\n        }"
        }
        InterpreterEvalKind::DiffUnified => {
            "{\n            let old = interpreter.eval_named_or_positional_arg(args, \"old\", 0)?;\n            let new = interpreter.eval_named_or_positional_arg(args, \"new\", 1)?;\n            Ok(Value::String(diff_unified_string(\n                &expect_string(old)?,\n                &expect_string(new)?,\n            )))\n        }"
        }
        InterpreterEvalKind::DurationAdd => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            Ok(Value::Int(expect_int(left)? + expect_int(right)?))\n        }"
        }
        InterpreterEvalKind::DurationAsMs | InterpreterEvalKind::DurationMs => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::DurationAsSeconds => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_int(value)? / 1000))\n        }"
        }
        InterpreterEvalKind::DurationSeconds => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_int(value)? * 1000))\n        }"
        }
        InterpreterEvalKind::FileErrorMessage => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            read_field(&value, \"message\")\n        }"
        }
        InterpreterEvalKind::HashSha256Bytes => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(sha256_digest(&expect_bytes(value)?)))\n        }"
        }
        InterpreterEvalKind::HashSha256String => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(sha256_digest(\n                expect_string(value)?.as_bytes(),\n            )))\n        }"
        }
        InterpreterEvalKind::IntToString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_int(value)?.to_string()))\n        }"
        }
        InterpreterEvalKind::PathExtension => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(value_option(\n                Path::new(&expect_string(path)?)\n                    .extension()\n                    .map(|extension| Value::String(extension.to_string_lossy().to_string())),\n                |value| value,\n            ))\n        }"
        }
        InterpreterEvalKind::PathFileName => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(value_option(\n                Path::new(&expect_string(path)?)\n                    .file_name()\n                    .map(|name| Value::String(name.to_string_lossy().to_string())),\n                |value| value,\n            ))\n        }"
        }
        InterpreterEvalKind::PathFromString
        | InterpreterEvalKind::PathToString
        | InterpreterEvalKind::StringToPath
        | InterpreterEvalKind::StringToUrl
        | InterpreterEvalKind::UrlFromString
        | InterpreterEvalKind::UrlToString => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::PathIsAbsolute => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(Path::new(&expect_string(path)?).is_absolute()))\n        }"
        }
        InterpreterEvalKind::PathJoin => {
            "{\n            let base = interpreter.eval_named_or_positional_arg(args, \"base\", 0)?;\n            let child = interpreter.eval_named_or_positional_arg(args, \"child\", 1)?;\n            Ok(Value::String(path_join_string(\n                &expect_string(base)?,\n                &expect_string(child)?,\n            )))\n        }"
        }
        InterpreterEvalKind::PathNormalize => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(path_normalize_string(&expect_string(path)?)))\n        }"
        }
        InterpreterEvalKind::PathParent => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(value_option(\n                Path::new(&expect_string(path)?)\n                    .parent()\n                    .map(|parent| Value::String(parent.to_string_lossy().to_string())),\n                |value| value,\n            ))\n        }"
        }
        InterpreterEvalKind::PathResolveRelative | InterpreterEvalKind::WorkspaceResolve => {
            "{\n            let root = interpreter.eval_named_or_positional_arg(args, \"root\", 0)?;\n            let relative = interpreter.eval_named_or_positional_arg(args, \"relative\", 1)?;\n            Ok(result_value(\n                path_resolve_relative_string(&expect_string(root)?, &expect_string(relative)?)\n                    .map(Value::String)\n                    .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::PathSafeRelative | InterpreterEvalKind::StringSafeRelative => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                path_safe_relative_string(&expect_string(value)?)\n                    .map(Value::String)\n                    .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::PathStartsWith => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let base = interpreter.eval_named_or_positional_arg(args, \"base\", 1)?;\n            Ok(Value::Bool(\n                Path::new(&expect_string(path)?).starts_with(Path::new(&expect_string(base)?)),\n            ))\n        }"
        }
        InterpreterEvalKind::PathWithExtension => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let extension = interpreter.eval_named_or_positional_arg(args, \"extension\", 1)?;\n            let mut path = PathBuf::from(expect_string(path)?);\n            path.set_extension(expect_string(extension)?);\n            Ok(Value::String(path.to_string_lossy().to_string()))\n        }"
        }
        InterpreterEvalKind::ListAppend => {
            "{\n            let list_name = interpreter.mut_arg_local_name(args, \"list\", 0)?;\n            let values = interpreter.eval_named_or_positional_arg(args, \"values\", 1)?;\n            let mut list = expect_list(interpreter.lookup(list_name)?)?;\n            list.extend(expect_list(values)?);\n            interpreter.assign(list_name, Value::List(list))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ListClear => {
            "{\n            let list_name = interpreter.mut_arg_local_name(args, \"list\", 0)?;\n            interpreter.assign(list_name, Value::List(Vec::new()))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ListConsume => {
            "{\n            interpreter.eval_first_arg(args)?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ListContainsValue => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            Ok(Value::Bool(\n                expect_list(list)?.iter().any(|item| item == &value),\n            ))\n        }"
        }
        InterpreterEvalKind::ListFirst => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(value_option(\n                expect_list(value)?.into_iter().next(),\n                |value| value,\n            ))\n        }"
        }
        InterpreterEvalKind::ListGet => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let index = interpreter.eval_named_or_positional_arg(args, \"index\", 1)?;\n            list_get(expect_list(list)?, expect_int(index)?)\n        }"
        }
        InterpreterEvalKind::ListIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_list(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::ListJoin => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let separator = interpreter.eval_named_or_positional_arg(args, \"separator\", 1)?;\n            let strings = expect_list(list)?\n                .into_iter()\n                .map(expect_string)\n                .collect::<Result<Vec<_>, _>>()?;\n            Ok(Value::String(strings.join(&expect_string(separator)?)))\n        }"
        }
        InterpreterEvalKind::ListLast => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(value_option(\n                expect_list(value)?.into_iter().last(),\n                |value| value,\n            ))\n        }"
        }
        InterpreterEvalKind::ListLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_list(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::ListNew => {
            "{\n            Ok(Value::List(Vec::new()))\n        }"
        }
        InterpreterEvalKind::ListPop => {
            "{\n            let list_name = interpreter.mut_arg_local_name(args, \"list\", 0)?;\n            let mut list = expect_list(interpreter.lookup(list_name)?)?;\n            let value = list.pop();\n            interpreter.assign(list_name, Value::List(list))?;\n            Ok(value_option(value, |value| value))\n        }"
        }
        InterpreterEvalKind::ListPush => {
            "{\n            let list_name = interpreter.mut_arg_local_name(args, \"list\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let mut list = expect_list(interpreter.lookup(list_name)?)?;\n            list.push(value);\n            interpreter.assign(list_name, Value::List(list))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ListRemoveAt => {
            "{\n            let list_name = interpreter.mut_arg_local_name(args, \"list\", 0)?;\n            let index = interpreter.eval_named_or_positional_arg(args, \"index\", 1)?;\n            let mut list = expect_list(interpreter.lookup(list_name)?)?;\n            let index = expect_int(index)?;\n            let value = if index < 0 || index as usize >= list.len() {\n                None\n            } else {\n                Some(list.remove(index as usize))\n            };\n            interpreter.assign(list_name, Value::List(list))?;\n            Ok(value_option(value, |value| value))\n        }"
        }
        InterpreterEvalKind::ListReverse => {
            "{\n            let list = interpreter.eval_first_arg(args)?;\n            let mut list = expect_list(list)?;\n            list.reverse();\n            Ok(Value::List(list))\n        }"
        }
        InterpreterEvalKind::ListSet => {
            "{\n            let list_name = interpreter.mut_arg_local_name(args, \"list\", 0)?;\n            let index = interpreter.eval_named_or_positional_arg(args, \"index\", 1)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 2)?;\n            interpreter.assign_list_index(list_name, expect_int(index)?, value)?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ListSkip => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let count = interpreter.eval_named_or_positional_arg(args, \"count\", 1)?;\n            let count = expect_int(count)?.max(0) as usize;\n            Ok(Value::List(\n                expect_list(list)?.into_iter().skip(count).collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::ListSlice => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let start = interpreter.eval_named_or_positional_arg(args, \"start\", 1)?;\n            let len = interpreter.eval_named_or_positional_arg(args, \"len\", 2)?;\n            Ok(Value::List(list_slice(\n                expect_list(list)?,\n                expect_int(start)?,\n                expect_int(len)?,\n            )))\n        }"
        }
        InterpreterEvalKind::ListSort => {
            "{\n            let list_name = interpreter.mut_arg_local_name(args, \"list\", 0)?;\n            let mut list = expect_list(interpreter.lookup(list_name)?)?;\n            sort_values(&mut list)?;\n            interpreter.assign(list_name, Value::List(list))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ListTake => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let count = interpreter.eval_named_or_positional_arg(args, \"count\", 1)?;\n            let count = expect_int(count)?.max(0) as usize;\n            Ok(Value::List(\n                expect_list(list)?.into_iter().take(count).collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::ListToJsonStrings => {
            "{\n            let list = interpreter.eval_first_arg(args)?;\n            let strings = expect_list(list)?\n                .into_iter()\n                .map(expect_string)\n                .map(|value| value.map(serde_json::Value::String))\n                .collect::<Result<Vec<_>, _>>()?;\n            Ok(Value::Json(serde_json::Value::Array(strings)))\n        }"
        }
        InterpreterEvalKind::ListToJsonValues => {
            "{\n            let list = interpreter.eval_first_arg(args)?;\n            let values = expect_list(list)?\n                .into_iter()\n                .map(expect_json)\n                .collect::<Result<Vec<_>, _>>()?;\n            Ok(Value::Json(serde_json::Value::Array(values)))\n        }"
        }
        InterpreterEvalKind::MapClear => {
            "{\n            let map_name = interpreter.mut_arg_local_name(args, \"map\", 0)?;\n            interpreter.assign(map_name, Value::Map(Vec::new()))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::MapContainsKey => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            Ok(Value::Bool(map_get(&expect_map(map)?, &key).is_some()))\n        }"
        }
        InterpreterEvalKind::MapGet => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            Ok(value_option(map_get(&expect_map(map)?, &key), |value| {\n                value\n            }))\n        }"
        }
        InterpreterEvalKind::MapGetOrDefault => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let default = interpreter.eval_named_or_positional_arg(args, \"default\", 2)?;\n            Ok(map_get(&expect_map(map)?, &key).unwrap_or(default))\n        }"
        }
        InterpreterEvalKind::MapInsert => {
            "{\n            let map_name = interpreter.mut_arg_local_name(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 2)?;\n            let mut map = expect_map(interpreter.lookup(map_name)?)?;\n            map_insert(&mut map, key, value);\n            interpreter.assign(map_name, Value::Map(map))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::MapInsertOld => {
            "{\n            let map_name = interpreter.mut_arg_local_name(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 2)?;\n            let mut map = expect_map(interpreter.lookup(map_name)?)?;\n            let old = map_insert(&mut map, key, value);\n            interpreter.assign(map_name, Value::Map(map))?;\n            Ok(value_option(old, |value| value))\n        }"
        }
        InterpreterEvalKind::MapIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_map(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::MapKeys => {
            "{\n            let map = interpreter.eval_first_arg(args)?;\n            Ok(Value::List(\n                expect_map(map)?.into_iter().map(|(key, _)| key).collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::MapLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_map(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::MapNew => {
            "{\n            Ok(Value::Map(Vec::new()))\n        }"
        }
        InterpreterEvalKind::MapRemove => {
            "{\n            let map_name = interpreter.mut_arg_local_name(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let mut map = expect_map(interpreter.lookup(map_name)?)?;\n            let old = map_remove(&mut map, &key);\n            interpreter.assign(map_name, Value::Map(map))?;\n            Ok(value_option(old, |value| value))\n        }"
        }
        InterpreterEvalKind::MapValues => {
            "{\n            let map = interpreter.eval_first_arg(args)?;\n            Ok(Value::List(\n                expect_map(map)?\n                    .into_iter()\n                    .map(|(_, value)| value)\n                    .collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::HttpErrorMessage => {
            "{\n            let error = interpreter.eval_named_or_positional_arg(args, \"error\", 0)?;\n            read_field(&error, \"message\")\n        }"
        }
        InterpreterEvalKind::HttpResponseIsSuccess => {
            "{\n            let response = interpreter.eval_named_or_positional_arg(args, \"response\", 0)?;\n            let (status, _) = expect_http_response(response)?;\n            Ok(Value::Bool((200..300).contains(&status)))\n        }"
        }
        InterpreterEvalKind::HttpResponseLines => {
            "{\n            let response = interpreter.eval_named_or_positional_arg(args, \"response\", 0)?;\n            let (_, body) = expect_http_response(response)?;\n            Ok(Value::List(\n                body.lines()\n                    .map(|line| Value::String(line.to_string()))\n                    .collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::HttpResponseStatus => {
            "{\n            let response = interpreter.eval_named_or_positional_arg(args, \"response\", 0)?;\n            let (status, _) = expect_http_response(response)?;\n            Ok(Value::Int(status))\n        }"
        }
        InterpreterEvalKind::HttpResponseText => {
            "{\n            let response = interpreter.eval_named_or_positional_arg(args, \"response\", 0)?;\n            let (_, body) = expect_http_response(response)?;\n            Ok(Value::String(body))\n        }"
        }
        InterpreterEvalKind::OptionIsNone => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(matches!(\n                value,\n                Value::Variant { name, .. } if name == \"None\"\n            )))\n        }"
        }
        InterpreterEvalKind::OptionIsSome => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(matches!(\n                value,\n                Value::Variant { name, .. } if name == \"Some\"\n            )))\n        }"
        }
        InterpreterEvalKind::OptionOkOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let error = interpreter.eval_named_or_positional_arg(args, \"error\", 1)?;\n            Ok(match option_payload(value)? {\n                Some(value) => value_ok(value),\n                None => value_err(error),\n            })\n        }"
        }
        InterpreterEvalKind::OptionOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 1)?;\n            Ok(match option_payload(value)? {\n                Some(value) => value_some(value),\n                None => fallback,\n            })\n        }"
        }
        InterpreterEvalKind::OptionUnwrapOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let default = interpreter.eval_named_or_positional_arg(args, \"default\", 1)?;\n            Ok(option_payload(value)?.unwrap_or(default))\n        }"
        }
        InterpreterEvalKind::OsClose => {
            "{\n            let fd = interpreter.eval_named_or_positional_arg(args, \"fd\", 0)?;\n            let _ = expect_int(fd)?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::PersistentMapClear => {
            "{\n            interpreter.eval_first_arg(args)?;\n            Ok(Value::Map(Vec::new()))\n        }"
        }
        InterpreterEvalKind::PersistentMapContainsKey => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            Ok(Value::Bool(map_get(&expect_map(map)?, &key).is_some()))\n        }"
        }
        InterpreterEvalKind::PersistentMapGet => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            Ok(value_option(map_get(&expect_map(map)?, &key), |value| {\n                value\n            }))\n        }"
        }
        InterpreterEvalKind::PersistentMapInsert => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 2)?;\n            let mut map = expect_map(map)?;\n            map_insert(&mut map, key, value);\n            Ok(Value::Map(map))\n        }"
        }
        InterpreterEvalKind::PersistentMapIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_map(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::PersistentMapLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_map(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::PersistentMapNew => {
            "{\n            Ok(Value::Map(Vec::new()))\n        }"
        }
        InterpreterEvalKind::PersistentMapRemove => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let mut map = expect_map(map)?;\n            map_remove(&mut map, &key);\n            Ok(Value::Map(map))\n        }"
        }
        InterpreterEvalKind::PoolErrorMessage => {
            "{\n            let error = interpreter.eval_named_or_positional_arg(args, \"error\", 0)?;\n            read_field(&error, \"message\")\n        }"
        }
        InterpreterEvalKind::PoolStatsAvailable => {
            "{\n            let stats = interpreter.eval_named_or_positional_arg(args, \"stats\", 0)?;\n            read_field(&stats, \"available\")\n        }"
        }
        InterpreterEvalKind::PoolStatsCapacity => {
            "{\n            let stats = interpreter.eval_named_or_positional_arg(args, \"stats\", 0)?;\n            read_field(&stats, \"capacity\")\n        }"
        }
        InterpreterEvalKind::PoolStatsCreated => {
            "{\n            let stats = interpreter.eval_named_or_positional_arg(args, \"stats\", 0)?;\n            read_field(&stats, \"created\")\n        }"
        }
        InterpreterEvalKind::PoolStatsInUse => {
            "{\n            let stats = interpreter.eval_named_or_positional_arg(args, \"stats\", 0)?;\n            read_field(&stats, \"in_use\")\n        }"
        }
        InterpreterEvalKind::RowBufferNew => {
            "{\n            let size = interpreter.eval_named_or_positional_arg(args, \"size\", 0)?;\n            let capacity = expect_int(size)?.max(0) as usize;\n            Ok(row_buffer_value(Vec::with_capacity(capacity)))\n        }"
        }
        InterpreterEvalKind::RowFieldString => {
            "{\n            let row = interpreter.eval_named_or_positional_arg(args, \"row\", 0)?;\n            let index = interpreter.eval_named_or_positional_arg(args, \"index\", 1)?;\n            Ok(result_value(row_field_string_value(\n                expect_row_fields(row)?,\n                expect_int(index)?,\n            )))\n        }"
        }
        InterpreterEvalKind::RegexCaptures => {
            "{\n            let regex = interpreter.eval_named_or_positional_arg(args, \"regex\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let regex = expect_regex(regex)?;\n            let captures = regex\n                .captures(&expect_string(value)?)\n                .map(|captures| {\n                    captures\n                        .iter()\n                        .filter_map(|matched| {\n                            matched.map(|matched| Value::String(matched.as_str().to_string()))\n                        })\n                        .collect()\n                })\n                .unwrap_or_default();\n            Ok(Value::List(captures))\n        }"
        }
        InterpreterEvalKind::RegexCompile => {
            "{\n            let pattern = interpreter.eval_first_arg(args)?;\n            let pattern = expect_string(pattern)?;\n            Ok(result_value(\n                regex::Regex::new(&pattern)\n                    .map(|_| regex_value(pattern))\n                    .map_err(|error| regex_error(error.to_string())),\n            ))\n        }"
        }
        InterpreterEvalKind::RegexErrorMessage => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            read_field(&value, \"message\")\n        }"
        }
        InterpreterEvalKind::RegexFind => {
            "{\n            let regex = interpreter.eval_named_or_positional_arg(args, \"regex\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let regex = expect_regex(regex)?;\n            let value = expect_string(value)?;\n            Ok(value_option(\n                regex.find(&value).map(|m| m.as_str().to_string()),\n                Value::String,\n            ))\n        }"
        }
        InterpreterEvalKind::RegexIsMatch => {
            "{\n            let regex = interpreter.eval_named_or_positional_arg(args, \"regex\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let regex = expect_regex(regex)?;\n            Ok(Value::Bool(regex.is_match(&expect_string(value)?)))\n        }"
        }
        InterpreterEvalKind::RegexReplaceAll => {
            "{\n            let regex = interpreter.eval_named_or_positional_arg(args, \"regex\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let replacement = interpreter.eval_named_or_positional_arg(args, \"replacement\", 2)?;\n            let regex = expect_regex(regex)?;\n            Ok(Value::String(\n                regex\n                    .replace_all(&expect_string(value)?, &expect_string(replacement)?)\n                    .to_string(),\n            ))\n        }"
        }
        InterpreterEvalKind::RegexSplit => {
            "{\n            let regex = interpreter.eval_named_or_positional_arg(args, \"regex\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let regex = expect_regex(regex)?;\n            let parts = regex\n                .split(&expect_string(value)?)\n                .map(|part| Value::String(part.to_string()))\n                .collect();\n            Ok(Value::List(parts))\n        }"
        }
        InterpreterEvalKind::RequestNew => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Struct {\n                name: \"Request\".to_string(),\n                fields: BTreeMap::from([(\n                    \"path\".to_string(),\n                    Value::String(expect_string(path)?),\n                )]),\n            })\n        }"
        }
        InterpreterEvalKind::RequestPath => {
            "{\n            let request = interpreter.eval_first_arg(args)?;\n            read_field(&request, \"path\")\n        }"
        }
        InterpreterEvalKind::ResponseBody => {
            "{\n            let response = interpreter.eval_first_arg(args)?;\n            read_field(&response, \"body\")\n        }"
        }
        InterpreterEvalKind::ResponseOk => {
            "{\n            let body = interpreter.eval_first_arg(args)?;\n            Ok(value_ok(Value::Struct {\n                name: \"Response\".to_string(),\n                fields: BTreeMap::from([\n                    (\"status\".to_string(), Value::Int(200)),\n                    (\"body\".to_string(), Value::String(expect_string(body)?)),\n                ]),\n            }))\n        }"
        }
        InterpreterEvalKind::ResponseStatus => {
            "{\n            let response = interpreter.eval_first_arg(args)?;\n            read_field(&response, \"status\")\n        }"
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
        InterpreterEvalKind::StringBuilderFinish => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::StringBuilderNew => {
            "{\n            Ok(Value::String(String::new()))\n        }"
        }
        InterpreterEvalKind::StringBuilderPush => {
            "{\n            let builder_name = interpreter.mut_arg_local_name(args, \"builder\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let mut builder = expect_string(interpreter.lookup(builder_name)?)?;\n            builder.push_str(&expect_string(value)?);\n            interpreter.assign(builder_name, Value::String(builder))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::TcpErrorMessage | InterpreterEvalKind::WebSocketErrorMessage => {
            "{\n            let error = interpreter.eval_named_or_positional_arg(args, \"error\", 0)?;\n            read_field(&error, \"message\")\n        }"
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
