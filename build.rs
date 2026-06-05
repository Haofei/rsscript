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
    Base64Decode,
    Base64DecodeString,
    Base64Encode,
    Base64EncodeBytes,
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
    CacheGet,
    CacheInsert,
    CacheLookup,
    CacheNew,
    ChannelBounded,
    ChannelErrorMessage,
    ChannelReceiver,
    ChannelSender,
    CharCompare,
    CharFromCode,
    CharIsAlphanumeric,
    CharIsAlpha,
    CharIsDigit,
    CharIsWhitespace,
    CharToCode,
    CharToString,
    DecodeErrorMessage,
    CancellationSourceCancel,
    CancellationSourceNew,
    CancellationSourceToken,
    CancellationTokenIsCancelled,
    ClockNow,
    ClockSystemUnixMs,
    ConfigLoad,
    ConfigName,
    ConfigNew,
    ConfigRuleCount,
    ConfigStoreName,
    ConfigStoreNew,
    ConfigStoreReplace,
    CsvOpenRead,
    CsvParseRow,
    CsvReadInto,
    CsvRows,
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
    FalliblePipelineCollect,
    FalliblePipelineEach,
    FalliblePipelineFilter,
    FalliblePipelineMap,
    FalliblePipelineTryMap,
    DeadlineAfter,
    DeadlineAfterMs,
    DeadlineIsExpired,
    DeadlineRemainingMs,
    DbClose,
    DbConnectionOpen,
    DbConnectionQuery,
    DbConnectionTryOpen,
    DirectoryCopyFile,
    DirectoryCreate,
    DirectoryCreateAll,
    DirectoryCreateDirAll,
    DirectoryExists,
    DirectoryIsDir,
    DirectoryIsFile,
    DirectoryListFiles,
    DirectoryListPaths,
    DirectoryMetadata,
    DirectoryReadString,
    DirectoryRemoveDirAll,
    DirectoryRemoveFile,
    DirectoryRename,
    DirectoryWriteString,
    DurationAdd,
    DurationAsMs,
    DurationAsSeconds,
    DurationMs,
    DurationSeconds,
    EnvironmentBindFunction,
    EnvironmentChild,
    EnvironmentHasFunction,
    EnvironmentHasParent,
    EnvironmentRoot,
    EnvCurrentDir,
    EnvGet,
    EnvGetOrDefault,
    EnvHomeDir,
    EnvRunWorkspaceRoot,
    EnvSet,
    EnvSetCurrentDir,
    EnvTempDir,
    FileAppendBytes,
    FileAppendString,
    FileBytesStream,
    FileExists,
    FileErrorMessage,
    FileOpen,
    FileOpenRead,
    FileOpenWrite,
    FileReadAll,
    FileReadAllAsync,
    FileReadAllString,
    FileReadAllStringAsync,
    FileReadBytes,
    FileReadInto,
    FileReadString,
    FileRemove,
    FileWrite,
    FileWriteAsync,
    FileWriteAtomic,
    FileWriteBytes,
    FileWriteBytesView,
    FileWriteBuffer,
    FileWriteBufferView,
    FileWriteString,
    FileWriteStringAsync,
    FileWriteStringToPath,
    FunctionObjectHasClosure,
    FunctionObjectNew,
    GlobalConfigNew,
    GlobalConfigReplace,
    GlobalConfigRuleCount,
    HashSha256Bytes,
    HashSha256File,
    HashSha256String,
    HexDecode,
    HexEncode,
    HexEncodeString,
    InstantElapsed,
    PathExists,
    PathExtension,
    PathFileName,
    PathFromString,
    PathIsAbsolute,
    PathIsDir,
    PathIsFile,
    PathJoin,
    PathListFiles,
    PathListPaths,
    PathNormalize,
    PathParent,
    PathReadString,
    PathResolveRelative,
    PathSafeRelative,
    PathStartsWith,
    PathToString,
    PathWithExtension,
    PathWriteString,
    IntToString,
    ListAll,
    ListAppend,
    ListAny,
    ListClear,
    ListConsume,
    ListContains,
    ListContainsValue,
    ListCountWhere,
    ListFilter,
    ListFind,
    ListFirst,
    ListFlatMap,
    ListFold,
    ListGet,
    ListGroupBy,
    ListIsEmpty,
    ListJoin,
    ListLast,
    ListLen,
    ListMap,
    ListNew,
    ListPartition,
    ListPipeline,
    ListPop,
    ListPush,
    ListRemoveAt,
    ListReverse,
    ListSet,
    ListSkip,
    ListSlice,
    ListSort,
    ListSortBy,
    ListSortWith,
    ListTake,
    ListToJsonStrings,
    ListToJsonValues,
    ListTryFold,
    MapClear,
    MapContainsKey,
    MapFilter,
    MapFold,
    MapForEach,
    MapGet,
    MapGetOrDefault,
    MapInsert,
    MapInsertOld,
    MapIsEmpty,
    MapKeys,
    MapLen,
    MapMapValues,
    MapMerge,
    MapNew,
    MapRemove,
    MapTryFold,
    MapValues,
    HttpErrorMessage,
    HttpGet,
    HttpGetAsync,
    HttpGetRetryAsync,
    HttpGetTimeoutAsync,
    HttpPostForm,
    HttpPostFormAsync,
    HttpPostJson,
    HttpPostJsonAsync,
    HttpPostJsonBearerRetryAsync,
    HttpPostJsonRetryAsync,
    HttpPostJsonTimeoutAsync,
    HttpSendAsync,
    HttpRequestJson,
    HttpRequestWithHeader,
    HttpRequestWithRetry,
    HttpRequestWithTimeout,
    HttpResponseIsSuccess,
    HttpResponseLines,
    HttpResponseStatus,
    HttpResponseText,
    ImageInspect,
    ImageLoad,
    ImageNormalize,
    ImageResize,
    ImageSave,
    ImageSharpen,
    JsonArray,
    JsonArrayBools,
    JsonArrayContainsPrefix,
    JsonArrayContainsString,
    JsonArrayContainsSubstring,
    JsonArrayCountWhere,
    JsonArrayFold,
    JsonArrayGet,
    JsonArrayInts,
    JsonArrayLen,
    JsonArrayStrings,
    JsonAsBool,
    JsonAsInt,
    JsonAsString,
    JsonAt,
    JsonAtBool,
    JsonAtBoolOr,
    JsonAtInt,
    JsonAtIntOr,
    JsonAtOptional,
    JsonAtOptionalBool,
    JsonAtOptionalInt,
    JsonAtOptionalString,
    JsonAtOr,
    JsonAtString,
    JsonAtStringOr,
    JsonAtToString,
    JsonAtToStringOr,
    JsonBoolField,
    JsonBoolAt,
    JsonBoolAtOr,
    JsonClone,
    JsonField,
    JsonFieldBool,
    JsonFieldInt,
    JsonFieldOptional,
    JsonFieldOptionalBool,
    JsonFieldOptionalInt,
    JsonFieldOptionalString,
    JsonFieldString,
    JsonIsArray,
    JsonIsNull,
    JsonIsObject,
    JsonIntField,
    JsonIntAt,
    JsonIntAtOr,
    JsonKind,
    JsonObject,
    JsonObjectKeys,
    JsonObjectLen,
    JsonParse,
    JsonParseFile,
    JsonQuoteString,
    JsonRawField,
    JsonStringArray,
    JsonStringAt,
    JsonStringAtOr,
    JsonStringField,
    JsonStrings,
    JsonToString,
    JsonToStringAt,
    JsonToStringAtOr,
    JsonValue,
    JsonValues,
    JsonErrorMessage,
    OptionAndThen,
    OptionFilter,
    OptionIsNone,
    OptionIsSome,
    OptionMap,
    OptionOkOr,
    OptionOr,
    OptionUnwrapOr,
    OptionUnwrapOrElse,
    OrdCompare,
    OsClose,
    PatchApplyText,
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
    PipelineCollect,
    PipelineEach,
    PipelineFilter,
    PipelineMap,
    PipelineTryMap,
    ProcessRun,
    ProcessRunAsync,
    ProcessRunManyStdout,
    ProcessRunManyStdoutAsync,
    ProcessRunManyStdoutTimeout,
    ProcessRunManyStdoutTimeoutAsync,
    ProcessRunRequest,
    ProcessRunRequestAsync,
    ProcessRunRequestCancellableAsync,
    ProcessRunStdout,
    ProcessRunStdoutAsync,
    ProcessRunStdoutTimeout,
    ProcessRunStdoutTimeoutAsync,
    ProcessRunTimeout,
    ProcessRunTimeoutAsync,
    ProcessStream,
    ResourcePoolDiscard,
    ResourcePoolStats,
    RowBufferNew,
    RowFieldString,
    RegexCaptures,
    RegexCompile,
    RegexErrorMessage,
    RegexFind,
    RegexIsMatch,
    RegexReplaceAll,
    RegexSplit,
    ReceiverClose,
    ReceiverIntoStream,
    ReceiverRecv,
    ReceiverRecvCancellable,
    RequestNew,
    RequestPath,
    ResponseBody,
    ResponseOk,
    ResponseStatus,
    ResultErr,
    ResultErrMessage,
    ResultAndThen,
    ResultIsErr,
    ResultIsOk,
    ResultMap,
    ResultMapError,
    ResultOk,
    ResultUnwrapOr,
    ResultUnwrapOrElse,
    RuleLoaderLoadRules,
    SetClear,
    SetContains,
    SetDifference,
    SetForEach,
    SetInsert,
    SetIntersection,
    SetIsEmpty,
    SetIsSubset,
    SetLen,
    SetNew,
    SetRemove,
    SetToList,
    SetUnion,
    SenderClose,
    SenderSend,
    SenderSendCancellable,
    SortedMapClear,
    SortedMapContainsKey,
    SortedMapGet,
    SortedMapInsert,
    SortedMapIsEmpty,
    SortedMapKeys,
    SortedMapLen,
    SortedMapNew,
    SortedMapRemove,
    SortedMapValues,
    SortedSetClear,
    SortedSetContains,
    SortedSetInsert,
    SortedSetIsEmpty,
    SortedSetLen,
    SortedSetNew,
    SortedSetRemove,
    SortedSetToList,
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
    StringEnv,
    StringEnvOr,
    StringJoin,
    StringSafeRelative,
    StringToPath,
    StringToUrl,
    StreamCollectList,
    StreamFromList,
    StreamNext,
    TcpConnect,
    TcpErrorMessage,
    TcpStreamRead,
    TcpStreamShutdown,
    TcpStreamWrite,
    TcpStreamWriteAll,
    TempDirKeep,
    TempDirNew,
    TempDirNewIn,
    TempDirPath,
    TimerSleep,
    TimerSleepCancellable,
    TimerSleepUntil,
    TomlParseFile,
    UrlFromString,
    UrlDecodeComponent,
    UrlEncodeComponent,
    UrlToString,
    WebSocketClose,
    WebSocketConnect,
    WebSocketErrorMessage,
    WebSocketRecvBytes,
    WebSocketRecvText,
    WebSocketSendBytes,
    WebSocketSendText,
    WorkspaceResolve,
    YamlParse,
    YamlParseFile,
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
        namespace: "Base64",
        name: "decode",
        variant: "Base64Decode",
        eval_kind: InterpreterEvalKind::Base64Decode,
    },
    InterpreterIntrinsicSpec {
        namespace: "Base64",
        name: "decode_string",
        variant: "Base64DecodeString",
        eval_kind: InterpreterEvalKind::Base64DecodeString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Base64",
        name: "encode",
        variant: "Base64Encode",
        eval_kind: InterpreterEvalKind::Base64Encode,
    },
    InterpreterIntrinsicSpec {
        namespace: "Base64",
        name: "encode_bytes",
        variant: "Base64EncodeBytes",
        eval_kind: InterpreterEvalKind::Base64EncodeBytes,
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
        namespace: "Cache",
        name: "get",
        variant: "CacheGet",
        eval_kind: InterpreterEvalKind::CacheGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "Cache",
        name: "insert",
        variant: "CacheInsert",
        eval_kind: InterpreterEvalKind::CacheInsert,
    },
    InterpreterIntrinsicSpec {
        namespace: "Cache",
        name: "lookup",
        variant: "CacheLookup",
        eval_kind: InterpreterEvalKind::CacheLookup,
    },
    InterpreterIntrinsicSpec {
        namespace: "Channel",
        name: "bounded",
        variant: "ChannelBounded",
        eval_kind: InterpreterEvalKind::ChannelBounded,
    },
    InterpreterIntrinsicSpec {
        namespace: "Channel",
        name: "receiver",
        variant: "ChannelReceiver",
        eval_kind: InterpreterEvalKind::ChannelReceiver,
    },
    InterpreterIntrinsicSpec {
        namespace: "Channel",
        name: "sender",
        variant: "ChannelSender",
        eval_kind: InterpreterEvalKind::ChannelSender,
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
        namespace: "DecodeError",
        name: "message",
        variant: "DecodeErrorMessage",
        eval_kind: InterpreterEvalKind::DecodeErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "CancellationSource",
        name: "cancel",
        variant: "CancellationSourceCancel",
        eval_kind: InterpreterEvalKind::CancellationSourceCancel,
    },
    InterpreterIntrinsicSpec {
        namespace: "CancellationSource",
        name: "new",
        variant: "CancellationSourceNew",
        eval_kind: InterpreterEvalKind::CancellationSourceNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "CancellationSource",
        name: "token",
        variant: "CancellationSourceToken",
        eval_kind: InterpreterEvalKind::CancellationSourceToken,
    },
    InterpreterIntrinsicSpec {
        namespace: "CancellationToken",
        name: "is_cancelled",
        variant: "CancellationTokenIsCancelled",
        eval_kind: InterpreterEvalKind::CancellationTokenIsCancelled,
    },
    InterpreterIntrinsicSpec {
        namespace: "Clock",
        name: "now",
        variant: "ClockNow",
        eval_kind: InterpreterEvalKind::ClockNow,
    },
    InterpreterIntrinsicSpec {
        namespace: "Clock",
        name: "system_unix_ms",
        variant: "ClockSystemUnixMs",
        eval_kind: InterpreterEvalKind::ClockSystemUnixMs,
    },
    InterpreterIntrinsicSpec {
        namespace: "Config",
        name: "load",
        variant: "ConfigLoad",
        eval_kind: InterpreterEvalKind::ConfigLoad,
    },
    InterpreterIntrinsicSpec {
        namespace: "Config",
        name: "name",
        variant: "ConfigName",
        eval_kind: InterpreterEvalKind::ConfigName,
    },
    InterpreterIntrinsicSpec {
        namespace: "Config",
        name: "new",
        variant: "ConfigNew",
        eval_kind: InterpreterEvalKind::ConfigNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "Config",
        name: "rule_count",
        variant: "ConfigRuleCount",
        eval_kind: InterpreterEvalKind::ConfigRuleCount,
    },
    InterpreterIntrinsicSpec {
        namespace: "ConfigStore",
        name: "name",
        variant: "ConfigStoreName",
        eval_kind: InterpreterEvalKind::ConfigStoreName,
    },
    InterpreterIntrinsicSpec {
        namespace: "ConfigStore",
        name: "new",
        variant: "ConfigStoreNew",
        eval_kind: InterpreterEvalKind::ConfigStoreNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "ConfigStore",
        name: "replace",
        variant: "ConfigStoreReplace",
        eval_kind: InterpreterEvalKind::ConfigStoreReplace,
    },
    InterpreterIntrinsicSpec {
        namespace: "Csv",
        name: "open_read",
        variant: "CsvOpenRead",
        eval_kind: InterpreterEvalKind::CsvOpenRead,
    },
    InterpreterIntrinsicSpec {
        namespace: "Csv",
        name: "parse_row",
        variant: "CsvParseRow",
        eval_kind: InterpreterEvalKind::CsvParseRow,
    },
    InterpreterIntrinsicSpec {
        namespace: "Csv",
        name: "read_into",
        variant: "CsvReadInto",
        eval_kind: InterpreterEvalKind::CsvReadInto,
    },
    InterpreterIntrinsicSpec {
        namespace: "Csv",
        name: "rows",
        variant: "CsvRows",
        eval_kind: InterpreterEvalKind::CsvRows,
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
        namespace: "Deadline",
        name: "after",
        variant: "DeadlineAfter",
        eval_kind: InterpreterEvalKind::DeadlineAfter,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deadline",
        name: "after_ms",
        variant: "DeadlineAfterMs",
        eval_kind: InterpreterEvalKind::DeadlineAfterMs,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deadline",
        name: "is_expired",
        variant: "DeadlineIsExpired",
        eval_kind: InterpreterEvalKind::DeadlineIsExpired,
    },
    InterpreterIntrinsicSpec {
        namespace: "Deadline",
        name: "remaining_ms",
        variant: "DeadlineRemainingMs",
        eval_kind: InterpreterEvalKind::DeadlineRemainingMs,
    },
    InterpreterIntrinsicSpec {
        namespace: "Db",
        name: "close",
        variant: "DbClose",
        eval_kind: InterpreterEvalKind::DbClose,
    },
    InterpreterIntrinsicSpec {
        namespace: "DbConnection",
        name: "open",
        variant: "DbConnectionOpen",
        eval_kind: InterpreterEvalKind::DbConnectionOpen,
    },
    InterpreterIntrinsicSpec {
        namespace: "DbConnection",
        name: "query",
        variant: "DbConnectionQuery",
        eval_kind: InterpreterEvalKind::DbConnectionQuery,
    },
    InterpreterIntrinsicSpec {
        namespace: "DbConnection",
        name: "try_open",
        variant: "DbConnectionTryOpen",
        eval_kind: InterpreterEvalKind::DbConnectionTryOpen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "copy_file",
        variant: "DirectoryCopyFile",
        eval_kind: InterpreterEvalKind::DirectoryCopyFile,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "create",
        variant: "DirectoryCreate",
        eval_kind: InterpreterEvalKind::DirectoryCreate,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "create_all",
        variant: "DirectoryCreateAll",
        eval_kind: InterpreterEvalKind::DirectoryCreateAll,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "create_dir_all",
        variant: "DirectoryCreateDirAll",
        eval_kind: InterpreterEvalKind::DirectoryCreateDirAll,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "exists",
        variant: "DirectoryExists",
        eval_kind: InterpreterEvalKind::DirectoryExists,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "is_dir",
        variant: "DirectoryIsDir",
        eval_kind: InterpreterEvalKind::DirectoryIsDir,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "is_file",
        variant: "DirectoryIsFile",
        eval_kind: InterpreterEvalKind::DirectoryIsFile,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "list_files",
        variant: "DirectoryListFiles",
        eval_kind: InterpreterEvalKind::DirectoryListFiles,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "list_paths",
        variant: "DirectoryListPaths",
        eval_kind: InterpreterEvalKind::DirectoryListPaths,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "metadata",
        variant: "DirectoryMetadata",
        eval_kind: InterpreterEvalKind::DirectoryMetadata,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "read_string",
        variant: "DirectoryReadString",
        eval_kind: InterpreterEvalKind::DirectoryReadString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "remove_dir_all",
        variant: "DirectoryRemoveDirAll",
        eval_kind: InterpreterEvalKind::DirectoryRemoveDirAll,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "remove_file",
        variant: "DirectoryRemoveFile",
        eval_kind: InterpreterEvalKind::DirectoryRemoveFile,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "rename",
        variant: "DirectoryRename",
        eval_kind: InterpreterEvalKind::DirectoryRename,
    },
    InterpreterIntrinsicSpec {
        namespace: "Directory",
        name: "write_string",
        variant: "DirectoryWriteString",
        eval_kind: InterpreterEvalKind::DirectoryWriteString,
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
        namespace: "Environment",
        name: "bind_function",
        variant: "EnvironmentBindFunction",
        eval_kind: InterpreterEvalKind::EnvironmentBindFunction,
    },
    InterpreterIntrinsicSpec {
        namespace: "Environment",
        name: "child",
        variant: "EnvironmentChild",
        eval_kind: InterpreterEvalKind::EnvironmentChild,
    },
    InterpreterIntrinsicSpec {
        namespace: "Environment",
        name: "has_function",
        variant: "EnvironmentHasFunction",
        eval_kind: InterpreterEvalKind::EnvironmentHasFunction,
    },
    InterpreterIntrinsicSpec {
        namespace: "Environment",
        name: "has_parent",
        variant: "EnvironmentHasParent",
        eval_kind: InterpreterEvalKind::EnvironmentHasParent,
    },
    InterpreterIntrinsicSpec {
        namespace: "Environment",
        name: "root",
        variant: "EnvironmentRoot",
        eval_kind: InterpreterEvalKind::EnvironmentRoot,
    },
    InterpreterIntrinsicSpec {
        namespace: "Env",
        name: "current_dir",
        variant: "EnvCurrentDir",
        eval_kind: InterpreterEvalKind::EnvCurrentDir,
    },
    InterpreterIntrinsicSpec {
        namespace: "Env",
        name: "get",
        variant: "EnvGet",
        eval_kind: InterpreterEvalKind::EnvGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "Env",
        name: "get_or_default",
        variant: "EnvGetOrDefault",
        eval_kind: InterpreterEvalKind::EnvGetOrDefault,
    },
    InterpreterIntrinsicSpec {
        namespace: "Env",
        name: "home_dir",
        variant: "EnvHomeDir",
        eval_kind: InterpreterEvalKind::EnvHomeDir,
    },
    InterpreterIntrinsicSpec {
        namespace: "Env",
        name: "run_workspace_root",
        variant: "EnvRunWorkspaceRoot",
        eval_kind: InterpreterEvalKind::EnvRunWorkspaceRoot,
    },
    InterpreterIntrinsicSpec {
        namespace: "Env",
        name: "set",
        variant: "EnvSet",
        eval_kind: InterpreterEvalKind::EnvSet,
    },
    InterpreterIntrinsicSpec {
        namespace: "Env",
        name: "set_current_dir",
        variant: "EnvSetCurrentDir",
        eval_kind: InterpreterEvalKind::EnvSetCurrentDir,
    },
    InterpreterIntrinsicSpec {
        namespace: "Env",
        name: "temp_dir",
        variant: "EnvTempDir",
        eval_kind: InterpreterEvalKind::EnvTempDir,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "append_bytes",
        variant: "FileAppendBytes",
        eval_kind: InterpreterEvalKind::FileAppendBytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "append_string",
        variant: "FileAppendString",
        eval_kind: InterpreterEvalKind::FileAppendString,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "bytes_stream",
        variant: "FileBytesStream",
        eval_kind: InterpreterEvalKind::FileBytesStream,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "exists",
        variant: "FileExists",
        eval_kind: InterpreterEvalKind::FileExists,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "open",
        variant: "FileOpen",
        eval_kind: InterpreterEvalKind::FileOpen,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "open_read",
        variant: "FileOpenRead",
        eval_kind: InterpreterEvalKind::FileOpenRead,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "open_write",
        variant: "FileOpenWrite",
        eval_kind: InterpreterEvalKind::FileOpenWrite,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "read_all",
        variant: "FileReadAll",
        eval_kind: InterpreterEvalKind::FileReadAll,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "read_all_async",
        variant: "FileReadAllAsync",
        eval_kind: InterpreterEvalKind::FileReadAllAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "read_all_string",
        variant: "FileReadAllString",
        eval_kind: InterpreterEvalKind::FileReadAllString,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "read_all_string_async",
        variant: "FileReadAllStringAsync",
        eval_kind: InterpreterEvalKind::FileReadAllStringAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "read_bytes",
        variant: "FileReadBytes",
        eval_kind: InterpreterEvalKind::FileReadBytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "read_into",
        variant: "FileReadInto",
        eval_kind: InterpreterEvalKind::FileReadInto,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "read_string",
        variant: "FileReadString",
        eval_kind: InterpreterEvalKind::FileReadString,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "remove",
        variant: "FileRemove",
        eval_kind: InterpreterEvalKind::FileRemove,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write",
        variant: "FileWrite",
        eval_kind: InterpreterEvalKind::FileWrite,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_async",
        variant: "FileWriteAsync",
        eval_kind: InterpreterEvalKind::FileWriteAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_atomic",
        variant: "FileWriteAtomic",
        eval_kind: InterpreterEvalKind::FileWriteAtomic,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_bytes",
        variant: "FileWriteBytes",
        eval_kind: InterpreterEvalKind::FileWriteBytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_bytes_view",
        variant: "FileWriteBytesView",
        eval_kind: InterpreterEvalKind::FileWriteBytesView,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_buffer",
        variant: "FileWriteBuffer",
        eval_kind: InterpreterEvalKind::FileWriteBuffer,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_buffer_view",
        variant: "FileWriteBufferView",
        eval_kind: InterpreterEvalKind::FileWriteBufferView,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_string",
        variant: "FileWriteString",
        eval_kind: InterpreterEvalKind::FileWriteString,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_string_async",
        variant: "FileWriteStringAsync",
        eval_kind: InterpreterEvalKind::FileWriteStringAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "File",
        name: "write_string_to_path",
        variant: "FileWriteStringToPath",
        eval_kind: InterpreterEvalKind::FileWriteStringToPath,
    },
    InterpreterIntrinsicSpec {
        namespace: "FileError",
        name: "message",
        variant: "FileErrorMessage",
        eval_kind: InterpreterEvalKind::FileErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "FalliblePipeline",
        name: "collect",
        variant: "FalliblePipelineCollect",
        eval_kind: InterpreterEvalKind::FalliblePipelineCollect,
    },
    InterpreterIntrinsicSpec {
        namespace: "FalliblePipeline",
        name: "each",
        variant: "FalliblePipelineEach",
        eval_kind: InterpreterEvalKind::FalliblePipelineEach,
    },
    InterpreterIntrinsicSpec {
        namespace: "FalliblePipeline",
        name: "filter",
        variant: "FalliblePipelineFilter",
        eval_kind: InterpreterEvalKind::FalliblePipelineFilter,
    },
    InterpreterIntrinsicSpec {
        namespace: "FalliblePipeline",
        name: "map",
        variant: "FalliblePipelineMap",
        eval_kind: InterpreterEvalKind::FalliblePipelineMap,
    },
    InterpreterIntrinsicSpec {
        namespace: "FalliblePipeline",
        name: "try_map",
        variant: "FalliblePipelineTryMap",
        eval_kind: InterpreterEvalKind::FalliblePipelineTryMap,
    },
    InterpreterIntrinsicSpec {
        namespace: "FunctionObject",
        name: "has_closure",
        variant: "FunctionObjectHasClosure",
        eval_kind: InterpreterEvalKind::FunctionObjectHasClosure,
    },
    InterpreterIntrinsicSpec {
        namespace: "FunctionObject",
        name: "new",
        variant: "FunctionObjectNew",
        eval_kind: InterpreterEvalKind::FunctionObjectNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "GlobalConfig",
        name: "new",
        variant: "GlobalConfigNew",
        eval_kind: InterpreterEvalKind::GlobalConfigNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "GlobalConfig",
        name: "replace",
        variant: "GlobalConfigReplace",
        eval_kind: InterpreterEvalKind::GlobalConfigReplace,
    },
    InterpreterIntrinsicSpec {
        namespace: "GlobalConfig",
        name: "rule_count",
        variant: "GlobalConfigRuleCount",
        eval_kind: InterpreterEvalKind::GlobalConfigRuleCount,
    },
    InterpreterIntrinsicSpec {
        namespace: "Hash",
        name: "sha256_bytes",
        variant: "HashSha256Bytes",
        eval_kind: InterpreterEvalKind::HashSha256Bytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "Hash",
        name: "sha256_file",
        variant: "HashSha256File",
        eval_kind: InterpreterEvalKind::HashSha256File,
    },
    InterpreterIntrinsicSpec {
        namespace: "Hash",
        name: "sha256_string",
        variant: "HashSha256String",
        eval_kind: InterpreterEvalKind::HashSha256String,
    },
    InterpreterIntrinsicSpec {
        namespace: "Hex",
        name: "decode",
        variant: "HexDecode",
        eval_kind: InterpreterEvalKind::HexDecode,
    },
    InterpreterIntrinsicSpec {
        namespace: "Hex",
        name: "encode",
        variant: "HexEncode",
        eval_kind: InterpreterEvalKind::HexEncode,
    },
    InterpreterIntrinsicSpec {
        namespace: "Hex",
        name: "encode_string",
        variant: "HexEncodeString",
        eval_kind: InterpreterEvalKind::HexEncodeString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Instant",
        name: "elapsed",
        variant: "InstantElapsed",
        eval_kind: InterpreterEvalKind::InstantElapsed,
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
        name: "exists",
        variant: "PathExists",
        eval_kind: InterpreterEvalKind::PathExists,
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
        name: "is_dir",
        variant: "PathIsDir",
        eval_kind: InterpreterEvalKind::PathIsDir,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "is_file",
        variant: "PathIsFile",
        eval_kind: InterpreterEvalKind::PathIsFile,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "join",
        variant: "PathJoin",
        eval_kind: InterpreterEvalKind::PathJoin,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "list_files",
        variant: "PathListFiles",
        eval_kind: InterpreterEvalKind::PathListFiles,
    },
    InterpreterIntrinsicSpec {
        namespace: "Path",
        name: "list_paths",
        variant: "PathListPaths",
        eval_kind: InterpreterEvalKind::PathListPaths,
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
        name: "read_string",
        variant: "PathReadString",
        eval_kind: InterpreterEvalKind::PathReadString,
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
        namespace: "Path",
        name: "write_string",
        variant: "PathWriteString",
        eval_kind: InterpreterEvalKind::PathWriteString,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "all",
        variant: "ListAll",
        eval_kind: InterpreterEvalKind::ListAll,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "append",
        variant: "ListAppend",
        eval_kind: InterpreterEvalKind::ListAppend,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "any",
        variant: "ListAny",
        eval_kind: InterpreterEvalKind::ListAny,
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
        name: "contains",
        variant: "ListContains",
        eval_kind: InterpreterEvalKind::ListContains,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "contains_value",
        variant: "ListContainsValue",
        eval_kind: InterpreterEvalKind::ListContainsValue,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "count_where",
        variant: "ListCountWhere",
        eval_kind: InterpreterEvalKind::ListCountWhere,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "filter",
        variant: "ListFilter",
        eval_kind: InterpreterEvalKind::ListFilter,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "find",
        variant: "ListFind",
        eval_kind: InterpreterEvalKind::ListFind,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "first",
        variant: "ListFirst",
        eval_kind: InterpreterEvalKind::ListFirst,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "flat_map",
        variant: "ListFlatMap",
        eval_kind: InterpreterEvalKind::ListFlatMap,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "fold",
        variant: "ListFold",
        eval_kind: InterpreterEvalKind::ListFold,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "get",
        variant: "ListGet",
        eval_kind: InterpreterEvalKind::ListGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "group_by",
        variant: "ListGroupBy",
        eval_kind: InterpreterEvalKind::ListGroupBy,
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
        name: "map",
        variant: "ListMap",
        eval_kind: InterpreterEvalKind::ListMap,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "new",
        variant: "ListNew",
        eval_kind: InterpreterEvalKind::ListNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "partition",
        variant: "ListPartition",
        eval_kind: InterpreterEvalKind::ListPartition,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "pipeline",
        variant: "ListPipeline",
        eval_kind: InterpreterEvalKind::ListPipeline,
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
        name: "sort_by",
        variant: "ListSortBy",
        eval_kind: InterpreterEvalKind::ListSortBy,
    },
    InterpreterIntrinsicSpec {
        namespace: "List",
        name: "sort_with",
        variant: "ListSortWith",
        eval_kind: InterpreterEvalKind::ListSortWith,
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
        namespace: "List",
        name: "try_fold",
        variant: "ListTryFold",
        eval_kind: InterpreterEvalKind::ListTryFold,
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
        name: "filter",
        variant: "MapFilter",
        eval_kind: InterpreterEvalKind::MapFilter,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "fold",
        variant: "MapFold",
        eval_kind: InterpreterEvalKind::MapFold,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "for_each",
        variant: "MapForEach",
        eval_kind: InterpreterEvalKind::MapForEach,
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
        name: "map_values",
        variant: "MapMapValues",
        eval_kind: InterpreterEvalKind::MapMapValues,
    },
    InterpreterIntrinsicSpec {
        namespace: "Map",
        name: "merge",
        variant: "MapMerge",
        eval_kind: InterpreterEvalKind::MapMerge,
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
        name: "try_fold",
        variant: "MapTryFold",
        eval_kind: InterpreterEvalKind::MapTryFold,
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
        namespace: "Http",
        name: "get",
        variant: "HttpGet",
        eval_kind: InterpreterEvalKind::HttpGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "get_async",
        variant: "HttpGetAsync",
        eval_kind: InterpreterEvalKind::HttpGetAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "get_retry_async",
        variant: "HttpGetRetryAsync",
        eval_kind: InterpreterEvalKind::HttpGetRetryAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "get_timeout_async",
        variant: "HttpGetTimeoutAsync",
        eval_kind: InterpreterEvalKind::HttpGetTimeoutAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "post_form",
        variant: "HttpPostForm",
        eval_kind: InterpreterEvalKind::HttpPostForm,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "post_form_async",
        variant: "HttpPostFormAsync",
        eval_kind: InterpreterEvalKind::HttpPostFormAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "post_json",
        variant: "HttpPostJson",
        eval_kind: InterpreterEvalKind::HttpPostJson,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "post_json_async",
        variant: "HttpPostJsonAsync",
        eval_kind: InterpreterEvalKind::HttpPostJsonAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "post_json_bearer_retry_async",
        variant: "HttpPostJsonBearerRetryAsync",
        eval_kind: InterpreterEvalKind::HttpPostJsonBearerRetryAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "post_json_retry_async",
        variant: "HttpPostJsonRetryAsync",
        eval_kind: InterpreterEvalKind::HttpPostJsonRetryAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "post_json_timeout_async",
        variant: "HttpPostJsonTimeoutAsync",
        eval_kind: InterpreterEvalKind::HttpPostJsonTimeoutAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Http",
        name: "send_async",
        variant: "HttpSendAsync",
        eval_kind: InterpreterEvalKind::HttpSendAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpRequest",
        name: "json",
        variant: "HttpRequestJson",
        eval_kind: InterpreterEvalKind::HttpRequestJson,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpRequest",
        name: "with_header",
        variant: "HttpRequestWithHeader",
        eval_kind: InterpreterEvalKind::HttpRequestWithHeader,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpRequest",
        name: "with_retry",
        variant: "HttpRequestWithRetry",
        eval_kind: InterpreterEvalKind::HttpRequestWithRetry,
    },
    InterpreterIntrinsicSpec {
        namespace: "HttpRequest",
        name: "with_timeout",
        variant: "HttpRequestWithTimeout",
        eval_kind: InterpreterEvalKind::HttpRequestWithTimeout,
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
        namespace: "Image",
        name: "inspect",
        variant: "ImageInspect",
        eval_kind: InterpreterEvalKind::ImageInspect,
    },
    InterpreterIntrinsicSpec {
        namespace: "Image",
        name: "load",
        variant: "ImageLoad",
        eval_kind: InterpreterEvalKind::ImageLoad,
    },
    InterpreterIntrinsicSpec {
        namespace: "Image",
        name: "normalize",
        variant: "ImageNormalize",
        eval_kind: InterpreterEvalKind::ImageNormalize,
    },
    InterpreterIntrinsicSpec {
        namespace: "Image",
        name: "resize",
        variant: "ImageResize",
        eval_kind: InterpreterEvalKind::ImageResize,
    },
    InterpreterIntrinsicSpec {
        namespace: "Image",
        name: "save",
        variant: "ImageSave",
        eval_kind: InterpreterEvalKind::ImageSave,
    },
    InterpreterIntrinsicSpec {
        namespace: "Image",
        name: "sharpen",
        variant: "ImageSharpen",
        eval_kind: InterpreterEvalKind::ImageSharpen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array",
        variant: "JsonArray",
        eval_kind: InterpreterEvalKind::JsonArray,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_bools",
        variant: "JsonArrayBools",
        eval_kind: InterpreterEvalKind::JsonArrayBools,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_contains_prefix",
        variant: "JsonArrayContainsPrefix",
        eval_kind: InterpreterEvalKind::JsonArrayContainsPrefix,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_contains_string",
        variant: "JsonArrayContainsString",
        eval_kind: InterpreterEvalKind::JsonArrayContainsString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_contains_substring",
        variant: "JsonArrayContainsSubstring",
        eval_kind: InterpreterEvalKind::JsonArrayContainsSubstring,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_count_where",
        variant: "JsonArrayCountWhere",
        eval_kind: InterpreterEvalKind::JsonArrayCountWhere,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_fold",
        variant: "JsonArrayFold",
        eval_kind: InterpreterEvalKind::JsonArrayFold,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_get",
        variant: "JsonArrayGet",
        eval_kind: InterpreterEvalKind::JsonArrayGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_ints",
        variant: "JsonArrayInts",
        eval_kind: InterpreterEvalKind::JsonArrayInts,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_len",
        variant: "JsonArrayLen",
        eval_kind: InterpreterEvalKind::JsonArrayLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "array_strings",
        variant: "JsonArrayStrings",
        eval_kind: InterpreterEvalKind::JsonArrayStrings,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "as_bool",
        variant: "JsonAsBool",
        eval_kind: InterpreterEvalKind::JsonAsBool,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "as_int",
        variant: "JsonAsInt",
        eval_kind: InterpreterEvalKind::JsonAsInt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "as_string",
        variant: "JsonAsString",
        eval_kind: InterpreterEvalKind::JsonAsString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at",
        variant: "JsonAt",
        eval_kind: InterpreterEvalKind::JsonAt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_bool",
        variant: "JsonAtBool",
        eval_kind: InterpreterEvalKind::JsonAtBool,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_bool_or",
        variant: "JsonAtBoolOr",
        eval_kind: InterpreterEvalKind::JsonAtBoolOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_int",
        variant: "JsonAtInt",
        eval_kind: InterpreterEvalKind::JsonAtInt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_int_or",
        variant: "JsonAtIntOr",
        eval_kind: InterpreterEvalKind::JsonAtIntOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_optional",
        variant: "JsonAtOptional",
        eval_kind: InterpreterEvalKind::JsonAtOptional,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_optional_bool",
        variant: "JsonAtOptionalBool",
        eval_kind: InterpreterEvalKind::JsonAtOptionalBool,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_optional_int",
        variant: "JsonAtOptionalInt",
        eval_kind: InterpreterEvalKind::JsonAtOptionalInt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_optional_string",
        variant: "JsonAtOptionalString",
        eval_kind: InterpreterEvalKind::JsonAtOptionalString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_or",
        variant: "JsonAtOr",
        eval_kind: InterpreterEvalKind::JsonAtOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_string",
        variant: "JsonAtString",
        eval_kind: InterpreterEvalKind::JsonAtString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_string_or",
        variant: "JsonAtStringOr",
        eval_kind: InterpreterEvalKind::JsonAtStringOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_to_string",
        variant: "JsonAtToString",
        eval_kind: InterpreterEvalKind::JsonAtToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "at_to_string_or",
        variant: "JsonAtToStringOr",
        eval_kind: InterpreterEvalKind::JsonAtToStringOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "bool_field",
        variant: "JsonBoolField",
        eval_kind: InterpreterEvalKind::JsonBoolField,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "bool_at",
        variant: "JsonBoolAt",
        eval_kind: InterpreterEvalKind::JsonBoolAt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "bool_at_or",
        variant: "JsonBoolAtOr",
        eval_kind: InterpreterEvalKind::JsonBoolAtOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "clone",
        variant: "JsonClone",
        eval_kind: InterpreterEvalKind::JsonClone,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "field",
        variant: "JsonField",
        eval_kind: InterpreterEvalKind::JsonField,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "field_bool",
        variant: "JsonFieldBool",
        eval_kind: InterpreterEvalKind::JsonFieldBool,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "field_int",
        variant: "JsonFieldInt",
        eval_kind: InterpreterEvalKind::JsonFieldInt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "field_optional",
        variant: "JsonFieldOptional",
        eval_kind: InterpreterEvalKind::JsonFieldOptional,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "field_optional_bool",
        variant: "JsonFieldOptionalBool",
        eval_kind: InterpreterEvalKind::JsonFieldOptionalBool,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "field_optional_int",
        variant: "JsonFieldOptionalInt",
        eval_kind: InterpreterEvalKind::JsonFieldOptionalInt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "field_optional_string",
        variant: "JsonFieldOptionalString",
        eval_kind: InterpreterEvalKind::JsonFieldOptionalString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "field_string",
        variant: "JsonFieldString",
        eval_kind: InterpreterEvalKind::JsonFieldString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "int_field",
        variant: "JsonIntField",
        eval_kind: InterpreterEvalKind::JsonIntField,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "int_at",
        variant: "JsonIntAt",
        eval_kind: InterpreterEvalKind::JsonIntAt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "int_at_or",
        variant: "JsonIntAtOr",
        eval_kind: InterpreterEvalKind::JsonIntAtOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "is_array",
        variant: "JsonIsArray",
        eval_kind: InterpreterEvalKind::JsonIsArray,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "is_null",
        variant: "JsonIsNull",
        eval_kind: InterpreterEvalKind::JsonIsNull,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "is_object",
        variant: "JsonIsObject",
        eval_kind: InterpreterEvalKind::JsonIsObject,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "json_parse",
        variant: "JsonJsonParse",
        eval_kind: InterpreterEvalKind::JsonParse,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "json_bool_at_or",
        variant: "JsonJsonBoolAtOr",
        eval_kind: InterpreterEvalKind::JsonBoolAtOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "json_int_at_or",
        variant: "JsonJsonIntAtOr",
        eval_kind: InterpreterEvalKind::JsonIntAtOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "json_string_at_or",
        variant: "JsonJsonStringAtOr",
        eval_kind: InterpreterEvalKind::JsonStringAtOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "kind",
        variant: "JsonKind",
        eval_kind: InterpreterEvalKind::JsonKind,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "object",
        variant: "JsonObject",
        eval_kind: InterpreterEvalKind::JsonObject,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "object_keys",
        variant: "JsonObjectKeys",
        eval_kind: InterpreterEvalKind::JsonObjectKeys,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "object_len",
        variant: "JsonObjectLen",
        eval_kind: InterpreterEvalKind::JsonObjectLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "parse",
        variant: "JsonParse",
        eval_kind: InterpreterEvalKind::JsonParse,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "parse_file",
        variant: "JsonParseFile",
        eval_kind: InterpreterEvalKind::JsonParseFile,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "quote_string",
        variant: "JsonQuoteString",
        eval_kind: InterpreterEvalKind::JsonQuoteString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "raw_field",
        variant: "JsonRawField",
        eval_kind: InterpreterEvalKind::JsonRawField,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "string_array",
        variant: "JsonStringArray",
        eval_kind: InterpreterEvalKind::JsonStringArray,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "string_at",
        variant: "JsonStringAt",
        eval_kind: InterpreterEvalKind::JsonStringAt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "string_at_or",
        variant: "JsonStringAtOr",
        eval_kind: InterpreterEvalKind::JsonStringAtOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "string_field",
        variant: "JsonStringField",
        eval_kind: InterpreterEvalKind::JsonStringField,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "strings",
        variant: "JsonStrings",
        eval_kind: InterpreterEvalKind::JsonStrings,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "to_string",
        variant: "JsonToString",
        eval_kind: InterpreterEvalKind::JsonToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "to_string_at",
        variant: "JsonToStringAt",
        eval_kind: InterpreterEvalKind::JsonToStringAt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "to_string_at_or",
        variant: "JsonToStringAtOr",
        eval_kind: InterpreterEvalKind::JsonToStringAtOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "value",
        variant: "JsonValue",
        eval_kind: InterpreterEvalKind::JsonValue,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "value_at",
        variant: "JsonValueAt",
        eval_kind: InterpreterEvalKind::JsonAt,
    },
    InterpreterIntrinsicSpec {
        namespace: "Json",
        name: "values",
        variant: "JsonValues",
        eval_kind: InterpreterEvalKind::JsonValues,
    },
    InterpreterIntrinsicSpec {
        namespace: "JsonError",
        name: "message",
        variant: "JsonErrorMessage",
        eval_kind: InterpreterEvalKind::JsonErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "Option",
        name: "and_then",
        variant: "OptionAndThen",
        eval_kind: InterpreterEvalKind::OptionAndThen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Option",
        name: "filter",
        variant: "OptionFilter",
        eval_kind: InterpreterEvalKind::OptionFilter,
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
        name: "map",
        variant: "OptionMap",
        eval_kind: InterpreterEvalKind::OptionMap,
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
        namespace: "Option",
        name: "unwrap_or_else",
        variant: "OptionUnwrapOrElse",
        eval_kind: InterpreterEvalKind::OptionUnwrapOrElse,
    },
    InterpreterIntrinsicSpec {
        namespace: "Ord",
        name: "compare",
        variant: "OrdCompare",
        eval_kind: InterpreterEvalKind::OrdCompare,
    },
    InterpreterIntrinsicSpec {
        namespace: "OS",
        name: "close",
        variant: "OsClose",
        eval_kind: InterpreterEvalKind::OsClose,
    },
    InterpreterIntrinsicSpec {
        namespace: "Patch",
        name: "apply_text",
        variant: "PatchApplyText",
        eval_kind: InterpreterEvalKind::PatchApplyText,
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
        namespace: "Pipeline",
        name: "collect",
        variant: "PipelineCollect",
        eval_kind: InterpreterEvalKind::PipelineCollect,
    },
    InterpreterIntrinsicSpec {
        namespace: "Pipeline",
        name: "each",
        variant: "PipelineEach",
        eval_kind: InterpreterEvalKind::PipelineEach,
    },
    InterpreterIntrinsicSpec {
        namespace: "Pipeline",
        name: "filter",
        variant: "PipelineFilter",
        eval_kind: InterpreterEvalKind::PipelineFilter,
    },
    InterpreterIntrinsicSpec {
        namespace: "Pipeline",
        name: "map",
        variant: "PipelineMap",
        eval_kind: InterpreterEvalKind::PipelineMap,
    },
    InterpreterIntrinsicSpec {
        namespace: "Pipeline",
        name: "try_map",
        variant: "PipelineTryMap",
        eval_kind: InterpreterEvalKind::PipelineTryMap,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run",
        variant: "ProcessRun",
        eval_kind: InterpreterEvalKind::ProcessRun,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_async",
        variant: "ProcessRunAsync",
        eval_kind: InterpreterEvalKind::ProcessRunAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_many_stdout",
        variant: "ProcessRunManyStdout",
        eval_kind: InterpreterEvalKind::ProcessRunManyStdout,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_many_stdout_async",
        variant: "ProcessRunManyStdoutAsync",
        eval_kind: InterpreterEvalKind::ProcessRunManyStdoutAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_many_stdout_timeout",
        variant: "ProcessRunManyStdoutTimeout",
        eval_kind: InterpreterEvalKind::ProcessRunManyStdoutTimeout,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_many_stdout_timeout_async",
        variant: "ProcessRunManyStdoutTimeoutAsync",
        eval_kind: InterpreterEvalKind::ProcessRunManyStdoutTimeoutAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_request",
        variant: "ProcessRunRequest",
        eval_kind: InterpreterEvalKind::ProcessRunRequest,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_request_async",
        variant: "ProcessRunRequestAsync",
        eval_kind: InterpreterEvalKind::ProcessRunRequestAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_request_cancellable_async",
        variant: "ProcessRunRequestCancellableAsync",
        eval_kind: InterpreterEvalKind::ProcessRunRequestCancellableAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_stdout",
        variant: "ProcessRunStdout",
        eval_kind: InterpreterEvalKind::ProcessRunStdout,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_stdout_async",
        variant: "ProcessRunStdoutAsync",
        eval_kind: InterpreterEvalKind::ProcessRunStdoutAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_stdout_timeout",
        variant: "ProcessRunStdoutTimeout",
        eval_kind: InterpreterEvalKind::ProcessRunStdoutTimeout,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_stdout_timeout_async",
        variant: "ProcessRunStdoutTimeoutAsync",
        eval_kind: InterpreterEvalKind::ProcessRunStdoutTimeoutAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_timeout",
        variant: "ProcessRunTimeout",
        eval_kind: InterpreterEvalKind::ProcessRunTimeout,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "run_timeout_async",
        variant: "ProcessRunTimeoutAsync",
        eval_kind: InterpreterEvalKind::ProcessRunTimeoutAsync,
    },
    InterpreterIntrinsicSpec {
        namespace: "Process",
        name: "stream",
        variant: "ProcessStream",
        eval_kind: InterpreterEvalKind::ProcessStream,
    },
    InterpreterIntrinsicSpec {
        namespace: "ResourcePool",
        name: "discard",
        variant: "ResourcePoolDiscard",
        eval_kind: InterpreterEvalKind::ResourcePoolDiscard,
    },
    InterpreterIntrinsicSpec {
        namespace: "ResourcePool",
        name: "stats",
        variant: "ResourcePoolStats",
        eval_kind: InterpreterEvalKind::ResourcePoolStats,
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
        namespace: "Receiver",
        name: "close",
        variant: "ReceiverClose",
        eval_kind: InterpreterEvalKind::ReceiverClose,
    },
    InterpreterIntrinsicSpec {
        namespace: "Receiver",
        name: "into_stream",
        variant: "ReceiverIntoStream",
        eval_kind: InterpreterEvalKind::ReceiverIntoStream,
    },
    InterpreterIntrinsicSpec {
        namespace: "Receiver",
        name: "recv",
        variant: "ReceiverRecv",
        eval_kind: InterpreterEvalKind::ReceiverRecv,
    },
    InterpreterIntrinsicSpec {
        namespace: "Receiver",
        name: "recv_cancellable",
        variant: "ReceiverRecvCancellable",
        eval_kind: InterpreterEvalKind::ReceiverRecvCancellable,
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
        namespace: "Result",
        name: "err",
        variant: "ResultErr",
        eval_kind: InterpreterEvalKind::ResultErr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "err_message",
        variant: "ResultErrMessage",
        eval_kind: InterpreterEvalKind::ResultErrMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "and_then",
        variant: "ResultAndThen",
        eval_kind: InterpreterEvalKind::ResultAndThen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "is_err",
        variant: "ResultIsErr",
        eval_kind: InterpreterEvalKind::ResultIsErr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "is_ok",
        variant: "ResultIsOk",
        eval_kind: InterpreterEvalKind::ResultIsOk,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "map",
        variant: "ResultMap",
        eval_kind: InterpreterEvalKind::ResultMap,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "map_error",
        variant: "ResultMapError",
        eval_kind: InterpreterEvalKind::ResultMapError,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "ok",
        variant: "ResultOk",
        eval_kind: InterpreterEvalKind::ResultOk,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "unwrap_or",
        variant: "ResultUnwrapOr",
        eval_kind: InterpreterEvalKind::ResultUnwrapOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "Result",
        name: "unwrap_or_else",
        variant: "ResultUnwrapOrElse",
        eval_kind: InterpreterEvalKind::ResultUnwrapOrElse,
    },
    InterpreterIntrinsicSpec {
        namespace: "RuleLoader",
        name: "load_rules",
        variant: "RuleLoaderLoadRules",
        eval_kind: InterpreterEvalKind::RuleLoaderLoadRules,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "clear",
        variant: "SetClear",
        eval_kind: InterpreterEvalKind::SetClear,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "contains",
        variant: "SetContains",
        eval_kind: InterpreterEvalKind::SetContains,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "difference",
        variant: "SetDifference",
        eval_kind: InterpreterEvalKind::SetDifference,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "for_each",
        variant: "SetForEach",
        eval_kind: InterpreterEvalKind::SetForEach,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "insert",
        variant: "SetInsert",
        eval_kind: InterpreterEvalKind::SetInsert,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "intersection",
        variant: "SetIntersection",
        eval_kind: InterpreterEvalKind::SetIntersection,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "is_empty",
        variant: "SetIsEmpty",
        eval_kind: InterpreterEvalKind::SetIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "is_subset",
        variant: "SetIsSubset",
        eval_kind: InterpreterEvalKind::SetIsSubset,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "len",
        variant: "SetLen",
        eval_kind: InterpreterEvalKind::SetLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "new",
        variant: "SetNew",
        eval_kind: InterpreterEvalKind::SetNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "remove",
        variant: "SetRemove",
        eval_kind: InterpreterEvalKind::SetRemove,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "to_list",
        variant: "SetToList",
        eval_kind: InterpreterEvalKind::SetToList,
    },
    InterpreterIntrinsicSpec {
        namespace: "Set",
        name: "union",
        variant: "SetUnion",
        eval_kind: InterpreterEvalKind::SetUnion,
    },
    InterpreterIntrinsicSpec {
        namespace: "Sender",
        name: "close",
        variant: "SenderClose",
        eval_kind: InterpreterEvalKind::SenderClose,
    },
    InterpreterIntrinsicSpec {
        namespace: "Sender",
        name: "send",
        variant: "SenderSend",
        eval_kind: InterpreterEvalKind::SenderSend,
    },
    InterpreterIntrinsicSpec {
        namespace: "Sender",
        name: "send_cancellable",
        variant: "SenderSendCancellable",
        eval_kind: InterpreterEvalKind::SenderSendCancellable,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "clear",
        variant: "SortedMapClear",
        eval_kind: InterpreterEvalKind::SortedMapClear,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "contains_key",
        variant: "SortedMapContainsKey",
        eval_kind: InterpreterEvalKind::SortedMapContainsKey,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "get",
        variant: "SortedMapGet",
        eval_kind: InterpreterEvalKind::SortedMapGet,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "insert",
        variant: "SortedMapInsert",
        eval_kind: InterpreterEvalKind::SortedMapInsert,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "is_empty",
        variant: "SortedMapIsEmpty",
        eval_kind: InterpreterEvalKind::SortedMapIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "keys",
        variant: "SortedMapKeys",
        eval_kind: InterpreterEvalKind::SortedMapKeys,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "len",
        variant: "SortedMapLen",
        eval_kind: InterpreterEvalKind::SortedMapLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "new",
        variant: "SortedMapNew",
        eval_kind: InterpreterEvalKind::SortedMapNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "remove",
        variant: "SortedMapRemove",
        eval_kind: InterpreterEvalKind::SortedMapRemove,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedMap",
        name: "values",
        variant: "SortedMapValues",
        eval_kind: InterpreterEvalKind::SortedMapValues,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedSet",
        name: "clear",
        variant: "SortedSetClear",
        eval_kind: InterpreterEvalKind::SortedSetClear,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedSet",
        name: "contains",
        variant: "SortedSetContains",
        eval_kind: InterpreterEvalKind::SortedSetContains,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedSet",
        name: "insert",
        variant: "SortedSetInsert",
        eval_kind: InterpreterEvalKind::SortedSetInsert,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedSet",
        name: "is_empty",
        variant: "SortedSetIsEmpty",
        eval_kind: InterpreterEvalKind::SortedSetIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedSet",
        name: "len",
        variant: "SortedSetLen",
        eval_kind: InterpreterEvalKind::SortedSetLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedSet",
        name: "new",
        variant: "SortedSetNew",
        eval_kind: InterpreterEvalKind::SortedSetNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedSet",
        name: "remove",
        variant: "SortedSetRemove",
        eval_kind: InterpreterEvalKind::SortedSetRemove,
    },
    InterpreterIntrinsicSpec {
        namespace: "SortedSet",
        name: "to_list",
        variant: "SortedSetToList",
        eval_kind: InterpreterEvalKind::SortedSetToList,
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
        name: "env",
        variant: "StringEnv",
        eval_kind: InterpreterEvalKind::StringEnv,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "env_or",
        variant: "StringEnvOr",
        eval_kind: InterpreterEvalKind::StringEnvOr,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "join",
        variant: "StringJoin",
        eval_kind: InterpreterEvalKind::StringJoin,
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
        namespace: "Stream",
        name: "collect_list",
        variant: "StreamCollectList",
        eval_kind: InterpreterEvalKind::StreamCollectList,
    },
    InterpreterIntrinsicSpec {
        namespace: "Stream",
        name: "from_list",
        variant: "StreamFromList",
        eval_kind: InterpreterEvalKind::StreamFromList,
    },
    InterpreterIntrinsicSpec {
        namespace: "Stream",
        name: "next",
        variant: "StreamNext",
        eval_kind: InterpreterEvalKind::StreamNext,
    },
    InterpreterIntrinsicSpec {
        namespace: "Tcp",
        name: "connect",
        variant: "TcpConnect",
        eval_kind: InterpreterEvalKind::TcpConnect,
    },
    InterpreterIntrinsicSpec {
        namespace: "TcpError",
        name: "message",
        variant: "TcpErrorMessage",
        eval_kind: InterpreterEvalKind::TcpErrorMessage,
    },
    InterpreterIntrinsicSpec {
        namespace: "TcpStream",
        name: "read",
        variant: "TcpStreamRead",
        eval_kind: InterpreterEvalKind::TcpStreamRead,
    },
    InterpreterIntrinsicSpec {
        namespace: "TcpStream",
        name: "shutdown",
        variant: "TcpStreamShutdown",
        eval_kind: InterpreterEvalKind::TcpStreamShutdown,
    },
    InterpreterIntrinsicSpec {
        namespace: "TcpStream",
        name: "write",
        variant: "TcpStreamWrite",
        eval_kind: InterpreterEvalKind::TcpStreamWrite,
    },
    InterpreterIntrinsicSpec {
        namespace: "TcpStream",
        name: "write_all",
        variant: "TcpStreamWriteAll",
        eval_kind: InterpreterEvalKind::TcpStreamWriteAll,
    },
    InterpreterIntrinsicSpec {
        namespace: "TempDir",
        name: "keep",
        variant: "TempDirKeep",
        eval_kind: InterpreterEvalKind::TempDirKeep,
    },
    InterpreterIntrinsicSpec {
        namespace: "TempDir",
        name: "new",
        variant: "TempDirNew",
        eval_kind: InterpreterEvalKind::TempDirNew,
    },
    InterpreterIntrinsicSpec {
        namespace: "TempDir",
        name: "new_in",
        variant: "TempDirNewIn",
        eval_kind: InterpreterEvalKind::TempDirNewIn,
    },
    InterpreterIntrinsicSpec {
        namespace: "TempDir",
        name: "path",
        variant: "TempDirPath",
        eval_kind: InterpreterEvalKind::TempDirPath,
    },
    InterpreterIntrinsicSpec {
        namespace: "Timer",
        name: "sleep",
        variant: "TimerSleep",
        eval_kind: InterpreterEvalKind::TimerSleep,
    },
    InterpreterIntrinsicSpec {
        namespace: "Timer",
        name: "sleep_cancellable",
        variant: "TimerSleepCancellable",
        eval_kind: InterpreterEvalKind::TimerSleepCancellable,
    },
    InterpreterIntrinsicSpec {
        namespace: "Timer",
        name: "sleep_until",
        variant: "TimerSleepUntil",
        eval_kind: InterpreterEvalKind::TimerSleepUntil,
    },
    InterpreterIntrinsicSpec {
        namespace: "Toml",
        name: "parse_file",
        variant: "TomlParseFile",
        eval_kind: InterpreterEvalKind::TomlParseFile,
    },
    InterpreterIntrinsicSpec {
        namespace: "Url",
        name: "decode_component",
        variant: "UrlDecodeComponent",
        eval_kind: InterpreterEvalKind::UrlDecodeComponent,
    },
    InterpreterIntrinsicSpec {
        namespace: "Url",
        name: "encode_component",
        variant: "UrlEncodeComponent",
        eval_kind: InterpreterEvalKind::UrlEncodeComponent,
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
        namespace: "WebSocket",
        name: "close",
        variant: "WebSocketClose",
        eval_kind: InterpreterEvalKind::WebSocketClose,
    },
    InterpreterIntrinsicSpec {
        namespace: "WebSocket",
        name: "connect",
        variant: "WebSocketConnect",
        eval_kind: InterpreterEvalKind::WebSocketConnect,
    },
    InterpreterIntrinsicSpec {
        namespace: "WebSocket",
        name: "recv_bytes",
        variant: "WebSocketRecvBytes",
        eval_kind: InterpreterEvalKind::WebSocketRecvBytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "WebSocket",
        name: "recv_text",
        variant: "WebSocketRecvText",
        eval_kind: InterpreterEvalKind::WebSocketRecvText,
    },
    InterpreterIntrinsicSpec {
        namespace: "WebSocket",
        name: "send_bytes",
        variant: "WebSocketSendBytes",
        eval_kind: InterpreterEvalKind::WebSocketSendBytes,
    },
    InterpreterIntrinsicSpec {
        namespace: "WebSocket",
        name: "send_text",
        variant: "WebSocketSendText",
        eval_kind: InterpreterEvalKind::WebSocketSendText,
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
        namespace: "Yaml",
        name: "parse",
        variant: "YamlParse",
        eval_kind: InterpreterEvalKind::YamlParse,
    },
    InterpreterIntrinsicSpec {
        namespace: "Yaml",
        name: "parse_file",
        variant: "YamlParseFile",
        eval_kind: InterpreterEvalKind::YamlParseFile,
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
        functions.extend(collect_interpreter_interface_functions(&source));
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
    let mut by_namespace: BTreeMap<&str, Vec<&InterpreterIntrinsicSpec>> = BTreeMap::new();
    for intrinsic in INTERPRETER_INTRINSICS {
        by_namespace
            .entry(intrinsic.namespace)
            .or_default()
            .push(intrinsic);
    }

    let mut out = String::new();
    out.push_str(
        "fn eval_generated_runtime_intrinsic(\n    interpreter: &mut Interpreter<'_>,\n    intrinsic: InterpreterIntrinsic,\n    args: &[crate::hir::HirCallArg],\n) -> Result<Value, EvalError> {\n    match intrinsic {\n",
    );
    for (namespace, intrinsics) in &by_namespace {
        out.push_str("        ");
        for (index, intrinsic) in intrinsics.iter().enumerate() {
            if index > 0 {
                out.push_str("\n        | ");
            }
            out.push_str("InterpreterIntrinsic::");
            out.push_str(intrinsic.variant);
        }
        out.push_str(" => ");
        out.push_str(&interpreter_intrinsic_helper_name(namespace));
        out.push_str("(interpreter, intrinsic, args),\n");
    }
    out.push_str("    }\n}\n");

    for (namespace, intrinsics) in by_namespace {
        out.push('\n');
        out.push_str("#[allow(unused_variables)]\nfn ");
        out.push_str(&interpreter_intrinsic_helper_name(namespace));
        out.push_str(
            "(\n    interpreter: &mut Interpreter<'_>,\n    intrinsic: InterpreterIntrinsic,\n    args: &[crate::hir::HirCallArg],\n) -> Result<Value, EvalError> {\n    match intrinsic {\n",
        );
        for intrinsic in intrinsics {
            out.push_str("        InterpreterIntrinsic::");
            out.push_str(intrinsic.variant);
            out.push_str(" => ");
            out.push_str(eval_kind_body(intrinsic.eval_kind));
            out.push_str(",\n");
        }
        out.push_str("        _ => unreachable!(\"generated interpreter intrinsic helper received mismatched namespace\"),\n    }\n}\n");
    }
    out
}

fn interpreter_intrinsic_helper_name(namespace: &str) -> String {
    let mut name = String::from("eval_generated_runtime_intrinsic_");
    for ch in namespace.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else {
            name.push('_');
        }
    }
    name
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
        InterpreterEvalKind::Base64Decode => {
            "{\n            let text = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                base64::Engine::decode(\n                    &base64::engine::general_purpose::STANDARD,\n                    expect_string(text)?,\n                )\n                .map(Value::Bytes)\n                .map_err(decode_error),\n            ))\n        }"
        }
        InterpreterEvalKind::Base64DecodeString => {
            "{\n            let text = interpreter.eval_first_arg(args)?;\n            let result = base64::Engine::decode(\n                &base64::engine::general_purpose::STANDARD,\n                expect_string(text)?,\n            )\n            .map_err(decode_error)\n            .and_then(|bytes| String::from_utf8(bytes).map(Value::String).map_err(decode_error));\n            Ok(result_value(result))\n        }"
        }
        InterpreterEvalKind::Base64Encode => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(base64::Engine::encode(\n                &base64::engine::general_purpose::STANDARD,\n                expect_string(value)?.as_bytes(),\n            )))\n        }"
        }
        InterpreterEvalKind::Base64EncodeBytes => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(base64::Engine::encode(\n                &base64::engine::general_purpose::STANDARD,\n                expect_bytes(value)?,\n            )))\n        }"
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
        InterpreterEvalKind::CacheNew => "{\n            Ok(Value::Map(Vec::new()))\n        }",
        InterpreterEvalKind::CacheGet => {
            "{\n            let cache = interpreter.eval_named_or_positional_arg(args, \"cache\", 0)?;\n            let bytes = expect_map(cache)?\n                .into_iter()\n                .find_map(|(_, value)| expect_string(value).ok())\n                .map(String::into_bytes)\n                .unwrap_or_default();\n            Ok(image_value(\n                bytes,\n                None,\n                None,\n                vec![\"cache-get\".to_string()],\n            ))\n        }"
        }
        InterpreterEvalKind::CacheInsert => {
            "{\n            let cache_name = interpreter.mut_arg_local_name(args, \"cache\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 2)?;\n            let mut cache = expect_map(interpreter.lookup(cache_name)?)?;\n            map_insert(&mut cache, key, value);\n            interpreter.assign(cache_name, Value::Map(cache))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::CacheLookup => {
            "{\n            let cache = interpreter.eval_named_or_positional_arg(args, \"cache\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            Ok(match map_get(&expect_map(cache)?, &key) {\n                Some(value) => value,\n                None => Value::String(String::new()),\n            })\n        }"
        }
        InterpreterEvalKind::ChannelBounded => {
            "{\n            let capacity = interpreter.eval_named_or_positional_arg(args, \"capacity\", 0)?;\n            let capacity = expect_int(capacity)?;\n            Ok(result_value(if capacity <= 0 {\n                Err(channel_error(\"channel capacity must be positive\"))\n            } else {\n                let id = interpreter.next_channel_id;\n                interpreter.next_channel_id = interpreter.next_channel_id.saturating_add(1);\n                interpreter.channels.insert(id, InterpreterChannel::new(capacity as usize));\n                Ok(channel_value(id, capacity, false))\n            }))\n        }"
        }
        InterpreterEvalKind::ChannelSender => {
            "{\n            let channel = interpreter.eval_named_or_positional_arg(args, \"channel\", 0)?;\n            let channel = expect_channel(channel)?;\n            let state = interpreter.channel_state_mut(channel.id)?;\n            state.senders = state.senders.saturating_add(1);\n            Ok(sender_value(channel.id, false))\n        }"
        }
        InterpreterEvalKind::ChannelReceiver => {
            "{\n            let channel_name = interpreter.mut_arg_local_name(args, \"channel\", 0)?;\n            let mut channel = expect_channel(interpreter.lookup(channel_name)?)?;\n            let already_taken = interpreter\n                .channels\n                .get(&channel.id)\n                .map(|state| state.receiver_taken)\n                .unwrap_or(channel.receiver_taken);\n            Ok(result_value(if already_taken {\n                Err(channel_error(\"channel receiver already taken\"))\n            } else {\n                channel.receiver_taken = true;\n                interpreter.channel_state_mut(channel.id)?.receiver_taken = true;\n                interpreter.assign(channel_name, channel.to_value())?;\n                Ok(receiver_value(channel.id, false))\n            }))\n        }"
        }
        InterpreterEvalKind::ChannelErrorMessage => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            read_field(&value, \"message\")\n        }"
        }
        InterpreterEvalKind::DecodeErrorMessage => {
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
        InterpreterEvalKind::CancellationSourceCancel => {
            "{\n            let source_name = interpreter.mut_arg_local_name(args, \"source\", 0)?;\n            let id = expect_cancellation_id(interpreter.lookup(source_name)?, \"CancellationSource\")?;\n            interpreter.cancellation_flags.insert(id, true);\n            interpreter.assign(source_name, cancellation_source_value(id))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::CancellationSourceNew => {
            "{\n            let id = interpreter.next_cancellation_id;\n            interpreter.next_cancellation_id = interpreter.next_cancellation_id.saturating_add(1);\n            interpreter.cancellation_flags.insert(id, false);\n            Ok(cancellation_source_value(id))\n        }"
        }
        InterpreterEvalKind::CancellationSourceToken => {
            "{\n            let source = interpreter.eval_named_or_positional_arg(args, \"source\", 0)?;\n            Ok(cancellation_token_value(expect_cancellation_id(\n                source,\n                \"CancellationSource\",\n            )?))\n        }"
        }
        InterpreterEvalKind::CancellationTokenIsCancelled => {
            "{\n            let token = interpreter.eval_named_or_positional_arg(args, \"token\", 0)?;\n            let id = expect_cancellation_id(token, \"CancellationToken\")?;\n            Ok(Value::Bool(\n                interpreter.cancellation_flags.get(&id).copied().unwrap_or(false),\n            ))\n        }"
        }
        InterpreterEvalKind::ClockNow => {
            "{\n            Ok(instant_value(clock_system_unix_ms()))\n        }"
        }
        InterpreterEvalKind::ClockSystemUnixMs => {
            "{\n            Ok(Value::Int(clock_system_unix_ms()))\n        }"
        }
        InterpreterEvalKind::ConfigLoad => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            Ok(result_value(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map(|text| config_value(config_name_from_text(&text)))\n                    .map_err(config_error),\n            ))\n        }"
        }
        InterpreterEvalKind::ConfigName => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            Ok(Value::String(expect_config_value_name(value)?))\n        }"
        }
        InterpreterEvalKind::ConfigNew => {
            "{\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 0)?;\n            let rules = interpreter.eval_named_or_positional_arg(args, \"rules\", 1)?;\n            Ok(config_rules_value(\n                expect_string(name)?,\n                expect_list(rules)?.len() as i64,\n            ))\n        }"
        }
        InterpreterEvalKind::ConfigRuleCount => {
            "{\n            let config = interpreter.eval_named_or_positional_arg(args, \"config\", 0)?;\n            Ok(Value::Int(expect_config_rule_count(config)?))\n        }"
        }
        InterpreterEvalKind::ConfigStoreName => {
            "{\n            let store = interpreter.eval_named_or_positional_arg(args, \"store\", 0)?;\n            Ok(Value::String(expect_config_store_name(store)?))\n        }"
        }
        InterpreterEvalKind::ConfigStoreNew => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            Ok(config_store_value(expect_config_value_name(value)?))\n        }"
        }
        InterpreterEvalKind::ConfigStoreReplace => {
            "{\n            let store_name = interpreter.mut_arg_local_name(args, \"store\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            interpreter.assign(\n                store_name,\n                config_store_value(expect_config_value_name(value)?),\n            )?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::CsvOpenRead => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let path = expect_string(path)?;\n            Ok(result_value(\n                std::fs::File::open(&path)\n                    .map(|_| file_value(path, \"read\", 0))\n                    .map_err(csv_error_from_io),\n            ))\n        }"
        }
        InterpreterEvalKind::CsvParseRow => {
            "{\n            let buffer = interpreter.eval_named_or_positional_arg(args, \"buffer\", 0)?;\n            Ok(result_value(csv_parse_row_value(&expect_row_buffer_bytes(\n                buffer,\n            )?)))\n        }"
        }
        InterpreterEvalKind::CsvReadInto => {
            "{\n            let file_name = interpreter.mut_arg_local_name(args, \"file\", 0)?;\n            let buffer_name = interpreter.mut_arg_local_name(args, \"buffer\", 1)?;\n            let mut file = expect_file(interpreter.lookup(file_name)?)?;\n            let result = file_read_remaining(&mut file).map_err(csv_error_from_io);\n            interpreter.assign(file_name, file.to_value())?;\n            Ok(match result {\n                Ok(bytes) => {\n                    interpreter.assign(buffer_name, row_buffer_value(bytes))?;\n                    value_ok(Value::Unit)\n                }\n                Err(error) => value_err(error),\n            })\n        }"
        }
        InterpreterEvalKind::CsvRows => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let _buffer_size = interpreter.eval_named_or_positional_arg(args, \"buffer_size\", 1)?;\n            Ok(result_value(\n                csv_rows_stream_value(&expect_string(path)?).map_err(channel_error),\n            ))\n        }"
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
        InterpreterEvalKind::DequeNew => "{\n            Ok(Value::List(Vec::new()))\n        }",
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
        InterpreterEvalKind::DeadlineAfter | InterpreterEvalKind::DeadlineAfterMs => {
            "{\n            let ms = interpreter\n                .eval_named_or_positional_arg(args, \"ms\", 0)\n                .or_else(|_| interpreter.eval_named_or_positional_arg(args, \"duration\", 0))?;\n            Ok(deadline_value(deadline_after_ms(expect_int(ms)?)))\n        }"
        }
        InterpreterEvalKind::DeadlineIsExpired => {
            "{\n            let deadline = interpreter.eval_named_or_positional_arg(args, \"deadline\", 0)?;\n            Ok(Value::Bool(\n                clock_system_unix_ms() >= expect_deadline_unix_ms(deadline)?,\n            ))\n        }"
        }
        InterpreterEvalKind::DeadlineRemainingMs => {
            "{\n            let deadline = interpreter.eval_named_or_positional_arg(args, \"deadline\", 0)?;\n            Ok(Value::Int(\n                expect_deadline_unix_ms(deadline)?\n                    .saturating_sub(clock_system_unix_ms())\n                    .max(0),\n            ))\n        }"
        }
        InterpreterEvalKind::DbClose => {
            "{\n            let fd = interpreter.eval_named_or_positional_arg(args, \"fd\", 0)?;\n            let _ = expect_int(fd)?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::DbConnectionOpen => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            Ok(db_connection_value(expect_string(url)?, Vec::new()))\n        }"
        }
        InterpreterEvalKind::DbConnectionQuery => {
            "{\n            let conn_name = interpreter.mut_arg_local_name(args, \"conn\", 0)?;\n            let sql = interpreter.eval_named_or_positional_arg(args, \"sql\", 1)?;\n            let sql = expect_string(sql)?;\n            let mut conn = expect_db_connection(interpreter.lookup(conn_name)?)?;\n            Ok(result_value(if sql.trim().is_empty() {\n                Err(db_error(\"SQL query is empty\"))\n            } else {\n                interpreter\n                    .stdout\n                    .push_str(&format!(\"db query on {}: {sql}\\n\", conn.url));\n                conn.queries.push(sql);\n                interpreter.assign(conn_name, conn.to_value())?;\n                Ok(Value::Unit)\n            }))\n        }"
        }
        InterpreterEvalKind::DbConnectionTryOpen => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let url = expect_string(url)?;\n            Ok(result_value(if url.trim().is_empty() {\n                Err(db_error(\"database URL is empty\"))\n            } else {\n                Ok(db_connection_value(url, Vec::new()))\n            }))\n        }"
        }
        InterpreterEvalKind::DirectoryCopyFile => {
            "{\n            let from = interpreter.eval_named_or_positional_arg(args, \"from\", 0)?;\n            let to = interpreter.eval_named_or_positional_arg(args, \"to\", 1)?;\n            Ok(file_result_unit(\n                std::fs::copy(expect_string(from)?, expect_string(to)?).map(|_| ()),\n            ))\n        }"
        }
        InterpreterEvalKind::DirectoryCreate => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(file_result_unit(std::fs::create_dir(expect_string(path)?)))\n        }"
        }
        InterpreterEvalKind::DirectoryCreateAll | InterpreterEvalKind::DirectoryCreateDirAll => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(file_result_unit(std::fs::create_dir_all(expect_string(\n                path,\n            )?)))\n        }"
        }
        InterpreterEvalKind::DirectoryExists => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(Path::new(&expect_string(path)?).exists()))\n        }"
        }
        InterpreterEvalKind::DirectoryIsDir => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(Path::new(&expect_string(path)?).is_dir()))\n        }"
        }
        InterpreterEvalKind::DirectoryIsFile => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(Path::new(&expect_string(path)?).is_file()))\n        }"
        }
        InterpreterEvalKind::DirectoryListFiles => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                directory_list_files(&PathBuf::from(expect_string(path)?))\n                    .map(|files| Value::List(files.into_iter().map(Value::String).collect()))\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::DirectoryListPaths => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                directory_list_paths(&PathBuf::from(expect_string(path)?))\n                    .map(|paths| {\n                        Value::List(\n                            paths\n                                .into_iter()\n                                .map(|path| Value::String(path.to_string_lossy().to_string()))\n                                .collect(),\n                        )\n                    })\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::DirectoryMetadata => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                std::fs::metadata(expect_string(path)?)\n                    .map(|metadata| Value::Struct {\n                        name: \"FileMetadata\".to_string(),\n                        fields: BTreeMap::from([\n                            (\"is_file\".to_string(), Value::Bool(metadata.is_file())),\n                            (\"is_dir\".to_string(), Value::Bool(metadata.is_dir())),\n                            (\"len\".to_string(), Value::Int(metadata.len() as i64)),\n                        ]),\n                    })\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::DirectoryReadString => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map(Value::String)\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::DirectoryRemoveDirAll => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(file_result_unit(std::fs::remove_dir_all(expect_string(\n                path,\n            )?)))\n        }"
        }
        InterpreterEvalKind::DirectoryRemoveFile => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(file_result_unit(std::fs::remove_file(expect_string(path)?)))\n        }"
        }
        InterpreterEvalKind::DirectoryRename => {
            "{\n            let from = interpreter.eval_named_or_positional_arg(args, \"from\", 0)?;\n            let to = interpreter.eval_named_or_positional_arg(args, \"to\", 1)?;\n            Ok(file_result_unit(std::fs::rename(\n                expect_string(from)?,\n                expect_string(to)?,\n            )))\n        }"
        }
        InterpreterEvalKind::DirectoryWriteString => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let content = interpreter.eval_named_or_positional_arg(args, \"content\", 1)?;\n            Ok(file_result_unit(std::fs::write(\n                expect_string(path)?,\n                expect_string(content)?,\n            )))\n        }"
        }
        InterpreterEvalKind::PathExists => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(Path::new(&expect_string(path)?).exists()))\n        }"
        }
        InterpreterEvalKind::PathIsDir => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(Path::new(&expect_string(path)?).is_dir()))\n        }"
        }
        InterpreterEvalKind::PathIsFile => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(Path::new(&expect_string(path)?).is_file()))\n        }"
        }
        InterpreterEvalKind::PathListFiles => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                directory_list_files(&PathBuf::from(expect_string(path)?))\n                    .map(|files| Value::List(files.into_iter().map(Value::String).collect()))\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::PathListPaths => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                directory_list_paths(&PathBuf::from(expect_string(path)?))\n                    .map(|paths| {\n                        Value::List(\n                            paths\n                                .into_iter()\n                                .map(|path| Value::String(path.to_string_lossy().to_string()))\n                                .collect(),\n                        )\n                    })\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::PathReadString => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map(Value::String)\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::PathWriteString => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 1)?;\n            Ok(file_result_unit(std::fs::write(\n                expect_string(path)?,\n                expect_string(text)?,\n            )))\n        }"
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
        InterpreterEvalKind::EnvironmentBindFunction => {
            "{\n            let env_name = interpreter.mut_arg_local_name(args, \"env\", 0)?;\n            let function = interpreter.eval_named_or_positional_arg(args, \"function\", 1)?;\n            let _ = expect_function_has_closure(function)?;\n            let (has_parent, _) = expect_environment_state(interpreter.lookup(env_name)?)?;\n            interpreter.assign(env_name, environment_value(has_parent, true))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::EnvironmentChild => {
            "{\n            let parent = interpreter.eval_named_or_positional_arg(args, \"parent\", 0)?;\n            let _ = expect_environment_state(parent)?;\n            Ok(environment_value(true, false))\n        }"
        }
        InterpreterEvalKind::EnvironmentHasFunction => {
            "{\n            let env = interpreter.eval_named_or_positional_arg(args, \"env\", 0)?;\n            let (_, has_function) = expect_environment_state(env)?;\n            Ok(Value::Bool(has_function))\n        }"
        }
        InterpreterEvalKind::EnvironmentHasParent => {
            "{\n            let env = interpreter.eval_named_or_positional_arg(args, \"env\", 0)?;\n            let (has_parent, _) = expect_environment_state(env)?;\n            Ok(Value::Bool(has_parent))\n        }"
        }
        InterpreterEvalKind::EnvironmentRoot => {
            "{\n            Ok(environment_value(false, false))\n        }"
        }
        InterpreterEvalKind::EnvCurrentDir => {
            "{\n            Ok(result_value(\n                std::env::current_dir()\n                    .map(|path| Value::String(path_to_string(&path)))\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::EnvGet => {
            "{\n            let name = interpreter.eval_first_arg(args)?;\n            Ok(value_option(\n                std::env::var(expect_string(name)?).ok(),\n                Value::String,\n            ))\n        }"
        }
        InterpreterEvalKind::EnvGetOrDefault => {
            "{\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 0)?;\n            let default = interpreter.eval_named_or_positional_arg(args, \"default\", 1)?;\n            Ok(Value::String(\n                std::env::var(expect_string(name)?).unwrap_or(expect_string(default)?),\n            ))\n        }"
        }
        InterpreterEvalKind::EnvHomeDir => {
            "{\n            Ok(value_option(\n                std::env::var(\"HOME\").ok().filter(|value| !value.is_empty()),\n                Value::String,\n            ))\n        }"
        }
        InterpreterEvalKind::EnvRunWorkspaceRoot => {
            "{\n            Ok(Value::String(path_to_string(Path::new(env!(\n                \"CARGO_MANIFEST_DIR\"\n            )))))\n        }"
        }
        InterpreterEvalKind::EnvSet => {
            "{\n            let _name = interpreter.eval_named_or_positional_arg(args, \"name\", 0)?;\n            let _value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            interpreter\n                .stderr\n                .push_str(\"[rsscript] warning: Env.set is a no-op in the safe runtime\\n\");\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::EnvSetCurrentDir => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            Ok(result_value(\n                std::env::set_current_dir(expect_string(path)?)\n                    .map(|_| Value::Unit)\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::EnvTempDir => {
            "{\n            Ok(Value::String(path_to_string(&std::env::temp_dir())))\n        }"
        }
        InterpreterEvalKind::FileAppendBytes => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let data = interpreter.eval_named_or_positional_arg(args, \"data\", 1)?;\n            Ok(file_append_result(\n                expect_string(path)?,\n                &expect_bytes(data)?,\n            ))\n        }"
        }
        InterpreterEvalKind::FileAppendString => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 1)?;\n            Ok(file_append_result(\n                expect_string(path)?,\n                expect_string(text)?.as_bytes(),\n            ))\n        }"
        }
        InterpreterEvalKind::FileBytesStream => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let chunk_size = interpreter.eval_named_or_positional_arg(args, \"chunk_size\", 1)?;\n            Ok(result_value(\n                file_bytes_stream_value(&expect_string(path)?, expect_int(chunk_size)?)\n                    .map_err(channel_error),\n            ))\n        }"
        }
        InterpreterEvalKind::FileExists => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(Path::new(&expect_string(path)?).exists()))\n        }"
        }
        InterpreterEvalKind::FileErrorMessage => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            read_field(&value, \"message\")\n        }"
        }
        InterpreterEvalKind::FalliblePipelineCollect => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::FalliblePipelineMap => {
            "{\n            let pipeline = interpreter.eval_named_or_positional_arg(args, \"pipeline\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            Ok(match result_payload(pipeline)? {\n                Ok(items) => {\n                    let mut mapped = Vec::new();\n                    for value in expect_list(items)? {\n                        mapped.push(interpreter.call_closure(mapper.clone(), vec![value])?);\n                    }\n                    value_ok(Value::List(mapped))\n                }\n                Err(error) => value_err(error),\n            })\n        }"
        }
        InterpreterEvalKind::FalliblePipelineFilter => {
            "{\n            let pipeline = interpreter.eval_named_or_positional_arg(args, \"pipeline\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            Ok(match result_payload(pipeline)? {\n                Ok(items) => {\n                    let mut filtered = Vec::new();\n                    for value in expect_list(items)? {\n                        if expect_bool(interpreter.call_closure(predicate.clone(), vec![value.clone()])?)? {\n                            filtered.push(value);\n                        }\n                    }\n                    value_ok(Value::List(filtered))\n                }\n                Err(error) => value_err(error),\n            })\n        }"
        }
        InterpreterEvalKind::FalliblePipelineEach => {
            "{\n            let pipeline = interpreter.eval_named_or_positional_arg(args, \"pipeline\", 0)?;\n            let action = interpreter.eval_named_or_positional_arg(args, \"action\", 1)?;\n            Ok(match result_payload(pipeline)? {\n                Ok(items) => {\n                    let items = expect_list(items)?;\n                    for value in items.iter().cloned() {\n                        let _ = interpreter.call_closure(action.clone(), vec![value])?;\n                    }\n                    value_ok(Value::List(items))\n                }\n                Err(error) => value_err(error),\n            })\n        }"
        }
        InterpreterEvalKind::FalliblePipelineTryMap => {
            "{\n            let pipeline = interpreter.eval_named_or_positional_arg(args, \"pipeline\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            match result_payload(pipeline)? {\n                Ok(items) => {\n                    let mut mapped = Vec::new();\n                    for value in expect_list(items)? {\n                        match result_payload(interpreter.call_closure(mapper.clone(), vec![value])?)? {\n                            Ok(value) => mapped.push(value),\n                            Err(error) => return Ok(value_err(error)),\n                        }\n                    }\n                    Ok(value_ok(Value::List(mapped)))\n                }\n                Err(error) => Ok(value_err(error)),\n            }\n        }"
        }
        InterpreterEvalKind::FileOpen | InterpreterEvalKind::FileOpenRead => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            let path = expect_string(path)?;\n            Ok(result_value(\n                std::fs::File::open(&path)\n                    .map(|_| file_value(path, \"read\", 0))\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::FileOpenWrite => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            let path = expect_string(path)?;\n            Ok(result_value(\n                std::fs::File::create(&path)\n                    .map(|_| file_value(path, \"write\", 0))\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::FileReadAll => {
            "{\n            let file_name = interpreter.mut_arg_local_name(args, \"file\", 0)?;\n            let mut file = expect_file(interpreter.lookup(file_name)?)?;\n            let result = file_read_remaining(&mut file)\n                .map(Value::Bytes)\n                .map_err(file_error);\n            interpreter.assign(file_name, file.to_value())?;\n            Ok(result_value(result))\n        }"
        }
        InterpreterEvalKind::FileReadAllAsync | InterpreterEvalKind::FileReadBytes => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                std::fs::read(expect_string(path)?)\n                    .map(Value::Bytes)\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::FileReadAllString => {
            "{\n            let file_name = interpreter.mut_arg_local_name(args, \"file\", 0)?;\n            let mut file = expect_file(interpreter.lookup(file_name)?)?;\n            let result = file_read_remaining(&mut file)\n                .and_then(|bytes| {\n                    String::from_utf8(bytes).map_err(|error| {\n                        std::io::Error::new(std::io::ErrorKind::InvalidData, error)\n                    })\n                })\n                .map(Value::String)\n                .map_err(file_error);\n            interpreter.assign(file_name, file.to_value())?;\n            Ok(result_value(result))\n        }"
        }
        InterpreterEvalKind::FileReadAllStringAsync => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map(Value::String)\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::FileReadInto => {
            "{\n            let file_name = interpreter.mut_arg_local_name(args, \"file\", 0)?;\n            let buffer_name = interpreter.mut_arg_local_name(args, \"buffer\", 1)?;\n            let mut file = expect_file(interpreter.lookup(file_name)?)?;\n            let result = file_read_remaining(&mut file)\n                .map(|bytes| {\n                    let did_read = !bytes.is_empty();\n                    (Value::Bytes(bytes), Value::Bool(did_read))\n                })\n                .map_err(file_error);\n            interpreter.assign(file_name, file.to_value())?;\n            Ok(match result {\n                Ok((buffer, did_read)) => {\n                    interpreter.assign(buffer_name, buffer)?;\n                    value_ok(did_read)\n                }\n                Err(error) => value_err(error),\n            })\n        }"
        }
        InterpreterEvalKind::FileReadString => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map(Value::String)\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::FileRemove => {
            "{\n            let path = interpreter.eval_first_arg(args)?;\n            Ok(file_result_unit(std::fs::remove_file(expect_string(path)?)))\n        }"
        }
        InterpreterEvalKind::FileWrite
        | InterpreterEvalKind::FileWriteBytesView
        | InterpreterEvalKind::FileWriteBuffer
        | InterpreterEvalKind::FileWriteBufferView => {
            "{\n            let file_name = interpreter.mut_arg_local_name(args, \"file\", 0)?;\n            let data = interpreter\n                .eval_named_or_positional_arg(args, \"data\", 1)\n                .or_else(|_| interpreter.eval_named_or_positional_arg(args, \"buffer\", 1))?;\n            let mut file = expect_file(interpreter.lookup(file_name)?)?;\n            let result = file_write_at_cursor(&mut file, &expect_bytes(data)?);\n            interpreter.assign(file_name, file.to_value())?;\n            Ok(file_result_unit(result))\n        }"
        }
        InterpreterEvalKind::FileWriteAsync | InterpreterEvalKind::FileWriteBytes => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let data = interpreter.eval_named_or_positional_arg(args, \"data\", 1)?;\n            Ok(file_result_unit(std::fs::write(\n                expect_string(path)?,\n                expect_bytes(data)?,\n            )))\n        }"
        }
        InterpreterEvalKind::FileWriteAtomic => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 1)?;\n            Ok(file_atomic_write_result(\n                PathBuf::from(expect_string(path)?),\n                &expect_string(text)?,\n            ))\n        }"
        }
        InterpreterEvalKind::FileWriteString => {
            "{\n            let file_name = interpreter.mut_arg_local_name(args, \"file\", 0)?;\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 1)?;\n            let mut file = expect_file(interpreter.lookup(file_name)?)?;\n            let result = file_write_at_cursor(&mut file, expect_string(text)?.as_bytes());\n            interpreter.assign(file_name, file.to_value())?;\n            Ok(file_result_unit(result))\n        }"
        }
        InterpreterEvalKind::FileWriteStringAsync => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 1)?;\n            Ok(file_result_unit(std::fs::write(\n                expect_string(path)?,\n                expect_string(text)?,\n            )))\n        }"
        }
        InterpreterEvalKind::FileWriteStringToPath => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 1)?;\n            Ok(file_result_unit(std::fs::write(\n                expect_string(path)?,\n                expect_string(text)?,\n            )))\n        }"
        }
        InterpreterEvalKind::FunctionObjectHasClosure => {
            "{\n            let function = interpreter.eval_named_or_positional_arg(args, \"function\", 0)?;\n            Ok(Value::Bool(expect_function_has_closure(function)?))\n        }"
        }
        InterpreterEvalKind::FunctionObjectNew => {
            "{\n            let closure = interpreter.eval_named_or_positional_arg(args, \"closure\", 0)?;\n            let _ = expect_environment_state(closure)?;\n            Ok(function_object_value(true))\n        }"
        }
        InterpreterEvalKind::GlobalConfigNew => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            Ok(global_config_value(expect_config_rule_count(value)?))\n        }"
        }
        InterpreterEvalKind::GlobalConfigReplace => {
            "{\n            let global_name = interpreter.mut_arg_local_name(args, \"global\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            interpreter.assign(\n                global_name,\n                global_config_value(expect_config_rule_count(value)?),\n            )?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::GlobalConfigRuleCount => {
            "{\n            let global = interpreter.eval_named_or_positional_arg(args, \"global\", 0)?;\n            Ok(Value::Int(expect_global_config_rule_count(global)?))\n        }"
        }
        InterpreterEvalKind::HashSha256Bytes => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(sha256_digest(&expect_bytes(value)?)))\n        }"
        }
        InterpreterEvalKind::HashSha256File => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            Ok(result_value(\n                std::fs::read(expect_string(path)?)\n                    .map(|bytes| Value::String(sha256_digest(&bytes)))\n                    .map_err(file_error),\n            ))\n        }"
        }
        InterpreterEvalKind::HashSha256String => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(sha256_digest(\n                expect_string(value)?.as_bytes(),\n            )))\n        }"
        }
        InterpreterEvalKind::HexDecode => {
            "{\n            let text = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                hex::decode(expect_string(text)?)\n                    .map(Value::Bytes)\n                    .map_err(decode_error),\n            ))\n        }"
        }
        InterpreterEvalKind::HexEncode => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(hex::encode(expect_bytes(value)?)))\n        }"
        }
        InterpreterEvalKind::HexEncodeString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(hex::encode(expect_string(value)?.as_bytes())))\n        }"
        }
        InterpreterEvalKind::InstantElapsed => {
            "{\n            let start = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(\n                clock_system_unix_ms()\n                    .saturating_sub(expect_instant_unix_ms(start)?)\n                    .max(0),\n            ))\n        }"
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
        InterpreterEvalKind::UrlDecodeComponent => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                percent_encoding::percent_decode_str(&expect_string(value)?)\n                    .decode_utf8()\n                    .map(|value| Value::String(value.to_string()))\n                    .map_err(decode_error),\n            ))\n        }"
        }
        InterpreterEvalKind::UrlEncodeComponent => {
            "{\n            const COMPONENT_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC\n                .remove(b'-')\n                .remove(b'_')\n                .remove(b'.')\n                .remove(b'~');\n            let value = interpreter.eval_first_arg(args)?;\n            let value = expect_string(value)?;\n            Ok(Value::String(\n                percent_encoding::utf8_percent_encode(&value, COMPONENT_SET).to_string(),\n            ))\n        }"
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
        InterpreterEvalKind::ListCountWhere => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            let mut count = 0_i64;\n            for value in expect_list(list)? {\n                if expect_bool(interpreter.call_closure(predicate.clone(), vec![value])?)? {\n                    count += 1;\n                }\n            }\n            Ok(Value::Int(count))\n        }"
        }
        InterpreterEvalKind::ListAny | InterpreterEvalKind::ListContains => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            for value in expect_list(list)? {\n                if expect_bool(interpreter.call_closure(predicate.clone(), vec![value])?)? {\n                    return Ok(Value::Bool(true));\n                }\n            }\n            Ok(Value::Bool(false))\n        }"
        }
        InterpreterEvalKind::ListAll => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            for value in expect_list(list)? {\n                if !expect_bool(interpreter.call_closure(predicate.clone(), vec![value])?)? {\n                    return Ok(Value::Bool(false));\n                }\n            }\n            Ok(Value::Bool(true))\n        }"
        }
        InterpreterEvalKind::ListFind => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            for value in expect_list(list)? {\n                if expect_bool(interpreter.call_closure(predicate.clone(), vec![value.clone()])?)? {\n                    return Ok(value_some(value));\n                }\n            }\n            Ok(value_none())\n        }"
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
        InterpreterEvalKind::ListFilter => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            let mut filtered = Vec::new();\n            for value in expect_list(list)? {\n                if expect_bool(interpreter.call_closure(predicate.clone(), vec![value.clone()])?)? {\n                    filtered.push(value);\n                }\n            }\n            Ok(Value::List(filtered))\n        }"
        }
        InterpreterEvalKind::ListMap => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            let mut mapped = Vec::new();\n            for value in expect_list(list)? {\n                mapped.push(interpreter.call_closure(mapper.clone(), vec![value])?);\n            }\n            Ok(Value::List(mapped))\n        }"
        }
        InterpreterEvalKind::ListFold => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let mut state = interpreter.eval_named_or_positional_arg(args, \"initial\", 1)?;\n            let folder = interpreter.eval_named_or_positional_arg(args, \"folder\", 2)?;\n            for value in expect_list(list)? {\n                state = interpreter.call_closure(folder.clone(), vec![state, value])?;\n            }\n            Ok(state)\n        }"
        }
        InterpreterEvalKind::ListTryFold => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let mut state = interpreter.eval_named_or_positional_arg(args, \"initial\", 1)?;\n            let folder = interpreter.eval_named_or_positional_arg(args, \"folder\", 2)?;\n            for value in expect_list(list)? {\n                match result_payload(interpreter.call_closure(folder.clone(), vec![state, value])?)? {\n                    Ok(next) => state = next,\n                    Err(error) => return Ok(value_err(error)),\n                }\n            }\n            Ok(value_ok(state))\n        }"
        }
        InterpreterEvalKind::ListGroupBy => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let mut groups: Vec<(Value, Value)> = Vec::new();\n            for value in expect_list(list)? {\n                let group_key = interpreter.call_closure(key.clone(), vec![value.clone()])?;\n                if let Some((_, group)) = groups\n                    .iter_mut()\n                    .find(|(entry_key, _)| entry_key == &group_key)\n                {\n                    expect_list_mut(group)?.push(value);\n                } else {\n                    groups.push((group_key, Value::List(vec![value])));\n                }\n            }\n            Ok(Value::Map(groups))\n        }"
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
        InterpreterEvalKind::ListNew => "{\n            Ok(Value::List(Vec::new()))\n        }",
        InterpreterEvalKind::ListPipeline | InterpreterEvalKind::PipelineCollect => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::ListFlatMap => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            let mut flattened = Vec::new();\n            for value in expect_list(list)? {\n                flattened.extend(expect_list(\n                    interpreter.call_closure(mapper.clone(), vec![value])?,\n                )?);\n            }\n            Ok(Value::List(flattened))\n        }"
        }
        InterpreterEvalKind::ListPartition => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            let mut matched = Vec::new();\n            let mut unmatched = Vec::new();\n            for value in expect_list(list)? {\n                if expect_bool(interpreter.call_closure(predicate.clone(), vec![value.clone()])?)? {\n                    matched.push(value);\n                } else {\n                    unmatched.push(value);\n                }\n            }\n            Ok(Value::List(vec![\n                Value::List(matched),\n                Value::List(unmatched),\n            ]))\n        }"
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
        InterpreterEvalKind::ListSortWith => {
            "{\n            let list_name = interpreter.mut_arg_local_name(args, \"list\", 0)?;\n            let compare = interpreter.eval_named_or_positional_arg(args, \"compare\", 1)?;\n            let mut list = expect_list(interpreter.lookup(list_name)?)?;\n            interpreter.sort_list_with_closure(&mut list, compare)?;\n            interpreter.assign(list_name, Value::List(list))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ListSortBy => {
            "{\n            let list = interpreter.eval_named_or_positional_arg(args, \"list\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let compare = interpreter.eval_named_or_positional_arg(args, \"compare\", 2)?;\n            let mut list = expect_list(list)?;\n            interpreter.sort_list_by_closure(&mut list, key, compare)?;\n            Ok(Value::List(list))\n        }"
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
        InterpreterEvalKind::MapFilter => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            let mut filtered = Vec::new();\n            for (key, value) in expect_map(map)? {\n                if expect_bool(\n                    interpreter.call_closure(predicate.clone(), vec![key.clone(), value.clone()])?,\n                )? {\n                    filtered.push((key, value));\n                }\n            }\n            Ok(Value::Map(filtered))\n        }"
        }
        InterpreterEvalKind::MapFold => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let mut state = interpreter.eval_named_or_positional_arg(args, \"initial\", 1)?;\n            let folder = interpreter.eval_named_or_positional_arg(args, \"folder\", 2)?;\n            for (key, value) in expect_map(map)? {\n                state = interpreter.call_closure(folder.clone(), vec![state, key, value])?;\n            }\n            Ok(state)\n        }"
        }
        InterpreterEvalKind::MapTryFold => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let mut state = interpreter.eval_named_or_positional_arg(args, \"initial\", 1)?;\n            let folder = interpreter.eval_named_or_positional_arg(args, \"folder\", 2)?;\n            for (key, value) in expect_map(map)? {\n                match result_payload(\n                    interpreter.call_closure(folder.clone(), vec![state, key, value])?,\n                )? {\n                    Ok(next) => state = next,\n                    Err(error) => return Ok(value_err(error)),\n                }\n            }\n            Ok(value_ok(state))\n        }"
        }
        InterpreterEvalKind::MapForEach => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let callback = interpreter.eval_named_or_positional_arg(args, \"callback\", 1)?;\n            for (key, value) in expect_map(map)? {\n                let _ = interpreter.call_closure(callback.clone(), vec![key, value])?;\n            }\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::MapMapValues => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            let mut mapped = Vec::new();\n            for (key, value) in expect_map(map)? {\n                mapped.push((key, interpreter.call_closure(mapper.clone(), vec![value])?));\n            }\n            Ok(Value::Map(mapped))\n        }"
        }
        InterpreterEvalKind::MapMerge => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let resolver = interpreter.eval_named_or_positional_arg(args, \"resolver\", 2)?;\n            let mut merged = expect_map(left)?;\n            for (key, right_value) in expect_map(right)? {\n                if let Some((_, left_value)) =\n                    merged.iter_mut().find(|(entry_key, _)| entry_key == &key)\n                {\n                    *left_value = interpreter.call_closure(\n                        resolver.clone(),\n                        vec![left_value.clone(), right_value],\n                    )?;\n                } else {\n                    merged.push((key, right_value));\n                }\n            }\n            Ok(Value::Map(merged))\n        }"
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
        InterpreterEvalKind::MapNew => "{\n            Ok(Value::Map(Vec::new()))\n        }",
        InterpreterEvalKind::MapRemove => {
            "{\n            let map_name = interpreter.mut_arg_local_name(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let mut map = expect_map(interpreter.lookup(map_name)?)?;\n            let old = map_remove(&mut map, &key);\n            interpreter.assign(map_name, Value::Map(map))?;\n            Ok(value_option(old, |value| value))\n        }"
        }
        InterpreterEvalKind::MapValues => {
            "{\n            let map = interpreter.eval_first_arg(args)?;\n            Ok(Value::List(\n                expect_map(map)?\n                    .into_iter()\n                    .map(|(_, value)| value)\n                    .collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::HttpErrorMessage => {
            "{\n            let error = interpreter.eval_named_or_positional_arg(args, \"error\", 0)?;\n            read_field(&error, \"message\")\n        }"
        }
        InterpreterEvalKind::HttpGet => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP client runtime is not configured for GET {}\",\n                expect_string(url)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpGetAsync => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let url = expect_string(url)?;\n            Ok(result_value(interpreter.http_get_local(&url).map_err(http_error)))\n        }"
        }
        InterpreterEvalKind::HttpGetRetryAsync => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let timeout = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 1)?;\n            let attempts = interpreter.eval_named_or_positional_arg(args, \"attempts\", 2)?;\n            let backoff = interpreter.eval_named_or_positional_arg(args, \"backoff_ms\", 3)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP async provider is not configured for GET {} with timeout {}ms attempts {} backoff {}ms\",\n                expect_string(url)?,\n                expect_int(timeout)?,\n                expect_int(attempts)?,\n                expect_int(backoff)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpGetTimeoutAsync => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let timeout = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 1)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP async provider is not configured for GET {} with timeout {}ms\",\n                expect_string(url)?,\n                expect_int(timeout)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpPostForm => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let _body = interpreter.eval_named_or_positional_arg(args, \"body\", 1)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP client runtime is not configured for POST form {}\",\n                expect_string(url)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpPostFormAsync => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let _body = interpreter.eval_named_or_positional_arg(args, \"body\", 1)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP async provider is not configured for POST form {}\",\n                expect_string(url)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpPostJson => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let _body = interpreter.eval_named_or_positional_arg(args, \"body\", 1)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP client runtime is not configured for POST JSON {}\",\n                expect_string(url)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpPostJsonAsync => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let _body = interpreter.eval_named_or_positional_arg(args, \"body\", 1)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP async provider is not configured for POST JSON {}\",\n                expect_string(url)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpPostJsonBearerRetryAsync => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let _body = interpreter.eval_named_or_positional_arg(args, \"body\", 1)?;\n            let _token = interpreter.eval_named_or_positional_arg(args, \"token\", 2)?;\n            let timeout = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 3)?;\n            let attempts = interpreter.eval_named_or_positional_arg(args, \"attempts\", 4)?;\n            let backoff = interpreter.eval_named_or_positional_arg(args, \"backoff_ms\", 5)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP async provider is not configured for bearer POST JSON {} with timeout {}ms attempts {} backoff {}ms\",\n                expect_string(url)?,\n                expect_int(timeout)?,\n                expect_int(attempts)?,\n                expect_int(backoff)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpPostJsonRetryAsync => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let _body = interpreter.eval_named_or_positional_arg(args, \"body\", 1)?;\n            let timeout = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 2)?;\n            let attempts = interpreter.eval_named_or_positional_arg(args, \"attempts\", 3)?;\n            let backoff = interpreter.eval_named_or_positional_arg(args, \"backoff_ms\", 4)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP async provider is not configured for POST JSON {} with timeout {}ms attempts {} backoff {}ms\",\n                expect_string(url)?,\n                expect_int(timeout)?,\n                expect_int(attempts)?,\n                expect_int(backoff)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpPostJsonTimeoutAsync => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let _body = interpreter.eval_named_or_positional_arg(args, \"body\", 1)?;\n            let timeout = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 2)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP async provider is not configured for POST JSON {} with timeout {}ms\",\n                expect_string(url)?,\n                expect_int(timeout)?\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpSendAsync => {
            "{\n            let request = interpreter.eval_named_or_positional_arg(args, \"request\", 0)?;\n            let request = expect_http_request(request)?;\n            Ok(value_err(http_error(format!(\n                \"HTTP async provider is not configured for {} {}\",\n                request.method, request.url\n            ))))\n        }"
        }
        InterpreterEvalKind::HttpRequestJson => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let body = interpreter.eval_named_or_positional_arg(args, \"body\", 1)?;\n            Ok(http_request_value(\n                \"POST\",\n                expect_string(url)?,\n                expect_string(body)?,\n                0,\n                1,\n                0,\n                0,\n            ))\n        }"
        }
        InterpreterEvalKind::HttpRequestWithHeader => {
            "{\n            let request = interpreter.eval_named_or_positional_arg(args, \"request\", 0)?;\n            let _name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            let _value = interpreter.eval_named_or_positional_arg(args, \"value\", 2)?;\n            let mut request = expect_http_request(request)?;\n            request.header_count += 1;\n            Ok(request.to_value())\n        }"
        }
        InterpreterEvalKind::HttpRequestWithRetry => {
            "{\n            let request = interpreter.eval_named_or_positional_arg(args, \"request\", 0)?;\n            let attempts = interpreter.eval_named_or_positional_arg(args, \"attempts\", 1)?;\n            let backoff_ms = interpreter.eval_named_or_positional_arg(args, \"backoff_ms\", 2)?;\n            let mut request = expect_http_request(request)?;\n            request.attempts = expect_int(attempts)?;\n            request.backoff_ms = expect_int(backoff_ms)?;\n            Ok(request.to_value())\n        }"
        }
        InterpreterEvalKind::HttpRequestWithTimeout => {
            "{\n            let request = interpreter.eval_named_or_positional_arg(args, \"request\", 0)?;\n            let timeout_ms = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 1)?;\n            let mut request = expect_http_request(request)?;\n            request.timeout_ms = expect_int(timeout_ms)?;\n            Ok(request.to_value())\n        }"
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
        InterpreterEvalKind::ImageInspect => {
            "{\n            let image = interpreter.eval_named_or_positional_arg(args, \"image\", 0)?;\n            let image = expect_image(image)?;\n            interpreter.stdout.push_str(&image.inspect_line());\n            interpreter.stdout.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ImageLoad => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            Ok(result_value(\n                std::fs::read(expect_string(path)?)\n                    .map(|bytes| image_value(bytes, None, None, vec![\"load\".to_string()]))\n                    .map_err(image_error_from_io),\n            ))\n        }"
        }
        InterpreterEvalKind::ImageNormalize => {
            "{\n            let image_name = interpreter.mut_arg_local_name(args, \"image\", 0)?;\n            let mut image = expect_image(interpreter.lookup(image_name)?)?;\n            image.operations.push(\"normalize\".to_string());\n            interpreter.assign(image_name, image.to_value())?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ImageResize => {
            "{\n            let image_name = interpreter.mut_arg_local_name(args, \"image\", 0)?;\n            let width = interpreter.eval_named_or_positional_arg(args, \"width\", 1)?;\n            let height = interpreter.eval_named_or_positional_arg(args, \"height\", 2)?;\n            let mut image = expect_image(interpreter.lookup(image_name)?)?;\n            image.width = Some(expect_int(width)?);\n            image.height = Some(expect_int(height)?);\n            image.operations.push(\"resize\".to_string());\n            interpreter.assign(image_name, image.to_value())?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ImageSave => {
            "{\n            let image = interpreter.eval_named_or_positional_arg(args, \"image\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let image = expect_image(image)?;\n            Ok(result_value(\n                std::fs::write(expect_string(path)?, image.saved_bytes())\n                    .map(|_| Value::Unit)\n                    .map_err(image_error_from_io),\n            ))\n        }"
        }
        InterpreterEvalKind::ImageSharpen => {
            "{\n            let image_name = interpreter.mut_arg_local_name(args, \"image\", 0)?;\n            let mut image = expect_image(interpreter.lookup(image_name)?)?;\n            image.operations.push(\"sharpen\".to_string());\n            interpreter.assign(image_name, image.to_value())?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::JsonArray => {
            "{\n            let items = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(format!(\n                \"[{}]\",\n                expect_string_list(items)?.join(\",\")\n            )))\n        }"
        }
        InterpreterEvalKind::JsonArrayBools => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                json_array_bools(expect_json(value)?).map(Value::List),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonArrayContainsPrefix => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let prefix = interpreter.eval_named_or_positional_arg(args, \"prefix\", 1)?;\n            Ok(result_value(json_array_contains_string(\n                expect_json(value)?,\n                &expect_string(prefix)?,\n                JsonArrayStringMatch::Prefix,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonArrayContainsString => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let item = interpreter.eval_named_or_positional_arg(args, \"item\", 1)?;\n            Ok(result_value(json_array_contains_string(\n                expect_json(value)?,\n                &expect_string(item)?,\n                JsonArrayStringMatch::Exact,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonArrayContainsSubstring => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 1)?;\n            Ok(result_value(json_array_contains_string(\n                expect_json(value)?,\n                &expect_string(text)?,\n                JsonArrayStringMatch::Substring,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonArrayCountWhere => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            let json = expect_json(value)?;\n            let items = match json_array(&json) {\n                Ok(items) => items,\n                Err(error) => return Ok(value_err(error)),\n            };\n            let mut count = 0_i64;\n            for item in items {\n                match result_payload(\n                    interpreter.call_closure(predicate.clone(), vec![Value::Json(item.clone())])?,\n                )? {\n                    Ok(value) => {\n                        if expect_bool(value)? {\n                            count += 1;\n                        }\n                    }\n                    Err(error) => return Ok(value_err(error)),\n                }\n            }\n            Ok(value_ok(Value::Int(count)))\n        }"
        }
        InterpreterEvalKind::JsonArrayFold => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let mut state = interpreter.eval_named_or_positional_arg(args, \"initial\", 1)?;\n            let folder = interpreter.eval_named_or_positional_arg(args, \"folder\", 2)?;\n            let json = expect_json(value)?;\n            let items = match json_array(&json) {\n                Ok(items) => items,\n                Err(error) => return Ok(value_err(error)),\n            };\n            for item in items {\n                match result_payload(\n                    interpreter.call_closure(folder.clone(), vec![state, Value::Json(item.clone())])?,\n                )? {\n                    Ok(next) => state = next,\n                    Err(error) => return Ok(value_err(error)),\n                }\n            }\n            Ok(value_ok(state))\n        }"
        }
        InterpreterEvalKind::JsonArrayGet => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let index = interpreter.eval_named_or_positional_arg(args, \"index\", 1)?;\n            Ok(result_value(json_array_get_result(\n                expect_json(value)?,\n                expect_int(index)?,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonArrayInts => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                json_array_ints(expect_json(value)?).map(Value::List),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonArrayLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            let json = expect_json(value)?;\n            Ok(result_value(\n                json_array(&json).map(|items| Value::Int(items.len() as i64)),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonArrayStrings => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                json_array_strings(expect_json(value)?).map(Value::List),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonAsBool => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(json_as_bool_value(expect_json(value)?)))\n        }"
        }
        InterpreterEvalKind::JsonAsInt => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(json_as_int_value(expect_json(value)?)))\n        }"
        }
        InterpreterEvalKind::JsonAsString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(json_as_string_value(expect_json(value)?)))\n        }"
        }
        InterpreterEvalKind::JsonAt => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(json_value_at(\n                expect_json(value)?,\n                &expect_string(path)?,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonAtBool => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(\n                json_value_at(expect_json(value)?, &expect_string(path)?)\n                    .and_then(value_json)\n                    .and_then(json_as_bool_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonAtBoolOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            let found = json_value_at(expect_json(value)?, &expect_string(path)?)\n                .and_then(value_json)\n                .and_then(json_as_bool_value);\n            Ok(found.unwrap_or(Value::Bool(expect_bool(fallback)?)))\n        }"
        }
        InterpreterEvalKind::JsonAtInt => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(\n                json_value_at(expect_json(value)?, &expect_string(path)?)\n                    .and_then(value_json)\n                    .and_then(json_as_int_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonAtIntOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            let found = json_value_at(expect_json(value)?, &expect_string(path)?)\n                .and_then(value_json)\n                .and_then(json_as_int_value);\n            Ok(found.unwrap_or(Value::Int(expect_int(fallback)?)))\n        }"
        }
        InterpreterEvalKind::JsonAtOptional => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(Ok(json_optional_path(\n                expect_json(value)?,\n                &expect_string(path)?,\n                Value::Json,\n            ))))\n        }"
        }
        InterpreterEvalKind::JsonAtOptionalBool => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(json_optional_typed_path(\n                expect_json(value)?,\n                &expect_string(path)?,\n                json_as_bool_value,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonAtOptionalInt => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(json_optional_typed_path(\n                expect_json(value)?,\n                &expect_string(path)?,\n                json_as_int_value,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonAtOptionalString => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(json_optional_typed_path(\n                expect_json(value)?,\n                &expect_string(path)?,\n                json_as_string_value,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonAtOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            Ok(json_value_at(expect_json(value)?, &expect_string(path)?)\n                .unwrap_or(Value::Json(expect_json(fallback)?)))\n        }"
        }
        InterpreterEvalKind::JsonAtString => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(\n                json_value_at(expect_json(value)?, &expect_string(path)?)\n                    .and_then(value_json)\n                    .and_then(json_as_string_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonAtStringOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            let fallback = expect_string(fallback)?;\n            let found = json_value_at(expect_json(value)?, &expect_string(path)?)\n                .and_then(value_json)\n                .and_then(json_as_string_value);\n            Ok(found.unwrap_or(Value::String(fallback)))\n        }"
        }
        InterpreterEvalKind::JsonAtToString => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            Ok(result_value(\n                json_value_at(expect_json(value)?, &expect_string(path)?)\n                    .and_then(value_json)\n                    .map(|json| Value::String(json.to_string())),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonAtToStringOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            let fallback = expect_string(fallback)?;\n            let found = json_value_at(expect_json(value)?, &expect_string(path)?)\n                .and_then(value_json)\n                .map(|json| Value::String(json.to_string()));\n            Ok(found.unwrap_or(Value::String(fallback)))\n        }"
        }
        InterpreterEvalKind::JsonBoolField => {
            "{\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            Ok(Value::String(format!(\n                \"{}:{}\",\n                json_quote_string(&expect_string(name)?)?,\n                expect_bool(value)?\n            )))\n        }"
        }
        InterpreterEvalKind::JsonBoolAt => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let text = expect_string(text)?;\n            let path = expect_string(path)?;\n            Ok(result_value(\n                parse_json_value(&text)\n                    .and_then(|json| json_value_at(json, &path))\n                    .and_then(value_json)\n                    .and_then(json_as_bool_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonBoolAtOr => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            let text = expect_string(text)?;\n            let path = expect_string(path)?;\n            let found = parse_json_value(&text)\n                .and_then(|json| json_value_at(json, &path))\n                .and_then(value_json)\n                .and_then(json_as_bool_value);\n            Ok(found.unwrap_or(Value::Bool(expect_bool(fallback)?)))\n        }"
        }
        InterpreterEvalKind::JsonClone => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::JsonField => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            Ok(result_value(json_field(\n                expect_json(value)?,\n                &expect_string(name)?,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonFieldBool => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            Ok(result_value(\n                json_field(expect_json(value)?, &expect_string(name)?)\n                    .and_then(value_json)\n                    .and_then(json_as_bool_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonFieldInt => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            Ok(result_value(\n                json_field(expect_json(value)?, &expect_string(name)?)\n                    .and_then(value_json)\n                    .and_then(json_as_int_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonFieldOptional => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            Ok(result_value(Ok(json_optional_field(\n                expect_json(value)?,\n                &expect_string(name)?,\n                Value::Json,\n            ))))\n        }"
        }
        InterpreterEvalKind::JsonFieldOptionalBool => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            Ok(result_value(json_optional_typed_field(\n                expect_json(value)?,\n                &expect_string(name)?,\n                json_as_bool_value,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonFieldOptionalInt => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            Ok(result_value(json_optional_typed_field(\n                expect_json(value)?,\n                &expect_string(name)?,\n                json_as_int_value,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonFieldOptionalString => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            Ok(result_value(json_optional_typed_field(\n                expect_json(value)?,\n                &expect_string(name)?,\n                json_as_string_value,\n            )))\n        }"
        }
        InterpreterEvalKind::JsonFieldString => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 1)?;\n            Ok(result_value(\n                json_field(expect_json(value)?, &expect_string(name)?)\n                    .and_then(value_json)\n                    .and_then(json_as_string_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonIntField => {
            "{\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            Ok(Value::String(format!(\n                \"{}:{}\",\n                json_quote_string(&expect_string(name)?)?,\n                expect_int(value)?\n            )))\n        }"
        }
        InterpreterEvalKind::JsonIntAt => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let text = expect_string(text)?;\n            let path = expect_string(path)?;\n            Ok(result_value(\n                parse_json_value(&text)\n                    .and_then(|json| json_value_at(json, &path))\n                    .and_then(value_json)\n                    .and_then(json_as_int_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonIntAtOr => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            let text = expect_string(text)?;\n            let path = expect_string(path)?;\n            let found = parse_json_value(&text)\n                .and_then(|json| json_value_at(json, &path))\n                .and_then(value_json)\n                .and_then(json_as_int_value);\n            Ok(found.unwrap_or(Value::Int(expect_int(fallback)?)))\n        }"
        }
        InterpreterEvalKind::JsonIsArray => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_json(value)?.is_array()))\n        }"
        }
        InterpreterEvalKind::JsonIsNull => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_json(value)?.is_null()))\n        }"
        }
        InterpreterEvalKind::JsonIsObject => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_json(value)?.is_object()))\n        }"
        }
        InterpreterEvalKind::JsonKind => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(json_kind(&expect_json(value)?).to_string()))\n        }"
        }
        InterpreterEvalKind::JsonObject => {
            "{\n            let fields = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(format!(\n                \"{{{}}}\",\n                expect_string_list(fields)?.join(\",\")\n            )))\n        }"
        }
        InterpreterEvalKind::JsonObjectKeys => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            let json = expect_json(value)?;\n            Ok(result_value(json_object(&json).map(|fields| {\n                let mut keys = fields\n                    .keys()\n                    .cloned()\n                    .map(Value::String)\n                    .collect::<Vec<_>>();\n                keys.sort_by_key(Value::display);\n                Value::List(keys)\n            })))\n        }"
        }
        InterpreterEvalKind::JsonObjectLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            let json = expect_json(value)?;\n            Ok(result_value(\n                json_object(&json).map(|fields| Value::Int(fields.len() as i64)),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonParse => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(result_value(\n                serde_json::from_str(&expect_string(value)?)\n                    .map(Value::Json)\n                    .map_err(|error| json_error(error.to_string())),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonParseFile => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            Ok(json_result(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map_err(|error| error.to_string())\n                    .and_then(|text| {\n                        serde_json::from_str(&text).map_err(|error| error.to_string())\n                    }),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonQuoteString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            serde_json::to_string(&expect_string(value)?)\n                .map(Value::String)\n                .map_err(|error| EvalError::Runtime(error.to_string()))\n        }"
        }
        InterpreterEvalKind::JsonRawField => {
            "{\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            Ok(Value::String(format!(\n                \"{}:{}\",\n                json_quote_string(&expect_string(name)?)?,\n                expect_string(value)?\n            )))\n        }"
        }
        InterpreterEvalKind::JsonStringArray => {
            "{\n            let items = interpreter.eval_first_arg(args)?;\n            let quoted = expect_string_list(items)?\n                .into_iter()\n                .map(|item| json_quote_string(&item))\n                .collect::<Result<Vec<_>, _>>()?;\n            Ok(Value::String(format!(\"[{}]\", quoted.join(\",\"))))\n        }"
        }
        InterpreterEvalKind::JsonStringAt => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let text = expect_string(text)?;\n            let path = expect_string(path)?;\n            Ok(result_value(\n                parse_json_value(&text)\n                    .and_then(|json| json_value_at(json, &path))\n                    .and_then(value_json)\n                    .and_then(json_as_string_value),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonStringAtOr => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            let text = expect_string(text)?;\n            let path = expect_string(path)?;\n            let fallback = expect_string(fallback)?;\n            let found = parse_json_value(&text)\n                .and_then(|json| json_value_at(json, &path))\n                .and_then(value_json)\n                .and_then(json_as_string_value);\n            Ok(found.unwrap_or(Value::String(fallback)))\n        }"
        }
        InterpreterEvalKind::JsonStringField => {
            "{\n            let name = interpreter.eval_named_or_positional_arg(args, \"name\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            Ok(Value::String(format!(\n                \"{}:{}\",\n                json_quote_string(&expect_string(name)?)?,\n                json_quote_string(&expect_string(value)?)?\n            )))\n        }"
        }
        InterpreterEvalKind::JsonStrings => {
            "{\n            let items = interpreter.eval_first_arg(args)?;\n            Ok(Value::Json(serde_json::Value::Array(\n                expect_string_list(items)?\n                    .into_iter()\n                    .map(serde_json::Value::String)\n                    .collect(),\n            )))\n        }"
        }
        InterpreterEvalKind::JsonToString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_json(value)?.to_string()))\n        }"
        }
        InterpreterEvalKind::JsonToStringAt => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let text = expect_string(text)?;\n            let path = expect_string(path)?;\n            Ok(result_value(\n                parse_json_value(&text)\n                    .and_then(|json| json_value_at(json, &path))\n                    .and_then(value_json)\n                    .map(|json| Value::String(json.to_string())),\n            ))\n        }"
        }
        InterpreterEvalKind::JsonToStringAtOr => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 1)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 2)?;\n            let text = expect_string(text)?;\n            let path = expect_string(path)?;\n            let fallback = expect_string(fallback)?;\n            let found = parse_json_value(&text)\n                .and_then(|json| json_value_at(json, &path))\n                .and_then(value_json)\n                .map(|json| Value::String(json.to_string()));\n            Ok(found.unwrap_or(Value::String(fallback)))\n        }"
        }
        InterpreterEvalKind::JsonValue => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Json(value_to_json_literal(value)?))\n        }"
        }
        InterpreterEvalKind::JsonValues => {
            "{\n            let items = interpreter.eval_first_arg(args)?;\n            Ok(Value::Json(serde_json::Value::Array(expect_json_list(\n                items,\n            )?)))\n        }"
        }
        InterpreterEvalKind::JsonErrorMessage => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            read_field(&value, \"message\")\n        }"
        }
        InterpreterEvalKind::OptionUnwrapOrElse => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let default = interpreter.eval_named_or_positional_arg(args, \"default\", 1)?;\n            Ok(match option_payload(value)? {\n                Some(value) => value,\n                None => interpreter.call_closure(default, Vec::new())?,\n            })\n        }"
        }
        InterpreterEvalKind::OptionMap => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            Ok(match option_payload(value)? {\n                Some(value) => value_some(interpreter.call_closure(mapper, vec![value])?),\n                None => value_none(),\n            })\n        }"
        }
        InterpreterEvalKind::OptionAndThen => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            match option_payload(value)? {\n                Some(value) => {\n                    let mapped = interpreter.call_closure(mapper, vec![value])?;\n                    let _ = option_payload(mapped.clone())?;\n                    Ok(mapped)\n                }\n                None => Ok(value_none()),\n            }\n        }"
        }
        InterpreterEvalKind::OptionFilter => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            Ok(match option_payload(value)? {\n                Some(value) => {\n                    if expect_bool(interpreter.call_closure(predicate, vec![value.clone()])?)? {\n                        value_some(value)\n                    } else {\n                        value_none()\n                    }\n                }\n                None => value_none(),\n            })\n        }"
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
        InterpreterEvalKind::OrdCompare => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"self\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"other\", 1)?;\n            Ok(Value::Int(match value_cmp(&left, &right)? {\n                Ordering::Less => -1,\n                Ordering::Equal => 0,\n                Ordering::Greater => 1,\n            }))\n        }"
        }
        InterpreterEvalKind::OsClose => {
            "{\n            let fd = interpreter.eval_named_or_positional_arg(args, \"fd\", 0)?;\n            let _ = expect_int(fd)?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::PatchApplyText => {
            "{\n            let original = interpreter.eval_named_or_positional_arg(args, \"original\", 0)?;\n            let patch = interpreter.eval_named_or_positional_arg(args, \"patch\", 1)?;\n            Ok(result_value(\n                patch_apply_text_string(&expect_string(original)?, &expect_string(patch)?)\n                    .map(Value::String)\n                    .map_err(Value::String),\n            ))\n        }"
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
        InterpreterEvalKind::PipelineMap => {
            "{\n            let pipeline = interpreter.eval_named_or_positional_arg(args, \"pipeline\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            let mut mapped = Vec::new();\n            for value in expect_list(pipeline)? {\n                mapped.push(interpreter.call_closure(mapper.clone(), vec![value])?);\n            }\n            Ok(Value::List(mapped))\n        }"
        }
        InterpreterEvalKind::PipelineFilter => {
            "{\n            let pipeline = interpreter.eval_named_or_positional_arg(args, \"pipeline\", 0)?;\n            let predicate = interpreter.eval_named_or_positional_arg(args, \"predicate\", 1)?;\n            let mut filtered = Vec::new();\n            for value in expect_list(pipeline)? {\n                if expect_bool(interpreter.call_closure(predicate.clone(), vec![value.clone()])?)? {\n                    filtered.push(value);\n                }\n            }\n            Ok(Value::List(filtered))\n        }"
        }
        InterpreterEvalKind::PipelineEach => {
            "{\n            let pipeline = interpreter.eval_named_or_positional_arg(args, \"pipeline\", 0)?;\n            let action = interpreter.eval_named_or_positional_arg(args, \"action\", 1)?;\n            let items = expect_list(pipeline)?;\n            for value in items.iter().cloned() {\n                let _ = interpreter.call_closure(action.clone(), vec![value])?;\n            }\n            Ok(Value::List(items))\n        }"
        }
        InterpreterEvalKind::PipelineTryMap => {
            "{\n            let pipeline = interpreter.eval_named_or_positional_arg(args, \"pipeline\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            let mut mapped = Vec::new();\n            for value in expect_list(pipeline)? {\n                match result_payload(interpreter.call_closure(mapper.clone(), vec![value])?)? {\n                    Ok(value) => mapped.push(value),\n                    Err(error) => return Ok(value_err(error)),\n                }\n            }\n            Ok(value_ok(Value::List(mapped)))\n        }"
        }
        InterpreterEvalKind::ProcessRun | InterpreterEvalKind::ProcessRunAsync => {
            "{\n            let command = interpreter.eval_named_or_positional_arg(args, \"command\", 0)?;\n            let args_value = interpreter.eval_named_or_positional_arg(args, \"args\", 1)?;\n            let command = expect_string(command)?;\n            Ok(result_value(\n                process_run_output(&command, &expect_string_list(args_value)?)\n                    .map(process_output_value)\n                    .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ProcessRunStdout | InterpreterEvalKind::ProcessRunStdoutAsync => {
            "{\n            let command = interpreter.eval_named_or_positional_arg(args, \"command\", 0)?;\n            let args_value = interpreter.eval_named_or_positional_arg(args, \"args\", 1)?;\n            let command = expect_string(command)?;\n            Ok(result_value(\n                process_run_output(&command, &expect_string_list(args_value)?)\n                    .and_then(|output| process_stdout_result(&command, output))\n                    .map(Value::String)\n                    .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ProcessRunTimeout | InterpreterEvalKind::ProcessRunTimeoutAsync => {
            "{\n            let command = interpreter.eval_named_or_positional_arg(args, \"command\", 0)?;\n            let args_value = interpreter.eval_named_or_positional_arg(args, \"args\", 1)?;\n            let timeout = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 2)?;\n            let command = expect_string(command)?;\n            Ok(result_value(\n                process_run_timeout_output(\n                    &command,\n                    &expect_string_list(args_value)?,\n                    expect_int(timeout)?,\n                )\n                .map(process_output_value)\n                .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ProcessRunRequest | InterpreterEvalKind::ProcessRunRequestAsync => {
            "{\n            let request = interpreter.eval_named_or_positional_arg(args, \"request\", 0)?;\n            Ok(result_value(\n                process_run_request_output(&expect_process_request(request)?)\n                    .map(process_output_value)\n                    .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ProcessRunRequestCancellableAsync => {
            "{\n            let request = interpreter.eval_named_or_positional_arg(args, \"request\", 0)?;\n            let token = interpreter.eval_named_or_positional_arg(args, \"token\", 1)?;\n            let _ = expect_cancellation_id(token, \"CancellationToken\")?;\n            Ok(result_value(\n                process_run_request_output(&expect_process_request(request)?)\n                    .map(process_output_value)\n                    .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ProcessStream => {
            "{\n            let request = interpreter.eval_named_or_positional_arg(args, \"request\", 0)?;\n            Ok(result_value(\n                process_stream_value(&expect_process_request(request)?).map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ProcessRunStdoutTimeout
        | InterpreterEvalKind::ProcessRunStdoutTimeoutAsync => {
            "{\n            let command = interpreter.eval_named_or_positional_arg(args, \"command\", 0)?;\n            let args_value = interpreter.eval_named_or_positional_arg(args, \"args\", 1)?;\n            let timeout = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 2)?;\n            let command = expect_string(command)?;\n            Ok(result_value(\n                process_run_timeout_output(\n                    &command,\n                    &expect_string_list(args_value)?,\n                    expect_int(timeout)?,\n                )\n                .and_then(|output| process_stdout_result(&command, output))\n                .map(Value::String)\n                .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ProcessRunManyStdout
        | InterpreterEvalKind::ProcessRunManyStdoutAsync => {
            "{\n            let command = interpreter.eval_named_or_positional_arg(args, \"command\", 0)?;\n            let args_value = interpreter.eval_named_or_positional_arg(args, \"args\", 1)?;\n            let appended = interpreter.eval_named_or_positional_arg(args, \"appended_args\", 2)?;\n            let _jobs = interpreter.eval_named_or_positional_arg(args, \"jobs\", 3)?;\n            let command = expect_string(command)?;\n            Ok(result_value(\n                process_run_many_stdout(\n                    &command,\n                    &expect_string_list(args_value)?,\n                    &expect_string_list(appended)?,\n                    None,\n                )\n                .map(|items| Value::List(items.into_iter().map(Value::String).collect()))\n                .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ProcessRunManyStdoutTimeout
        | InterpreterEvalKind::ProcessRunManyStdoutTimeoutAsync => {
            "{\n            let command = interpreter.eval_named_or_positional_arg(args, \"command\", 0)?;\n            let args_value = interpreter.eval_named_or_positional_arg(args, \"args\", 1)?;\n            let appended = interpreter.eval_named_or_positional_arg(args, \"appended_args\", 2)?;\n            let _jobs = interpreter.eval_named_or_positional_arg(args, \"jobs\", 3)?;\n            let timeout = interpreter.eval_named_or_positional_arg(args, \"timeout_ms\", 4)?;\n            let command = expect_string(command)?;\n            Ok(result_value(\n                process_run_many_stdout(\n                    &command,\n                    &expect_string_list(args_value)?,\n                    &expect_string_list(appended)?,\n                    Some(expect_int(timeout)?),\n                )\n                .map(|items| Value::List(items.into_iter().map(Value::String).collect()))\n                .map_err(Value::String),\n            ))\n        }"
        }
        InterpreterEvalKind::ResourcePoolDiscard => {
            "{\n            let lease_name = interpreter.mut_arg_local_name(args, \"lease\", 0)?;\n            let lease = interpreter.lookup(lease_name)?;\n            interpreter.assign(lease_name, mark_pool_lease_discarded(lease)?)?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ResourcePoolStats => {
            "{\n            let pool_name = interpreter.mut_arg_local_name(args, \"pool\", 0)?;\n            let pool = expect_resource_pool(interpreter.lookup(pool_name)?)?;\n            let state = interpreter.pools.get(&pool.id).ok_or_else(|| {\n                EvalError::Runtime(format!(\"unknown ResourcePool id `{}`.\", pool.id))\n            })?;\n            Ok(pool_stats_value(\n                state.capacity,\n                state.created,\n                state.idle.len() as i64,\n                state.in_use,\n            ))\n        }"
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
        InterpreterEvalKind::ReceiverClose => {
            "{\n            let receiver_name = interpreter.mut_arg_local_name(args, \"receiver\", 0)?;\n            let receiver = expect_receiver(interpreter.lookup(receiver_name)?)?;\n            interpreter.channel_state_mut(receiver.channel_id)?.receiver_closed = true;\n            interpreter.assign(receiver_name, receiver_value(receiver.channel_id, true))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::ReceiverRecv => {
            "{\n            let receiver = interpreter.eval_named_or_positional_arg(args, \"receiver\", 0)?;\n            let receiver = expect_receiver(receiver)?;\n            if receiver.closed {\n                return Ok(value_err(channel_error(\"channel receiver closed\")));\n            }\n            Ok(result_value(interpreter.channel_recv(receiver.channel_id)))\n        }"
        }
        InterpreterEvalKind::ReceiverRecvCancellable => {
            "{\n            let receiver = interpreter.eval_named_or_positional_arg(args, \"receiver\", 0)?;\n            let receiver = expect_receiver(receiver)?;\n            if receiver.closed {\n                return Ok(value_err(channel_error(\"channel receiver closed\")));\n            }\n            let token = interpreter.eval_named_or_positional_arg(args, \"token\", 1)?;\n            let id = expect_cancellation_id(token, \"CancellationToken\")?;\n            if interpreter.cancellation_flags.get(&id).copied().unwrap_or(false) {\n                return Ok(value_err(channel_error(\"channel recv cancelled\")));\n            }\n            Ok(result_value(interpreter.channel_recv(receiver.channel_id)))\n        }"
        }
        InterpreterEvalKind::ReceiverIntoStream => {
            "{\n            let receiver = interpreter.eval_named_or_positional_arg(args, \"receiver\", 0)?;\n            let receiver = expect_receiver(receiver)?;\n            if receiver.closed {\n                return Ok(stream_collect_error_value(\"channel receiver closed\"));\n            }\n            Ok(stream_channel_value(receiver.channel_id))\n        }"
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
        InterpreterEvalKind::ResultErr => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(match result_payload(value)? {\n                Ok(_) => value_none(),\n                Err(error) => value_some(error),\n            })\n        }"
        }
        InterpreterEvalKind::ResultErrMessage => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(match result_payload(value)? {\n                Ok(_) => value_none(),\n                Err(error) => value_some(Value::String(error.display())),\n            })\n        }"
        }
        InterpreterEvalKind::ResultUnwrapOrElse => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"result\", 0)?;\n            let fallback = interpreter.eval_named_or_positional_arg(args, \"fallback\", 1)?;\n            Ok(match result_payload(value)? {\n                Ok(value) => value,\n                Err(error) => interpreter.call_closure(fallback, vec![error])?,\n            })\n        }"
        }
        InterpreterEvalKind::ResultMap => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"result\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            Ok(match result_payload(value)? {\n                Ok(value) => value_ok(interpreter.call_closure(mapper, vec![value])?),\n                Err(error) => value_err(error),\n            })\n        }"
        }
        InterpreterEvalKind::ResultMapError => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"result\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            Ok(match result_payload(value)? {\n                Ok(value) => value_ok(value),\n                Err(error) => value_err(interpreter.call_closure(mapper, vec![error])?),\n            })\n        }"
        }
        InterpreterEvalKind::ResultAndThen => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"result\", 0)?;\n            let mapper = interpreter.eval_named_or_positional_arg(args, \"mapper\", 1)?;\n            match result_payload(value)? {\n                Ok(value) => {\n                    let mapped = interpreter.call_closure(mapper, vec![value])?;\n                    let _ = result_payload(mapped.clone())?;\n                    Ok(mapped)\n                }\n                Err(error) => Ok(value_err(error)),\n            }\n        }"
        }
        InterpreterEvalKind::ResultIsErr => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(matches!(\n                value,\n                Value::Variant { name, .. } if name == \"Err\"\n            )))\n        }"
        }
        InterpreterEvalKind::ResultIsOk => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(matches!(\n                value,\n                Value::Variant { name, .. } if name == \"Ok\"\n            )))\n        }"
        }
        InterpreterEvalKind::ResultOk => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(match result_payload(value)? {\n                Ok(value) => value_some(value),\n                Err(_) => value_none(),\n            })\n        }"
        }
        InterpreterEvalKind::ResultUnwrapOr => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let default = interpreter.eval_named_or_positional_arg(args, \"default\", 1)?;\n            Ok(result_payload(value)?.map_or(default, |value| value))\n        }"
        }
        InterpreterEvalKind::RuleLoaderLoadRules => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            Ok(result_value(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map(|text| Value::List(rules_from_text(&text)))\n                    .map_err(config_error),\n            ))\n        }"
        }
        InterpreterEvalKind::SetClear => {
            "{\n            let set_name = interpreter.mut_arg_local_name(args, \"set\", 0)?;\n            interpreter.assign(set_name, Value::List(Vec::new()))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::SetContains => {
            "{\n            let set = interpreter.eval_named_or_positional_arg(args, \"set\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            Ok(Value::Bool(\n                expect_list(set)?.iter().any(|item| item == &value),\n            ))\n        }"
        }
        InterpreterEvalKind::SetDifference => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let right = expect_list(right)?;\n            Ok(Value::List(\n                expect_list(left)?\n                    .into_iter()\n                    .filter(|value| !right.iter().any(|item| item == value))\n                    .collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::SetForEach => {
            "{\n            let set = interpreter.eval_named_or_positional_arg(args, \"set\", 0)?;\n            let callback = interpreter.eval_named_or_positional_arg(args, \"callback\", 1)?;\n            for value in expect_list(set)? {\n                let _ = interpreter.call_closure(callback.clone(), vec![value])?;\n            }\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::SetInsert => {
            "{\n            let set_name = interpreter.mut_arg_local_name(args, \"set\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let mut set = expect_list(interpreter.lookup(set_name)?)?;\n            let inserted = set_insert(&mut set, value);\n            interpreter.assign(set_name, Value::List(set))?;\n            Ok(Value::Bool(inserted))\n        }"
        }
        InterpreterEvalKind::SetIntersection => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let right = expect_list(right)?;\n            Ok(Value::List(\n                expect_list(left)?\n                    .into_iter()\n                    .filter(|value| right.iter().any(|item| item == value))\n                    .collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::SetIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_list(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::SetIsSubset => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let right = expect_list(right)?;\n            Ok(Value::Bool(\n                expect_list(left)?\n                    .iter()\n                    .all(|value| right.iter().any(|item| item == value)),\n            ))\n        }"
        }
        InterpreterEvalKind::SetLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_list(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::SetNew => "{\n            Ok(Value::List(Vec::new()))\n        }",
        InterpreterEvalKind::SetRemove => {
            "{\n            let set_name = interpreter.mut_arg_local_name(args, \"set\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let mut set = expect_list(interpreter.lookup(set_name)?)?;\n            let removed = set_remove(&mut set, &value);\n            interpreter.assign(set_name, Value::List(set))?;\n            Ok(Value::Bool(removed))\n        }"
        }
        InterpreterEvalKind::SetToList => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
        }
        InterpreterEvalKind::SetUnion => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            let mut set = expect_list(left)?;\n            for value in expect_list(right)? {\n                set_insert(&mut set, value);\n            }\n            Ok(Value::List(set))\n        }"
        }
        InterpreterEvalKind::SenderClose => {
            "{\n            let sender_name = interpreter.mut_arg_local_name(args, \"sender\", 0)?;\n            let sender = expect_sender(interpreter.lookup(sender_name)?)?;\n            if !sender.closed {\n                let state = interpreter.channel_state_mut(sender.channel_id)?;\n                state.senders = state.senders.saturating_sub(1);\n            }\n            interpreter.assign(sender_name, sender_value(sender.channel_id, true))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::SenderSend => {
            "{\n            let sender = interpreter.eval_named_or_positional_arg(args, \"sender\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            Ok(result_value(interpreter.channel_send(expect_sender(sender)?, value)))\n        }"
        }
        InterpreterEvalKind::SenderSendCancellable => {
            "{\n            let sender = interpreter.eval_named_or_positional_arg(args, \"sender\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let sender = expect_sender(sender)?;\n            let token = interpreter.eval_named_or_positional_arg(args, \"token\", 2)?;\n            let id = expect_cancellation_id(token, \"CancellationToken\")?;\n            if interpreter.cancellation_flags.get(&id).copied().unwrap_or(false) {\n                return Ok(value_err(channel_error(\"channel send cancelled\")));\n            }\n            Ok(result_value(interpreter.channel_send(sender, value)))\n        }"
        }
        InterpreterEvalKind::SortedMapClear => {
            "{\n            let map_name = interpreter.mut_arg_local_name(args, \"map\", 0)?;\n            interpreter.assign(map_name, Value::Map(Vec::new()))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::SortedMapContainsKey => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            Ok(Value::Bool(map_get(&expect_map(map)?, &key).is_some()))\n        }"
        }
        InterpreterEvalKind::SortedMapGet => {
            "{\n            let map = interpreter.eval_named_or_positional_arg(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            Ok(value_option(map_get(&expect_map(map)?, &key), |value| {\n                value\n            }))\n        }"
        }
        InterpreterEvalKind::SortedMapInsert => {
            "{\n            let map_name = interpreter.mut_arg_local_name(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 2)?;\n            let mut map = expect_map(interpreter.lookup(map_name)?)?;\n            map_insert(&mut map, key, value);\n            sort_map_entries(&mut map)?;\n            interpreter.assign(map_name, Value::Map(map))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::SortedMapIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_map(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::SortedMapKeys => {
            "{\n            let map = interpreter.eval_first_arg(args)?;\n            Ok(Value::List(\n                expect_map(map)?.into_iter().map(|(key, _)| key).collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::SortedMapLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_map(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::SortedMapNew => "{\n            Ok(Value::Map(Vec::new()))\n        }",
        InterpreterEvalKind::SortedMapRemove => {
            "{\n            let map_name = interpreter.mut_arg_local_name(args, \"map\", 0)?;\n            let key = interpreter.eval_named_or_positional_arg(args, \"key\", 1)?;\n            let mut map = expect_map(interpreter.lookup(map_name)?)?;\n            let old = map_remove(&mut map, &key);\n            interpreter.assign(map_name, Value::Map(map))?;\n            Ok(value_option(old, |value| value))\n        }"
        }
        InterpreterEvalKind::SortedMapValues => {
            "{\n            let map = interpreter.eval_first_arg(args)?;\n            Ok(Value::List(\n                expect_map(map)?\n                    .into_iter()\n                    .map(|(_, value)| value)\n                    .collect(),\n            ))\n        }"
        }
        InterpreterEvalKind::SortedSetClear => {
            "{\n            let set_name = interpreter.mut_arg_local_name(args, \"set\", 0)?;\n            interpreter.assign(set_name, Value::List(Vec::new()))?;\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::SortedSetContains => {
            "{\n            let set = interpreter.eval_named_or_positional_arg(args, \"set\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            Ok(Value::Bool(\n                expect_list(set)?.iter().any(|item| item == &value),\n            ))\n        }"
        }
        InterpreterEvalKind::SortedSetInsert => {
            "{\n            let set_name = interpreter.mut_arg_local_name(args, \"set\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let mut set = expect_list(interpreter.lookup(set_name)?)?;\n            let inserted = set_insert(&mut set, value);\n            sort_values(&mut set)?;\n            interpreter.assign(set_name, Value::List(set))?;\n            Ok(Value::Bool(inserted))\n        }"
        }
        InterpreterEvalKind::SortedSetIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_list(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::SortedSetLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_list(value)?.len() as i64))\n        }"
        }
        InterpreterEvalKind::SortedSetNew => {
            "{\n            Ok(Value::List(Vec::new()))\n        }"
        }
        InterpreterEvalKind::SortedSetRemove => {
            "{\n            let set_name = interpreter.mut_arg_local_name(args, \"set\", 0)?;\n            let value = interpreter.eval_named_or_positional_arg(args, \"value\", 1)?;\n            let mut set = expect_list(interpreter.lookup(set_name)?)?;\n            let removed = set_remove(&mut set, &value);\n            interpreter.assign(set_name, Value::List(set))?;\n            Ok(Value::Bool(removed))\n        }"
        }
        InterpreterEvalKind::SortedSetToList => {
            "{\n            interpreter.eval_first_arg(args)\n        }"
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
        InterpreterEvalKind::StringEnv => {
            "{\n            let name = interpreter.eval_first_arg(args)?;\n            Ok(value_option(\n                std::env::var(expect_string(name)?).ok(),\n                Value::String,\n            ))\n        }"
        }
        InterpreterEvalKind::StringEnvOr => {
            "{\n            let name = interpreter.eval_named_or_positional_arg(args, \"value\", 0)?;\n            let default = interpreter.eval_named_or_positional_arg(args, \"default\", 1)?;\n            Ok(Value::String(\n                std::env::var(expect_string(name)?).unwrap_or(expect_string(default)?),\n            ))\n        }"
        }
        InterpreterEvalKind::StringJoin => {
            "{\n            let parts = interpreter.eval_named_or_positional_arg(args, \"parts\", 0)?;\n            let separator = interpreter.eval_named_or_positional_arg(args, \"separator\", 1)?;\n            Ok(Value::String(\n                expect_string_list(parts)?.join(&expect_string(separator)?),\n            ))\n        }"
        }
        InterpreterEvalKind::StreamCollectList => {
            "{\n            let stream = interpreter.eval_named_or_positional_arg(args, \"stream\", 0)?;\n            Ok(result_value(\n                interpreter.expect_stream_collect_items(stream)?.map(Value::List),\n            ))\n        }"
        }
        InterpreterEvalKind::StreamFromList => {
            "{\n            let items = interpreter.eval_named_or_positional_arg(args, \"items\", 0)?;\n            Ok(interpreter.stream_from_list(expect_list(items)?))\n        }"
        }
        InterpreterEvalKind::StreamNext => {
            "{\n            let stream = interpreter.eval_named_or_positional_arg(args, \"stream\", 0)?;\n            Ok(result_value(interpreter.stream_next(stream)?))\n        }"
        }
        InterpreterEvalKind::TcpConnect => {
            "{\n            let host = interpreter.eval_named_or_positional_arg(args, \"host\", 0)?;\n            let port = interpreter.eval_named_or_positional_arg(args, \"port\", 1)?;\n            let host = expect_string(host)?;\n            let port = expect_int(port)?;\n            Ok(result_value(\n                interpreter.tcp_connect(&host, port).map_err(tcp_error),\n            ))\n        }"
        }
        InterpreterEvalKind::TcpErrorMessage | InterpreterEvalKind::WebSocketErrorMessage => {
            "{\n            let error = interpreter.eval_named_or_positional_arg(args, \"error\", 0)?;\n            read_field(&error, \"message\")\n        }"
        }
        InterpreterEvalKind::TcpStreamRead => {
            "{\n            let stream = interpreter.eval_named_or_positional_arg(args, \"stream\", 0)?;\n            let max_bytes = interpreter.eval_named_or_positional_arg(args, \"max_bytes\", 1)?;\n            let id = expect_tcp_stream_id(stream)?;\n            Ok(result_value(\n                interpreter\n                    .tcp_stream_read(id, expect_int(max_bytes)?)\n                    .map(Value::Bytes)\n                    .map_err(tcp_error),\n            ))\n        }"
        }
        InterpreterEvalKind::TcpStreamWrite => {
            "{\n            let stream = interpreter.eval_named_or_positional_arg(args, \"stream\", 0)?;\n            let data = interpreter.eval_named_or_positional_arg(args, \"data\", 1)?;\n            let id = expect_tcp_stream_id(stream)?;\n            Ok(result_value(\n                interpreter\n                    .tcp_stream_write(id, &expect_bytes(data)?)\n                    .map(Value::Int)\n                    .map_err(tcp_error),\n            ))\n        }"
        }
        InterpreterEvalKind::TcpStreamWriteAll => {
            "{\n            let stream = interpreter.eval_named_or_positional_arg(args, \"stream\", 0)?;\n            let data = interpreter.eval_named_or_positional_arg(args, \"data\", 1)?;\n            let id = expect_tcp_stream_id(stream)?;\n            Ok(result_value(\n                interpreter\n                    .tcp_stream_write_all(id, &expect_bytes(data)?)\n                    .map(|_| Value::Unit)\n                    .map_err(tcp_error),\n            ))\n        }"
        }
        InterpreterEvalKind::TcpStreamShutdown => {
            "{\n            let stream = interpreter.eval_named_or_positional_arg(args, \"stream\", 0)?;\n            let id = expect_tcp_stream_id(stream)?;\n            Ok(result_value(\n                interpreter\n                    .tcp_stream_shutdown(id)\n                    .map(|_| Value::Unit)\n                    .map_err(tcp_error),\n            ))\n        }"
        }
        InterpreterEvalKind::TempDirKeep => {
            "{\n            let dir = interpreter.eval_named_or_positional_arg(args, \"dir\", 0)?;\n            Ok(Value::String(expect_tempdir_path(dir)?))\n        }"
        }
        InterpreterEvalKind::TempDirNew => {
            "{\n            Ok(result_value(tempdir_new_value(std::env::temp_dir())))\n        }"
        }
        InterpreterEvalKind::TempDirNewIn => {
            "{\n            let parent = interpreter.eval_named_or_positional_arg(args, \"parent\", 0)?;\n            Ok(result_value(tempdir_new_value(PathBuf::from(\n                expect_string(parent)?,\n            ))))\n        }"
        }
        InterpreterEvalKind::TempDirPath => {
            "{\n            let dir = interpreter\n                .eval_named_or_positional_arg(args, \"dir\", 0)\n                .or_else(|_| interpreter.eval_first_arg(args))?;\n            Ok(Value::String(expect_tempdir_path(dir)?))\n        }"
        }
        InterpreterEvalKind::TimerSleep => {
            "{\n            let ms = interpreter.eval_named_or_positional_arg(args, \"ms\", 0)?;\n            let _ = expect_int(ms)?;\n            Ok(value_ok(Value::Unit))\n        }"
        }
        InterpreterEvalKind::TimerSleepCancellable => {
            "{\n            let ms = interpreter.eval_named_or_positional_arg(args, \"ms\", 0)?;\n            let token = interpreter.eval_named_or_positional_arg(args, \"token\", 1)?;\n            let _ = expect_int(ms)?;\n            let _ = expect_cancellation_id(token, \"CancellationToken\")?;\n            Ok(value_ok(Value::Unit))\n        }"
        }
        InterpreterEvalKind::TimerSleepUntil => {
            "{\n            let deadline = interpreter.eval_named_or_positional_arg(args, \"deadline\", 0)?;\n            let _ = expect_deadline_unix_ms(deadline)?;\n            Ok(value_ok(Value::Unit))\n        }"
        }
        InterpreterEvalKind::TomlParseFile => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            Ok(json_result(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map_err(|error| error.to_string())\n                    .and_then(|text| {\n                        text.parse::<toml::Value>()\n                            .map_err(|error| error.to_string())\n                    })\n                    .and_then(|value| {\n                        serde_json::to_value(value).map_err(|error| error.to_string())\n                    }),\n            ))\n        }"
        }
        InterpreterEvalKind::WebSocketConnect => {
            "{\n            let url = interpreter.eval_named_or_positional_arg(args, \"url\", 0)?;\n            let url = expect_string(url)?;\n            Ok(result_value(\n                interpreter.websocket_connect(&url).map_err(websocket_error),\n            ))\n        }"
        }
        InterpreterEvalKind::WebSocketSendText => {
            "{\n            let socket = interpreter.eval_named_or_positional_arg(args, \"socket\", 0)?;\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 1)?;\n            let id = expect_websocket_id(socket)?;\n            Ok(result_value(\n                interpreter\n                    .websocket_send_text(id, &expect_string(text)?)\n                    .map(|_| Value::Unit)\n                    .map_err(websocket_error),\n            ))\n        }"
        }
        InterpreterEvalKind::WebSocketSendBytes => {
            "{\n            let socket = interpreter.eval_named_or_positional_arg(args, \"socket\", 0)?;\n            let bytes = interpreter.eval_named_or_positional_arg(args, \"bytes\", 1)?;\n            let id = expect_websocket_id(socket)?;\n            Ok(result_value(\n                interpreter\n                    .websocket_send_bytes(id, &expect_bytes(bytes)?)\n                    .map(|_| Value::Unit)\n                    .map_err(websocket_error),\n            ))\n        }"
        }
        InterpreterEvalKind::WebSocketRecvText => {
            "{\n            let socket = interpreter.eval_named_or_positional_arg(args, \"socket\", 0)?;\n            let id = expect_websocket_id(socket)?;\n            Ok(result_value(\n                interpreter\n                    .websocket_recv_text(id)\n                    .map(|value| value.map(Value::String).map_or_else(value_none, value_some))\n                    .map_err(websocket_error),\n            ))\n        }"
        }
        InterpreterEvalKind::WebSocketRecvBytes => {
            "{\n            let socket = interpreter.eval_named_or_positional_arg(args, \"socket\", 0)?;\n            let id = expect_websocket_id(socket)?;\n            Ok(result_value(\n                interpreter\n                    .websocket_recv_bytes(id)\n                    .map(|value| value.map(Value::Bytes).map_or_else(value_none, value_some))\n                    .map_err(websocket_error),\n            ))\n        }"
        }
        InterpreterEvalKind::WebSocketClose => {
            "{\n            let socket = interpreter.eval_named_or_positional_arg(args, \"socket\", 0)?;\n            let id = expect_websocket_id(socket)?;\n            Ok(result_value(\n                interpreter\n                    .websocket_close(id)\n                    .map(|_| Value::Unit)\n                    .map_err(websocket_error),\n            ))\n        }"
        }
        InterpreterEvalKind::YamlParse => {
            "{\n            let text = interpreter.eval_named_or_positional_arg(args, \"text\", 0)?;\n            Ok(yaml_parse_result(&expect_string(text)?))\n        }"
        }
        InterpreterEvalKind::YamlParseFile => {
            "{\n            let path = interpreter.eval_named_or_positional_arg(args, \"path\", 0)?;\n            Ok(json_result(\n                std::fs::read_to_string(expect_string(path)?)\n                    .map_err(|error| error.to_string())\n                    .and_then(|text| yaml_parse_json(&text)),\n            ))\n        }"
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

fn collect_interpreter_interface_functions(source: &str) -> Vec<String> {
    let mut functions = collect_functions(source);
    functions.extend(collect_protocol_method_signatures(source));
    functions.sort();
    functions.dedup();
    functions
}

fn collect_protocol_method_signatures(source: &str) -> Vec<String> {
    let mut methods = Vec::new();
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
        if &source[token_start..index] != "protocol" {
            continue;
        }
        index = skip_ascii_whitespace(bytes, index);
        let Some((protocol, after_protocol)) = read_symbol(source, index) else {
            continue;
        };
        index = skip_ascii_whitespace(bytes, after_protocol);
        if bytes.get(index) != Some(&b'{') {
            continue;
        }
        let open = index;
        let Some(close) = find_matching_brace(source, open) else {
            break;
        };
        for method in collect_functions(&source[open + 1..close]) {
            methods.push(format!("{protocol}.{method}"));
        }
        index = close + 1;
    }
    methods
}

fn read_symbol(source: &str, index: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    if !bytes.get(index).copied().is_some_and(is_symbol_start) {
        return None;
    }
    let start = index;
    let mut end = index + 1;
    while end < bytes.len() && is_symbol_continue(bytes[end]) {
        end += 1;
    }
    Some((source[start..end].to_string(), end))
}

fn find_matching_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 1usize;
    let mut index = open + 1;
    while index < bytes.len() {
        if starts_line_comment(bytes, index) {
            index = skip_line_comment(bytes, index + 2);
            continue;
        }
        if bytes[index] == b'"' {
            index = skip_string_literal(bytes, index + 1);
            continue;
        }
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
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
