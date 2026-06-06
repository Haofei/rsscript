use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use base64::Engine;
use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, TimeZone, Timelike, Utc};
use hmac::{Hmac, Mac};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::diagnostic::Severity;
use crate::eval_types::{EvalError, EvalOutput, NativeInterpreterFn, NativeValue};
use crate::hir::{
    Hir, HirBlock, HirCallArg, HirCallReceiver, HirExpr, HirMatchArm, HirStmt, TypeInfo,
};
use crate::interfaces::builtin_interfaces;
use crate::package::package_lowering_input;
use crate::syntax::ast::{
    BinaryOp, Callee, MatchFieldPattern, MatchLiteral, MatchPattern, merge_programs,
};
use crate::syntax::parse_source;
use crate::vm_value::{VmClosure, VmMapKey, VmNative, VmStruct, VmValue};

const MS_PER_DAY: i64 = 86_400_000;

const URL_COMPONENT_SET: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn reg_vm_eval_source_main_with_args(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args(args)
}

pub fn reg_vm_eval_source_main(file: &str, source: &str) -> Result<EvalOutput, EvalError> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    reg_vm_eval_source_main_with_args(file, source, args)
}

pub fn reg_vm_eval_source_main_with_args_and_native_bindings(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?
        .eval_main_with_args_and_native_bindings(args, native_bindings)
}

pub fn reg_vm_eval_package_main_with_args(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_eval_package_main_with_args_and_native_bindings(
        package_dir,
        args,
        std::iter::empty::<(String, NativeInterpreterFn)>(),
    )
}

pub fn reg_vm_eval_package_main_with_args_and_native_bindings(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
    native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
) -> Result<EvalOutput, EvalError> {
    let input = package_lowering_input(package_dir).map_err(EvalError::Runtime)?;
    let mut interface_refs = builtin_interfaces()
        .map(|(path, contents)| (path.to_string(), contents.to_string()))
        .collect::<Vec<_>>();
    interface_refs.extend(input.interfaces.iter().cloned());
    let source_refs = input
        .sources
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let interface_refs_borrowed = interface_refs
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let diagnostics =
        crate::analyze_sources_with_interfaces_without_core(&source_refs, &interface_refs_borrowed);
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity.is_error())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(EvalError::Diagnostics(errors));
    }

    let program = merge_programs(
        input
            .sources
            .iter()
            .map(|(path, source)| parse_source(path, source)),
    );
    let interface_programs = interface_refs
        .iter()
        .map(|(path, source)| parse_source(path, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&program, &interface_programs);
    Ok(RegVmExecutable {
        unit: Rc::new(RegUnit::lower(&hir)?),
    })
    .and_then(|executable| {
        executable.eval_main_with_args_and_native_bindings(args, native_bindings)
    })
}

#[derive(Debug, Clone)]
pub struct RegVmExecutable {
    unit: Rc<RegUnit>,
}

pub fn reg_vm_compile_source(file: &str, source: &str) -> Result<RegVmExecutable, EvalError> {
    let diagnostics = crate::analyze_source_with_core(file, source);
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(EvalError::Diagnostics(errors));
    }

    let program = parse_source(file, source);
    let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
    Ok(RegVmExecutable {
        unit: Rc::new(RegUnit::lower(&hir)?),
    })
}

impl RegVmExecutable {
    pub fn eval_main_with_args(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_native_bindings(
            args,
            std::iter::empty::<(String, NativeInterpreterFn)>(),
        )
    }

    pub fn eval_main_with_args_and_native_bindings(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
    ) -> Result<EvalOutput, EvalError> {
        let mut vm = RegVm::new(
            Rc::clone(&self.unit),
            args.into_iter().map(Into::into).collect(),
            native_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
        );
        let value = vm.call_function("main", Vec::new())?;
        let display_value = value.display();
        let native_value = value.native_value();
        Ok(EvalOutput {
            value: display_value.clone(),
            display_value,
            native_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
        })
    }
}

#[derive(Debug, Clone)]
struct RegUnit {
    functions: Vec<Rc<RegFunction>>,
    function_ids: HashMap<String, usize>,
    resource_drop_functions: HashMap<String, usize>,
    types: HashMap<String, TypeInfo>,
}

#[derive(Debug, Clone)]
struct RegFunction {
    name: String,
    params: usize,
    captures: usize,
    regs: usize,
    local_regs: HashMap<String, Reg>,
    code: Vec<RegInstr>,
}

impl RegFunction {
    fn placeholder(name: String) -> Self {
        Self {
            name,
            params: 0,
            captures: 0,
            regs: 0,
            local_regs: HashMap::new(),
            code: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
enum RegInstr {
    LoadUnit {
        dst: Reg,
    },
    LoadInt {
        dst: Reg,
        value: i64,
    },
    LoadFloat {
        dst: Reg,
        value: f64,
    },
    LoadBool {
        dst: Reg,
        value: bool,
    },
    LoadString {
        dst: Reg,
        value: Rc<String>,
    },
    Move {
        dst: Reg,
        src: Reg,
    },
    Manage {
        dst: Reg,
        src: Reg,
    },
    GetField {
        dst: Reg,
        base: Reg,
        name: String,
    },
    MakeStruct {
        dst: Reg,
        name: String,
        fields: Vec<(String, Reg)>,
    },
    ResourceDrop {
        resource: Reg,
    },
    MakeVariant {
        dst: Reg,
        name: String,
        fields: Vec<(String, Reg)>,
    },
    MakeList {
        dst: Reg,
        items: Vec<Reg>,
    },
    MakeObject {
        dst: Reg,
        fields: Vec<(String, Reg)>,
    },
    MakeMap {
        dst: Reg,
        entries: Vec<(Reg, Reg)>,
    },
    AddInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    SubInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    MulInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    DivInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ModInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitAndInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitOrInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitXorInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ShiftLeftInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ShiftRightInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    LessInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    LessEqualInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    GreaterInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    GreaterEqualInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Equal {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    NotEqual {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Jump {
        target: usize,
    },
    JumpIfBool {
        cond: Reg,
        expected: bool,
        target: usize,
    },
    JumpIfIntCompare {
        lhs: Reg,
        rhs: Reg,
        op: RegIntCompare,
        expected: bool,
        target: usize,
    },
    MatchOption {
        src: Reg,
        some_ip: usize,
        none_ip: usize,
    },
    MatchResult {
        src: Reg,
        ok_ip: usize,
        err_ip: usize,
    },
    MatchVariant {
        src: Reg,
        expected: String,
        match_ip: usize,
        else_ip: usize,
    },
    RuntimeError {
        message: String,
    },
    MatchMapGet {
        map: Reg,
        key: Reg,
        value_dst: Reg,
        some_ip: usize,
        none_ip: usize,
    },
    UnwrapSome {
        dst: Reg,
        src: Reg,
    },
    UnwrapVariantValue {
        dst: Reg,
        src: Reg,
        expected: String,
    },
    MakeClosure {
        dst: Reg,
        function: usize,
        captures: Vec<Reg>,
    },
    MakeSome {
        dst: Reg,
        value: Reg,
    },
    LoadNone {
        dst: Reg,
    },
    CallKnown {
        dst: Reg,
        function: usize,
        args: Vec<Reg>,
    },
    CallNative {
        dst: Reg,
        key: String,
        args: Vec<Reg>,
    },
    #[allow(dead_code)]
    CallClosure {
        dst: Reg,
        closure: Reg,
        args: Vec<Reg>,
    },
    ListFilter {
        dst: Reg,
        list: Reg,
        predicate: Reg,
    },
    ListFold {
        dst: Reg,
        list: Reg,
        state: Reg,
        folder: Reg,
    },
    ListGet {
        dst: Reg,
        list: Reg,
        index: Reg,
    },
    ListLen {
        dst: Reg,
        list: Reg,
    },
    ListMap {
        dst: Reg,
        list: Reg,
        mapper: Reg,
    },
    ListAppend {
        dst: Reg,
        list: Reg,
        values: Reg,
    },
    ListClear {
        dst: Reg,
        list: Reg,
    },
    ListPop {
        dst: Reg,
        list: Reg,
    },
    ListPush {
        dst: Reg,
        list: Reg,
        value: Reg,
    },
    ListRemoveAt {
        dst: Reg,
        list: Reg,
        index: Reg,
    },
    ListSet {
        dst: Reg,
        list: Reg,
        index: Reg,
        value: Reg,
    },
    ListSort {
        dst: Reg,
        list: Reg,
    },
    ListSortBy {
        dst: Reg,
        list: Reg,
        key: Reg,
        compare: Reg,
    },
    ListSortWith {
        dst: Reg,
        list: Reg,
        compare: Reg,
    },
    DequeClear {
        dst: Reg,
        deque: Reg,
    },
    DequePopBack {
        dst: Reg,
        deque: Reg,
    },
    DequePopFront {
        dst: Reg,
        deque: Reg,
    },
    DequePushBack {
        dst: Reg,
        deque: Reg,
        value: Reg,
    },
    DequePushFront {
        dst: Reg,
        deque: Reg,
        value: Reg,
    },
    SetClear {
        dst: Reg,
        set: Reg,
    },
    SetForEach {
        dst: Reg,
        set: Reg,
        callback: Reg,
    },
    SetInsert {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SetRemove {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedSetClear {
        dst: Reg,
        set: Reg,
    },
    SortedSetInsert {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedSetRemove {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedMapClear {
        dst: Reg,
        map: Reg,
    },
    SortedMapInsert {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    SortedMapRemove {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    MapGet {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    MapClear {
        dst: Reg,
        map: Reg,
    },
    MapInsertOld {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    MapRemove {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    BufferClear {
        dst: Reg,
        buffer: Reg,
    },
    CounterAdd {
        dst: Reg,
        counter: Reg,
        amount: Reg,
    },
    ConfigStoreReplace {
        dst: Reg,
        store: Reg,
        value: Reg,
    },
    GlobalConfigReplace {
        dst: Reg,
        global: Reg,
        value: Reg,
    },
    MapInsert {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    StringBuilderPush {
        dst: Reg,
        builder: Reg,
        value: Reg,
    },
    StringConcat {
        dst: Reg,
        left: Reg,
        right: Reg,
    },
    CallIntrinsic {
        dst: Reg,
        intrinsic: RegIntrinsic,
        args: Vec<Reg>,
    },
    CallTypedIntrinsic {
        dst: Reg,
        intrinsic: RegIntrinsic,
        type_arg: String,
        args: Vec<Reg>,
    },
    TryResult {
        dst: Reg,
        src: Reg,
        cleanup: Vec<Reg>,
    },
    Return {
        src: Reg,
    },
}

#[derive(Debug, Clone, Copy)]
enum RegIntrinsic {
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
    BytesConcat,
    BytesConsume,
    BytesFromString,
    BytesIsEmpty,
    BytesLen,
    BytesSlice,
    BytesViewStartsWith,
    BytesViewToBytes,
    BufferNew,
    CacheGet,
    CacheLookup,
    CancellationSourceCancel,
    CancellationSourceNew,
    CancellationSourceToken,
    CancellationTokenIsCancelled,
    ChannelBounded,
    ChannelReceiver,
    ChannelSender,
    ChannelErrorMessage,
    CharCompare,
    CharFromCode,
    CharIsAlphanumeric,
    CharIsAlpha,
    CharIsDigit,
    CharIsLower,
    CharIsUpper,
    CharIsWhitespace,
    CharToCode,
    CharToLower,
    CharToString,
    CharToUpper,
    ClockNow,
    ClockSystemUnixMs,
    ConfigLoad,
    CapabilityFrom,
    ConfigName,
    ConfigNew,
    ConfigRuleCount,
    ConfigStoreName,
    ConfigStoreNew,
    CounterNew,
    CounterValue,
    CsvOpenRead,
    CsvParseRow,
    CsvReadInto,
    CsvRows,
    DateAddDays,
    DateAddMs,
    DateDay,
    DateDaysBetween,
    DateDaysInMonth,
    DateFormatIso,
    DateFormatYmd,
    DateHour,
    DateIsLeapYear,
    DateMinute,
    DateMonth,
    DateParseIso,
    DateParseYmd,
    DateSecond,
    DateStartOfDay,
    DateWeekday,
    DateYear,
    DecodeErrorMessage,
    DeadlineAfter,
    DeadlineAfterMs,
    DeadlineIsExpired,
    DeadlineRemainingMs,
    DequeIsEmpty,
    DequeLen,
    DequeNew,
    DequeToList,
    DiffUnified,
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
    DbClose,
    DbConnectionOpen,
    DbConnectionQuery,
    DbConnectionTryOpen,
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
    FalliblePipelineCollect,
    FalliblePipelineEach,
    FalliblePipelineFilter,
    FalliblePipelineMap,
    FalliblePipelineTryMap,
    FunctionObjectHasClosure,
    FunctionObjectNew,
    HashSha256Bytes,
    HashSha256File,
    HashSha256String,
    HmacSha256Bytes,
    HmacSha256String,
    GlobalConfigNew,
    GlobalConfigRuleCount,
    HexDecode,
    HexEncode,
    HexEncodeString,
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
    InstantElapsed,
    FloatToString,
    FloatIsFinite,
    FloatIsInfinite,
    FloatIsNan,
    IntToString,
    IntBitAnd,
    IntBitNot,
    IntBitOr,
    IntBitXor,
    IntShiftLeft,
    IntShiftRight,
    MathAbs,
    MathAbsFloat,
    MathCeil,
    MathClamp,
    MathClampFloat,
    MathFloor,
    MathMax,
    MathMaxFloat,
    MathMin,
    MathMinFloat,
    MathPow,
    MathPowFloat,
    MathRound,
    MathSqrt,
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
    JsonAsBool,
    JsonAsInt,
    JsonAsString,
    JsonBoolAt,
    JsonBoolAtOr,
    JsonBoolField,
    JsonClone,
    JsonDecode,
    JsonDecodeText,
    JsonEncode,
    JsonErrorMessage,
    JsonField,
    JsonFieldBool,
    JsonFieldInt,
    JsonFieldOptional,
    JsonFieldOptionalBool,
    JsonFieldOptionalInt,
    JsonFieldOptionalString,
    JsonFieldString,
    JsonIntAt,
    JsonIntAtOr,
    JsonIntField,
    JsonIsArray,
    JsonIsNull,
    JsonIsObject,
    JsonKind,
    JsonObject,
    JsonObjectKeys,
    JsonObjectLen,
    JsonParse,
    JsonParseFile,
    JsonQuoteString,
    JsonRawField,
    JsonStringAt,
    JsonStringAtOr,
    JsonStringArray,
    JsonStringField,
    JsonStrings,
    JsonToStringAt,
    JsonToStringAtOr,
    JsonToString,
    JsonValue,
    JsonValues,
    ListAll,
    ListAny,
    ListConsume,
    ListContains,
    ListContainsValue,
    ListCountWhere,
    ListEnumerate,
    ListFind,
    ListFlatMap,
    ListFlatten,
    ListFirst,
    ListGroupBy,
    ListIsEmpty,
    ListJoin,
    ListLast,
    ListDedup,
    ListMax,
    ListMin,
    ListNew,
    ListPartition,
    ListPipeline,
    ListReverse,
    ListSkip,
    ListSlice,
    ListSum,
    ListTake,
    ListZip,
    ListToJsonStrings,
    ListToJsonValues,
    ListTryFold,
    LogError,
    LogErrorJson,
    LogTrace,
    LogWrite,
    LogWriteJson,
    MapContainsKey,
    MapFilter,
    MapFold,
    MapForEach,
    MapGetOrDefault,
    MapIsEmpty,
    MapKeys,
    MapLen,
    MapMapValues,
    MapMerge,
    MapNew,
    MapTryFold,
    MapValues,
    OptionIsNone,
    OptionIsSome,
    OptionAndThen,
    OptionFilter,
    OptionMap,
    OptionOkOr,
    OptionOr,
    OptionUnwrapOr,
    OptionUnwrapOrElse,
    OrdCompare,
    OsClose,
    PatchApplyText,
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
    PersistentMapClear,
    PersistentMapContainsKey,
    PersistentMapGet,
    PersistentMapInsert,
    PersistentMapIsEmpty,
    PersistentMapLen,
    PersistentMapNew,
    PersistentMapRemove,
    PipelineCollect,
    PipelineEach,
    PoolErrorMessage,
    PoolStatsAvailable,
    PoolStatsCapacity,
    PoolStatsCreated,
    PoolStatsInUse,
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
    RandomBool,
    RandomBytes,
    RandomFloat,
    RandomInt,
    RandomString,
    RegexCaptures,
    RegexCompile,
    RegexErrorMessage,
    RegexFind,
    RegexIsMatch,
    RegexReplaceAll,
    RegexSplit,
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
    RequestNew,
    RequestPath,
    ReceiverClose,
    ReceiverIntoStream,
    ReceiverRecv,
    ReceiverRecvCancellable,
    ResponseBody,
    ResponseOk,
    ResponseStatus,
    RowBufferNew,
    RowFieldString,
    RuleLoaderLoadRules,
    ResourcePoolBorrow,
    ResourcePoolDiscard,
    ResourcePoolLazy,
    ResourcePoolNew,
    ResourcePoolStats,
    ResourcePoolTryBorrow,
    ResourcePoolTryLazy,
    ResourcePoolTryNew,
    SetContains,
    SetDifference,
    SetIntersection,
    SetIsEmpty,
    SetIsSubset,
    SetLen,
    SetNew,
    SetToList,
    SetUnion,
    SortedSetContains,
    SortedSetIsEmpty,
    SortedSetLen,
    SortedSetNew,
    SortedSetToList,
    SortedMapContainsKey,
    SortedMapGet,
    SortedMapIsEmpty,
    SortedMapKeys,
    SortedMapLen,
    SortedMapNew,
    SortedMapValues,
    StringAfter,
    StringBefore,
    StringBuilderNew,
    StringCharAt,
    StringChars,
    StringContains,
    StringCount,
    StringCopy,
    StringEndsWith,
    StringFromBool,
    StringFromFloat,
    StringFormat,
    StringIndexOf,
    StringFromInt,
    StringIsEmpty,
    StringJoin,
    StringLines,
    StringLen,
    StringPadLeft,
    StringPadRight,
    StringParseFloat,
    StringParseInt,
    StringRepeat,
    StringReplace,
    StringReplaceFirst,
    StringReverse,
    StringSlice,
    StringSplit,
    StringStartsWith,
    StringStripPrefix,
    StringToLowercase,
    StringToUppercase,
    StringTrim,
    StringTrimEnd,
    StringTrimStart,
    StreamCollectList,
    StreamFromList,
    StreamNext,
    SenderClose,
    SenderSend,
    SenderSendCancellable,
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
    TomlParseFile,
    UuidNewV4,
    UrlDecodeComponent,
    UrlEncodeComponent,
    UrlFromString,
    UrlToString,
    TimerSleep,
    TimerSleepCancellable,
    TimerSleepUntil,
    WebSocketClose,
    WebSocketConnect,
    WebSocketErrorMessage,
    WebSocketRecvBytes,
    WebSocketRecvText,
    WebSocketSendBytes,
    WebSocketSendText,
    YamlParse,
    YamlParseFile,
    WeakDowngrade,
    WeakFrom,
    WeakUpgrade,
}

#[derive(Debug, Clone, Copy)]
enum RegIntCompare {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

type Reg = usize;

#[derive(Debug, Default)]
struct LoopPatch {
    breaks: Vec<usize>,
    continues: Vec<usize>,
    cleanup_base: usize,
}

#[derive(Debug, Clone, Copy)]
enum MatchFailurePatch {
    Jump(usize),
    OptionSome(usize),
    OptionNone(usize),
    ResultOk(usize),
    ResultErr(usize),
    VariantOther(usize),
}

impl RegUnit {
    fn lower(hir: &Hir) -> Result<Self, EvalError> {
        let names = hir
            .function_bodies()
            .filter_map(|(name, body)| body.block.as_ref().map(|_| name.to_string()))
            .collect::<Vec<_>>();
        let function_ids = names
            .iter()
            .enumerate()
            .map(|(id, name)| (name.clone(), id))
            .collect::<HashMap<_, _>>();
        let mut functions = names
            .iter()
            .cloned()
            .map(RegFunction::placeholder)
            .collect::<Vec<_>>();
        for (function_id, name) in names.into_iter().enumerate() {
            let body = hir
                .function_body(&name)
                .and_then(|body| body.block.as_ref())
                .ok_or_else(|| {
                    EvalError::Runtime(format!("reg VM cannot find function `{name}`."))
                })?;
            let signature = hir.resolve_function(None, &name).ok_or_else(|| {
                EvalError::Runtime(format!("reg VM cannot resolve function `{name}`."))
            })?;
            let mut lowerer = RegLowerer {
                hir,
                function_ids: &function_ids,
                functions: &mut functions,
                function: RegFunction {
                    name,
                    params: signature.params.len(),
                    captures: 0,
                    regs: 0,
                    local_regs: HashMap::new(),
                    code: Vec::new(),
                },
                loop_stack: Vec::new(),
                cleanup_stack: Vec::new(),
            };
            for param in &signature.params {
                lowerer.local(&param.name);
            }
            lowerer.block(body)?;
            let unit = lowerer.temp();
            lowerer.emit(RegInstr::LoadUnit { dst: unit });
            lowerer.emit(RegInstr::Return { src: unit });
            functions[function_id] = lowerer.function;
        }
        let mut resource_drop_functions = HashMap::new();
        for (type_name, body) in hir.resource_drop_bodies() {
            let function_id = functions.len();
            functions.push(RegFunction::placeholder(format!("<drop:{type_name}>")));
            let mut lowerer = RegLowerer {
                hir,
                function_ids: &function_ids,
                functions: &mut functions,
                function: RegFunction {
                    name: format!("<drop:{type_name}>"),
                    params: 0,
                    captures: 0,
                    regs: 0,
                    local_regs: HashMap::new(),
                    code: Vec::new(),
                },
                loop_stack: Vec::new(),
                cleanup_stack: Vec::new(),
            };
            if let Some(info) = hir.type_info(type_name) {
                for field in &info.fields_ordered {
                    lowerer.local(&field.name);
                }
            }
            lowerer.block(body)?;
            let unit = lowerer.temp();
            lowerer.emit(RegInstr::LoadUnit { dst: unit });
            lowerer.emit(RegInstr::Return { src: unit });
            functions[function_id] = lowerer.function;
            resource_drop_functions.insert(type_name.to_string(), function_id);
        }
        Ok(Self {
            functions: functions.into_iter().map(Rc::new).collect(),
            function_ids,
            resource_drop_functions,
            types: hir
                .types()
                .map(|type_info| (type_info.name.clone(), type_info.clone()))
                .collect(),
        })
    }
}

struct RegLowerer<'a> {
    hir: &'a Hir,
    function_ids: &'a HashMap<String, usize>,
    functions: &'a mut Vec<RegFunction>,
    function: RegFunction,
    loop_stack: Vec<LoopPatch>,
    cleanup_stack: Vec<Reg>,
}

impl RegLowerer<'_> {
    fn local(&mut self, name: &str) -> Reg {
        if let Some(reg) = self.function.local_regs.get(name) {
            return *reg;
        }
        let reg = self.temp();
        self.function.local_regs.insert(name.to_string(), reg);
        reg
    }

    fn lookup_local(&self, name: &str) -> Result<Reg, EvalError> {
        self.function
            .local_regs
            .get(name)
            .copied()
            .ok_or_else(|| EvalError::Runtime(format!("reg VM cannot resolve local `{name}`.")))
    }

    fn temp(&mut self) -> Reg {
        let reg = self.function.regs;
        self.function.regs += 1;
        reg
    }

    fn emit(&mut self, instr: RegInstr) -> usize {
        let ip = self.function.code.len();
        self.function.code.push(instr);
        ip
    }

    fn cleanup_regs_since(&self, base: usize) -> Vec<Reg> {
        self.cleanup_stack[base..].iter().rev().copied().collect()
    }

    fn all_cleanup_regs(&self) -> Vec<Reg> {
        self.cleanup_regs_since(0)
    }

    fn emit_cleanup_since(&mut self, base: usize) {
        for resource in self.cleanup_regs_since(base) {
            self.emit(RegInstr::ResourceDrop { resource });
        }
    }

    fn emit_all_cleanup(&mut self) {
        self.emit_cleanup_since(0);
    }

    fn patch_jump(&mut self, jump_ip: usize, target: usize) {
        match &mut self.function.code[jump_ip] {
            RegInstr::Jump {
                target: jump_target,
            }
            | RegInstr::JumpIfBool {
                target: jump_target,
                ..
            }
            | RegInstr::JumpIfIntCompare {
                target: jump_target,
                ..
            }
            | RegInstr::MatchOption {
                some_ip: jump_target,
                ..
            }
            | RegInstr::MatchResult {
                ok_ip: jump_target, ..
            }
            | RegInstr::MatchVariant {
                match_ip: jump_target,
                ..
            } => *jump_target = target,
            _ => {}
        }
    }

    fn patch_match_none(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchOption { none_ip, .. } = &mut self.function.code[match_ip] {
            *none_ip = target;
        }
    }

    fn patch_result_match_err(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchResult { err_ip, .. } = &mut self.function.code[match_ip] {
            *err_ip = target;
        }
    }

    fn patch_variant_match_else(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchVariant { else_ip, .. } = &mut self.function.code[match_ip] {
            *else_ip = target;
        }
    }

    fn patch_map_match_some(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchMapGet { some_ip, .. } = &mut self.function.code[match_ip] {
            *some_ip = target;
        }
    }

    fn patch_map_match_none(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchMapGet { none_ip, .. } = &mut self.function.code[match_ip] {
            *none_ip = target;
        }
    }

    fn patch_many(&mut self, jump_ips: Vec<usize>, target: usize) {
        for jump_ip in jump_ips {
            self.patch_jump(jump_ip, target);
        }
    }

    fn block(&mut self, block: &HirBlock) -> Result<(), EvalError> {
        for statement in &block.statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    fn expr_block_value(&mut self, block: &HirBlock) -> Result<Reg, EvalError> {
        let Some((last, prefix)) = block.statements.split_last() else {
            return Err(EvalError::Runtime(
                "reg VM match expression arm cannot be empty.".to_string(),
            ));
        };
        for statement in prefix {
            self.statement(statement)?;
        }
        match last {
            HirStmt::Expr(value) => self.expr(value),
            HirStmt::Return { value, .. } => {
                let src = if let Some(value) = value {
                    self.expr(value)?
                } else {
                    let src = self.temp();
                    self.emit(RegInstr::LoadUnit { dst: src });
                    src
                };
                self.emit_all_cleanup();
                self.emit(RegInstr::Return { src });
                Ok(src)
            }
            other => Err(EvalError::Runtime(format!(
                "reg VM match expression arm must end with an expression, got `{other:?}`."
            ))),
        }
    }

    fn condition_jump(
        &mut self,
        condition: &HirExpr,
        expected: bool,
        target: usize,
    ) -> Result<usize, EvalError> {
        if let HirExpr::Binary {
            op, left, right, ..
        } = condition
        {
            if let Some(op) = int_compare_op(*op) {
                let lhs = self.expr(left)?;
                let rhs = self.expr(right)?;
                return Ok(self.emit(RegInstr::JumpIfIntCompare {
                    lhs,
                    rhs,
                    op,
                    expected,
                    target,
                }));
            }
        }
        let cond = self.expr(condition)?;
        Ok(self.emit(RegInstr::JumpIfBool {
            cond,
            expected,
            target,
        }))
    }

    fn statement(&mut self, statement: &HirStmt) -> Result<(), EvalError> {
        match statement {
            HirStmt::Let { name, value, .. } => {
                let dst = self.local(name);
                if let Some(value) = value {
                    let src = self.expr(value)?;
                    self.emit(RegInstr::Move { dst, src });
                } else {
                    self.emit(RegInstr::LoadUnit { dst });
                }
            }
            HirStmt::Assign { target, value, .. } => match target {
                HirExpr::Ident { name, .. } => {
                    let dst = self.lookup_local(name)?;
                    let src = self.expr(value)?;
                    self.emit(RegInstr::Move { dst, src });
                }
                HirExpr::Index { base, index, .. } => {
                    let HirExpr::Ident { name, .. } = base.as_ref() else {
                        return Err(EvalError::Runtime(
                            "reg VM only supports index assignment rooted at local variables."
                                .to_string(),
                        ));
                    };
                    let value = self.expr(value)?;
                    let list = self.lookup_local(name)?;
                    let index = self.expr(index)?;
                    let dst = self.temp();
                    self.emit(RegInstr::ListSet {
                        dst,
                        list,
                        index,
                        value,
                    });
                }
                _ => {
                    return Err(EvalError::Runtime(
                        "reg VM only supports assignment to local variables.".to_string(),
                    ));
                }
            },
            HirStmt::Return { value, .. } => {
                let src = if let Some(value) = value {
                    self.expr(value)?
                } else {
                    let src = self.temp();
                    self.emit(RegInstr::LoadUnit { dst: src });
                    src
                };
                self.emit_all_cleanup();
                self.emit(RegInstr::Return { src });
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                let src = self.expr(resource)?;
                let dst = self.local(binding);
                self.emit(RegInstr::Move { dst, src });
                self.cleanup_stack.push(dst);
                self.block(body)?;
                self.cleanup_stack
                    .pop()
                    .expect("with cleanup stack should contain binding");
                self.emit(RegInstr::ResourceDrop { resource: dst });
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                let else_jump = self.condition_jump(condition, false, usize::MAX)?;
                self.block(then_body)?;
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let else_ip = self.function.code.len();
                self.patch_jump(else_jump, else_ip);
                if let Some(else_body) = else_body {
                    self.block(else_body)?;
                }
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                let start_ip = self.function.code.len();
                let exit_jump = if let Some(condition) = condition {
                    Some(self.condition_jump(condition, false, usize::MAX)?)
                } else {
                    None
                };
                self.loop_stack.push(LoopPatch {
                    cleanup_base: self.cleanup_stack.len(),
                    ..LoopPatch::default()
                });
                self.block(body)?;
                let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                self.emit(RegInstr::Jump { target: start_ip });
                let exit_ip = self.function.code.len();
                if let Some(exit_jump) = exit_jump {
                    self.patch_jump(exit_jump, exit_ip);
                }
                self.patch_many(loop_patch.breaks, exit_ip);
                self.patch_many(loop_patch.continues, start_ip);
            }
            HirStmt::For {
                binding,
                iterable,
                is_async,
                body,
                ..
            } => {
                if *is_async {
                    let stream = self.expr(iterable)?;
                    let start_ip = self.function.code.len();
                    let next_result = self.temp();
                    self.emit(RegInstr::CallIntrinsic {
                        dst: next_result,
                        intrinsic: RegIntrinsic::StreamNext,
                        args: vec![stream],
                    });
                    let next_option = self.temp();
                    self.emit(RegInstr::TryResult {
                        dst: next_option,
                        src: next_result,
                        cleanup: self.all_cleanup_regs(),
                    });
                    let match_ip = self.emit(RegInstr::MatchOption {
                        src: next_option,
                        some_ip: usize::MAX,
                        none_ip: usize::MAX,
                    });
                    let some_ip = self.function.code.len();
                    self.patch_jump(match_ip, some_ip);
                    let item = self.local(binding);
                    self.emit(RegInstr::UnwrapSome {
                        dst: item,
                        src: next_option,
                    });

                    self.loop_stack.push(LoopPatch {
                        cleanup_base: self.cleanup_stack.len(),
                        ..LoopPatch::default()
                    });
                    self.block(body)?;
                    let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                    self.emit(RegInstr::Jump { target: start_ip });
                    let exit_ip = self.function.code.len();
                    self.patch_match_none(match_ip, exit_ip);
                    self.patch_many(loop_patch.breaks, exit_ip);
                    self.patch_many(loop_patch.continues, start_ip);
                    return Ok(());
                }
                let list = self.expr(iterable)?;
                let index = self.temp();
                self.emit(RegInstr::LoadInt {
                    dst: index,
                    value: 0,
                });
                let len = self.temp();
                self.emit(RegInstr::ListLen { dst: len, list });
                let one = self.temp();
                self.emit(RegInstr::LoadInt { dst: one, value: 1 });

                let start_ip = self.function.code.len();
                let exit_jump = self.emit(RegInstr::JumpIfIntCompare {
                    lhs: index,
                    rhs: len,
                    op: RegIntCompare::Less,
                    expected: false,
                    target: usize::MAX,
                });
                let item = self.local(binding);
                self.emit(RegInstr::ListGet {
                    dst: item,
                    list,
                    index,
                });

                self.loop_stack.push(LoopPatch {
                    cleanup_base: self.cleanup_stack.len(),
                    ..LoopPatch::default()
                });
                self.block(body)?;
                let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                let continue_ip = self.function.code.len();
                self.emit(RegInstr::AddInt {
                    dst: index,
                    lhs: index,
                    rhs: one,
                });
                self.emit(RegInstr::Jump { target: start_ip });
                let exit_ip = self.function.code.len();
                self.patch_jump(exit_jump, exit_ip);
                self.patch_many(loop_patch.breaks, exit_ip);
                self.patch_many(loop_patch.continues, continue_ip);
            }
            HirStmt::Match { value, arms, .. } => {
                if !self.map_get_match(value, arms)?
                    && !self.variant_match(value, arms)?
                    && !self.struct_match(value, arms)?
                {
                    return Err(EvalError::Runtime(
                        "reg VM v0 does not support this match pattern.".to_string(),
                    ));
                }
            }
            HirStmt::Select { arms, .. } => {
                if arms.len() > 1
                    && arms
                        .iter()
                        .any(|arm| self.select_operation_may_suspend(&arm.operation))
                {
                    return Err(EvalError::Runtime(
                        "reg VM select with pending multi-arm operations requires first-ready semantics and is not supported yet.".to_string(),
                    ));
                }
                if let Some(arm) = arms.first() {
                    let src = self.expr(&arm.operation)?;
                    if arm.binding != "_" {
                        let dst = self.local(&arm.binding);
                        self.emit(RegInstr::Move { dst, src });
                    }
                    self.block(&arm.body)?;
                }
            }
            HirStmt::Break(_) => {
                if self.loop_stack.is_empty() {
                    return Err(EvalError::Runtime(
                        "reg VM break used outside of a loop.".to_string(),
                    ));
                }
                let cleanup_base = self
                    .loop_stack
                    .last()
                    .expect("loop patch should exist")
                    .cleanup_base;
                self.emit_cleanup_since(cleanup_base);
                let jump = self.emit(RegInstr::Jump { target: usize::MAX });
                self.loop_stack
                    .last_mut()
                    .expect("loop patch should exist")
                    .breaks
                    .push(jump);
            }
            HirStmt::Continue(_) => {
                if self.loop_stack.is_empty() {
                    return Err(EvalError::Runtime(
                        "reg VM continue used outside of a loop.".to_string(),
                    ));
                }
                let cleanup_base = self
                    .loop_stack
                    .last()
                    .expect("loop patch should exist")
                    .cleanup_base;
                self.emit_cleanup_since(cleanup_base);
                let jump = self.emit(RegInstr::Jump { target: usize::MAX });
                self.loop_stack
                    .last_mut()
                    .expect("loop patch should exist")
                    .continues
                    .push(jump);
            }
            HirStmt::Expr(expr) => {
                self.expr(expr)?;
            }
            unsupported => Err(EvalError::Runtime(format!(
                "reg VM v0 does not support statement `{unsupported:?}`."
            )))?,
        }
        Ok(())
    }

    fn select_operation_may_suspend(&self, operation: &HirExpr) -> bool {
        let mut seen = HashSet::new();
        self.expr_may_suspend(operation, &mut seen)
    }

    fn block_may_suspend(&self, block: &HirBlock, seen: &mut HashSet<String>) -> bool {
        block
            .statements
            .iter()
            .any(|statement| self.stmt_may_suspend(statement, seen))
    }

    fn stmt_may_suspend(&self, statement: &HirStmt, seen: &mut HashSet<String>) -> bool {
        match statement {
            HirStmt::Let {
                value, is_async, ..
            } => {
                *is_async
                    || value
                        .as_ref()
                        .is_some_and(|value| self.expr_may_suspend(value, seen))
            }
            HirStmt::Return { value, .. } => value
                .as_ref()
                .is_some_and(|value| self.expr_may_suspend(value, seen)),
            HirStmt::Expr(value) => self.expr_may_suspend(value, seen),
            HirStmt::With { resource, body, .. } => {
                self.expr_may_suspend(resource, seen) || self.block_may_suspend(body, seen)
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.expr_may_suspend(condition, seen)
                    || self.block_may_suspend(then_body, seen)
                    || else_body
                        .as_ref()
                        .is_some_and(|body| self.block_may_suspend(body, seen))
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                condition
                    .as_ref()
                    .is_some_and(|condition| self.expr_may_suspend(condition, seen))
                    || self.block_may_suspend(body, seen)
            }
            HirStmt::For {
                iterable,
                is_async,
                body,
                ..
            } => {
                *is_async
                    || self.expr_may_suspend(iterable, seen)
                    || self.block_may_suspend(body, seen)
            }
            HirStmt::Match { value, arms, .. } => {
                self.expr_may_suspend(value, seen)
                    || arms.iter().any(|arm| {
                        arm.guard
                            .as_ref()
                            .is_some_and(|guard| self.expr_may_suspend(guard, seen))
                            || self.block_may_suspend(&arm.body, seen)
                    })
            }
            HirStmt::Select { .. } => true,
            HirStmt::Assign { target, value, .. } => {
                self.expr_may_suspend(target, seen) || self.expr_may_suspend(value, seen)
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => false,
        }
    }

    fn expr_may_suspend(&self, expr: &HirExpr, seen: &mut HashSet<String>) -> bool {
        match expr {
            HirExpr::Await { value, .. }
            | HirExpr::Effect { value, .. }
            | HirExpr::Manage { value, .. }
            | HirExpr::Try { value, .. } => self.expr_may_suspend(value, seen),
            HirExpr::Spawn { .. } => true,
            HirExpr::Call {
                callee,
                receiver,
                args,
                ..
            } => {
                if receiver
                    .as_ref()
                    .is_some_and(|receiver| self.expr_may_suspend(&receiver.value, seen))
                    || args
                        .iter()
                        .any(|arg| self.expr_may_suspend(&arg.value, seen))
                {
                    return true;
                }
                let (namespace, name) = match callee {
                    Callee::Name(name) => (None, type_root_name(name)),
                    Callee::Qualified { namespace, name } => {
                        (Some(type_root_name(namespace)), type_root_name(name))
                    }
                    Callee::ReceiverCall { method, .. } => (None, type_root_name(method)),
                };
                let Some(signature) = self.hir.resolve_function(namespace, name) else {
                    return false;
                };
                if !signature.is_async {
                    return false;
                }
                if signature.is_native || signature.is_builtin {
                    return true;
                }
                let body_name = signature
                    .namespace
                    .as_ref()
                    .map(|namespace| format!("{namespace}.{}", signature.name))
                    .unwrap_or_else(|| signature.name.clone());
                if !seen.insert(body_name.clone()) {
                    return true;
                }
                let may_suspend = self
                    .hir
                    .function_body(&body_name)
                    .and_then(|body| body.block.as_ref())
                    .is_none_or(|block| self.block_may_suspend(block, seen));
                seen.remove(&body_name);
                may_suspend
            }
            HirExpr::Binary { left, right, .. }
            | HirExpr::Index {
                base: left,
                index: right,
                ..
            } => self.expr_may_suspend(left, seen) || self.expr_may_suspend(right, seen),
            HirExpr::Field { base, .. } => self.expr_may_suspend(base, seen),
            HirExpr::ObjectLiteral { fields, .. } => fields
                .iter()
                .any(|field| self.expr_may_suspend(&field.value, seen)),
            HirExpr::MapLiteral { entries, .. } => entries.iter().any(|entry| {
                self.expr_may_suspend(&entry.key, seen) || self.expr_may_suspend(&entry.value, seen)
            }),
            HirExpr::ArrayLiteral { items, .. } => {
                items.iter().any(|item| self.expr_may_suspend(item, seen))
            }
            HirExpr::Closure { .. } => false,
            HirExpr::Match { value, arms, .. } => {
                self.expr_may_suspend(value, seen)
                    || arms.iter().any(|arm| {
                        arm.guard
                            .as_ref()
                            .is_some_and(|guard| self.expr_may_suspend(guard, seen))
                            || self.block_may_suspend(&arm.body, seen)
                    })
            }
            HirExpr::Ident { .. }
            | HirExpr::Number { .. }
            | HirExpr::String { .. }
            | HirExpr::Unknown(_) => false,
        }
    }

    fn logical_binary(
        &mut self,
        op: BinaryOp,
        left: &HirExpr,
        right: &HirExpr,
    ) -> Result<Reg, EvalError> {
        let lhs = self.expr(left)?;
        let dst = self.temp();
        match op {
            BinaryOp::LogicalAnd => {
                let short_circuit = self.emit(RegInstr::JumpIfBool {
                    cond: lhs,
                    expected: false,
                    target: usize::MAX,
                });
                let rhs = self.expr(right)?;
                self.emit(RegInstr::Move { dst, src: rhs });
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let false_ip = self.function.code.len();
                self.patch_jump(short_circuit, false_ip);
                self.emit(RegInstr::LoadBool { dst, value: false });
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            BinaryOp::LogicalOr => {
                let short_circuit = self.emit(RegInstr::JumpIfBool {
                    cond: lhs,
                    expected: true,
                    target: usize::MAX,
                });
                let rhs = self.expr(right)?;
                self.emit(RegInstr::Move { dst, src: rhs });
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let true_ip = self.function.code.len();
                self.patch_jump(short_circuit, true_ip);
                self.emit(RegInstr::LoadBool { dst, value: true });
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            _ => unreachable!(),
        }
        Ok(dst)
    }

    fn expr(&mut self, expr: &HirExpr) -> Result<Reg, EvalError> {
        match expr {
            HirExpr::Ident { name, .. } if name == "Unit" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadUnit { dst });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if name == "None" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadNone { dst });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if name == "true" || name == "false" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadBool {
                    dst,
                    value: name == "true",
                });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if self.hir.sum_type_for_variant(name).is_some() => {
                let fields = self.hir.sum_variant_fields(name).unwrap_or(&[]);
                if !fields.is_empty() {
                    return Err(EvalError::Runtime(format!(
                        "reg VM variant `{name}` requires {} field(s).",
                        fields.len()
                    )));
                }
                let dst = self.temp();
                self.emit(RegInstr::MakeVariant {
                    dst,
                    name: name.clone(),
                    fields: Vec::new(),
                });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } => self.lookup_local(name),
            HirExpr::Number { value, .. } => {
                let dst = self.temp();
                if value.contains('.') {
                    let value = value.parse::<f64>().map_err(|error| {
                        EvalError::Runtime(format!("invalid reg VM float `{value}`: {error}"))
                    })?;
                    self.emit(RegInstr::LoadFloat { dst, value });
                } else {
                    let value = value.parse::<i64>().map_err(|error| {
                        EvalError::Runtime(format!("invalid reg VM integer `{value}`: {error}"))
                    })?;
                    self.emit(RegInstr::LoadInt { dst, value });
                }
                Ok(dst)
            }
            HirExpr::String { value, .. } => {
                let dst = self.temp();
                self.emit(RegInstr::LoadString {
                    dst,
                    value: Rc::new(decode_string_token(value)),
                });
                Ok(dst)
            }
            HirExpr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.expr(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeList { dst, items });
                Ok(dst)
            }
            HirExpr::ObjectLiteral { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|field| Ok((field.name.clone(), self.expr(&field.value)?)))
                    .collect::<Result<Vec<_>, EvalError>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeObject { dst, fields });
                Ok(dst)
            }
            HirExpr::MapLiteral { entries, .. } => {
                let entries = entries
                    .iter()
                    .map(|entry| Ok((self.expr(&entry.key)?, self.expr(&entry.value)?)))
                    .collect::<Result<Vec<_>, EvalError>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeMap { dst, entries });
                Ok(dst)
            }
            HirExpr::Binary {
                op, left, right, ..
            } => {
                if matches!(op, BinaryOp::LogicalAnd | BinaryOp::LogicalOr) {
                    return self.logical_binary(*op, left, right);
                }
                let lhs = self.expr(left)?;
                let rhs = self.expr(right)?;
                let dst = self.temp();
                let instr = match op {
                    BinaryOp::Add => RegInstr::AddInt { dst, lhs, rhs },
                    BinaryOp::Subtract => RegInstr::SubInt { dst, lhs, rhs },
                    BinaryOp::Multiply => RegInstr::MulInt { dst, lhs, rhs },
                    BinaryOp::Divide => RegInstr::DivInt { dst, lhs, rhs },
                    BinaryOp::Modulo => RegInstr::ModInt { dst, lhs, rhs },
                    BinaryOp::BitAnd => RegInstr::BitAndInt { dst, lhs, rhs },
                    BinaryOp::BitOr => RegInstr::BitOrInt { dst, lhs, rhs },
                    BinaryOp::BitXor => RegInstr::BitXorInt { dst, lhs, rhs },
                    BinaryOp::ShiftLeft => RegInstr::ShiftLeftInt { dst, lhs, rhs },
                    BinaryOp::ShiftRight => RegInstr::ShiftRightInt { dst, lhs, rhs },
                    BinaryOp::Less => RegInstr::LessInt { dst, lhs, rhs },
                    BinaryOp::LessEqual => RegInstr::LessEqualInt { dst, lhs, rhs },
                    BinaryOp::Greater => RegInstr::GreaterInt { dst, lhs, rhs },
                    BinaryOp::GreaterEqual => RegInstr::GreaterEqualInt { dst, lhs, rhs },
                    BinaryOp::Equal => RegInstr::Equal { dst, lhs, rhs },
                    BinaryOp::NotEqual => RegInstr::NotEqual { dst, lhs, rhs },
                    BinaryOp::LogicalAnd | BinaryOp::LogicalOr => unreachable!(),
                };
                self.emit(instr);
                Ok(dst)
            }
            HirExpr::Field { base, name, .. } => {
                let base = self.expr(base)?;
                let dst = self.temp();
                self.emit(RegInstr::GetField {
                    dst,
                    base,
                    name: name.clone(),
                });
                Ok(dst)
            }
            HirExpr::Index { base, index, .. } => {
                let list = self.expr(base)?;
                let index = self.expr(index)?;
                let dst = self.temp();
                self.emit(RegInstr::ListGet { dst, list, index });
                Ok(dst)
            }
            HirExpr::Await { value, .. } | HirExpr::Effect { value, .. } => self.expr(value),
            HirExpr::Spawn { .. } => Err(EvalError::Runtime(
                "reg VM does not support spawn expressions.".to_string(),
            )),
            HirExpr::Manage { value, .. } => {
                let src = self.expr(value)?;
                let dst = self.temp();
                self.emit(RegInstr::Manage { dst, src });
                Ok(dst)
            }
            HirExpr::Try { value, .. } => {
                let src = self.expr(value)?;
                let dst = self.temp();
                self.emit(RegInstr::TryResult {
                    dst,
                    src,
                    cleanup: self.all_cleanup_regs(),
                });
                Ok(dst)
            }
            HirExpr::Call {
                callee,
                args,
                receiver,
                ..
            } => self.call(callee, receiver.as_ref(), args),
            HirExpr::Closure {
                params,
                captures,
                body,
                ..
            } => {
                let capture_names =
                    closure_capture_names(body, params, captures, &self.function.local_regs);
                let capture_regs = capture_names
                    .iter()
                    .map(|capture| self.lookup_local(capture))
                    .collect::<Result<Vec<_>, _>>()?;
                let function_id = self.functions.len();
                self.functions
                    .push(RegFunction::placeholder(format!("<closure:{function_id}>")));
                let closure_function = {
                    let mut lowerer = RegLowerer {
                        hir: self.hir,
                        function_ids: self.function_ids,
                        functions: &mut *self.functions,
                        function: RegFunction {
                            name: format!("<closure:{function_id}>"),
                            params: params.len(),
                            captures: capture_names.len(),
                            regs: 0,
                            local_regs: HashMap::new(),
                            code: Vec::new(),
                        },
                        loop_stack: Vec::new(),
                        cleanup_stack: Vec::new(),
                    };
                    for capture in &capture_names {
                        lowerer.local(capture);
                    }
                    for param in params {
                        lowerer.local(param);
                    }
                    lowerer.block(body)?;
                    let unit = lowerer.temp();
                    lowerer.emit(RegInstr::LoadUnit { dst: unit });
                    lowerer.emit(RegInstr::Return { src: unit });
                    lowerer.function
                };
                self.functions[function_id] = closure_function;
                let dst = self.temp();
                self.emit(RegInstr::MakeClosure {
                    dst,
                    function: function_id,
                    captures: capture_regs,
                });
                Ok(dst)
            }
            HirExpr::Match { value, arms, .. } => self.match_expr(value, arms),
            unsupported => Err(EvalError::Runtime(format!(
                "reg VM v0 does not support expression `{unsupported:?}`."
            )))?,
        }
    }

    fn call(
        &mut self,
        callee: &Callee,
        receiver: Option<&HirCallReceiver>,
        args: &[HirCallArg],
    ) -> Result<Reg, EvalError> {
        if let Callee::ReceiverCall { method, .. } = callee {
            let dst = self.temp();
            let Some(receiver) = receiver else {
                return Err(EvalError::Runtime(format!(
                    "reg VM receiver call `{method}` is missing HIR receiver metadata."
                )));
            };
            let namespace = receiver
                .resolved_namespace
                .as_deref()
                .or(receiver.type_name.as_deref());
            if let Some(namespace) = namespace
                && self.is_native_function(Some(namespace), method)
            {
                let receiver_reg = self.expr(&receiver.value)?;
                let arg_regs = args
                    .iter()
                    .map(|arg| self.expr(&arg.value))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut native_args = Vec::with_capacity(arg_regs.len() + 1);
                native_args.push(receiver_reg);
                native_args.extend(arg_regs);
                self.emit(RegInstr::CallNative {
                    dst,
                    key: format!("{}.{}", type_root_name(namespace), type_root_name(method)),
                    args: native_args,
                });
                return Ok(dst);
            }
            let Some(namespace) = namespace else {
                return Err(EvalError::Runtime(format!(
                    "reg VM receiver call `{method}` is missing receiver type metadata."
                )));
            };
            let receiver_reg = self.expr(&receiver.value)?;
            let arg_regs = args
                .iter()
                .map(|arg| self.expr(&arg.value))
                .collect::<Result<Vec<_>, _>>()?;
            match (type_root_name(namespace), type_root_name(method)) {
                ("Float", "to_string") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::FloatToString,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("Float", "is_finite") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::FloatIsFinite,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("Float", "is_infinite") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::FloatIsInfinite,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("Float", "is_nan") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::FloatIsNan,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("Int", "to_string") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::IntToString,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("String", "concat") => {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM String.concat receiver call expected 1 arg, got {}.",
                            arg_regs.len()
                        )));
                    }
                    self.emit(RegInstr::StringConcat {
                        dst,
                        left: receiver_reg,
                        right: arg_regs[0],
                    });
                    return Ok(dst);
                }
                ("String", "env") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::EnvGet,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("String", "env_or") => {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM String.env_or receiver call expected 1 arg, got {}.",
                            arg_regs.len()
                        )));
                    }
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::EnvGetOrDefault,
                        args: vec![receiver_reg, arg_regs[0]],
                    });
                    return Ok(dst);
                }
                ("String", "is_empty") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::StringIsEmpty,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("String", "len") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::StringLen,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("String", "safe_relative") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::PathSafeRelative,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("String", "to_path") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::PathFromString,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("String", "to_url") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::UrlFromString,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("List", "first") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::ListFirst,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("List", "get") => {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM List.get receiver call expected 1 arg, got {}.",
                            arg_regs.len()
                        )));
                    }
                    self.emit(RegInstr::ListGet {
                        dst,
                        list: receiver_reg,
                        index: arg_regs[0],
                    });
                    return Ok(dst);
                }
                ("List", "is_empty") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::ListIsEmpty,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("List", "last") => {
                    self.emit(RegInstr::CallIntrinsic {
                        dst,
                        intrinsic: RegIntrinsic::ListLast,
                        args: vec![receiver_reg],
                    });
                    return Ok(dst);
                }
                ("List", "len") => {
                    self.emit(RegInstr::ListLen {
                        dst,
                        list: receiver_reg,
                    });
                    return Ok(dst);
                }
                _ => {}
            }
            return Err(EvalError::Runtime(
                "reg VM v0 does not support receiver calls.".to_string(),
            ));
        }

        let arg_regs = args
            .iter()
            .map(|arg| self.expr(&arg.value))
            .collect::<Result<Vec<_>, _>>()?;
        let dst = self.temp();
        match callee {
            Callee::Name(name) => {
                if let Some(function) = self.function_ids.get(name).copied() {
                    self.emit(RegInstr::CallKnown {
                        dst,
                        function,
                        args: arg_regs,
                    });
                } else if self.is_native_function(None, name) {
                    self.emit(RegInstr::CallNative {
                        dst,
                        key: type_root_name(name).to_string(),
                        args: arg_regs,
                    });
                } else if type_root_name(name) == "Some" {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Option variant `Some` expected 1 payload, got {}.",
                            arg_regs.len()
                        )));
                    }
                    self.emit(RegInstr::MakeSome {
                        dst,
                        value: arg_regs[0],
                    });
                } else if matches!(type_root_name(name), "Ok" | "Err") {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Result variant `{}` expected 1 payload, got {}.",
                            type_root_name(name),
                            arg_regs.len()
                        )));
                    }
                    self.emit(RegInstr::MakeVariant {
                        dst,
                        name: type_root_name(name).to_string(),
                        fields: vec![("value".to_string(), arg_regs[0])],
                    });
                } else if self
                    .hir
                    .sum_type_for_variant(type_root_name(name))
                    .is_some()
                {
                    let variant_name = type_root_name(name);
                    let fields = self.hir.sum_variant_fields(variant_name).unwrap_or(&[]);
                    match fields.len() {
                        0 if arg_regs.is_empty() => {
                            self.emit(RegInstr::MakeVariant {
                                dst,
                                name: variant_name.to_string(),
                                fields: Vec::new(),
                            });
                        }
                        1 if arg_regs.len() == 1 => {
                            self.emit(RegInstr::MakeVariant {
                                dst,
                                name: variant_name.to_string(),
                                fields: vec![(fields[0].name.clone(), arg_regs[0])],
                            });
                        }
                        field_count if field_count == arg_regs.len() => {
                            let fields = args
                                .iter()
                                .zip(arg_regs.into_iter())
                                .enumerate()
                                .map(|(index, (arg, reg))| {
                                    let name = arg
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| fields[index].name.clone());
                                    (name, reg)
                                })
                                .collect::<Vec<_>>();
                            self.emit(RegInstr::MakeVariant {
                                dst,
                                name: variant_name.to_string(),
                                fields,
                            });
                        }
                        field_count => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM variant `{variant_name}` expected {field_count} field(s), got {}.",
                                arg_regs.len()
                            )));
                        }
                    }
                } else {
                    let fields = args
                        .iter()
                        .zip(arg_regs.into_iter())
                        .map(|(arg, reg)| {
                            arg.name.clone().map(|name| (name, reg)).ok_or_else(|| {
                                EvalError::Runtime(
                                    "reg VM v0 struct constructors require named fields."
                                        .to_string(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    self.emit(RegInstr::MakeStruct {
                        dst,
                        name: type_root_name(name).to_string(),
                        fields,
                    });
                }
            }
            Callee::Qualified { namespace, name } => {
                let namespace_root = type_root_name(namespace);
                let name_root = type_root_name(name);
                let intrinsic = match (namespace_root, name_root) {
                    ("Args", "all") => RegIntrinsic::ArgsAll,
                    ("Args", "count") => RegIntrinsic::ArgsCount,
                    ("Args", "get") => RegIntrinsic::ArgsGet,
                    ("Args", "get_or_default") => RegIntrinsic::ArgsGetOrDefault,
                    ("Assert", "equal") => RegIntrinsic::AssertEqual,
                    ("Assert", "equal_bool") => RegIntrinsic::AssertEqualBool,
                    ("Assert", "equal_int") => RegIntrinsic::AssertEqualInt,
                    ("Base64", "decode") => RegIntrinsic::Base64Decode,
                    ("Base64", "decode_string") => RegIntrinsic::Base64DecodeString,
                    ("Base64", "encode") => RegIntrinsic::Base64Encode,
                    ("Base64", "encode_bytes") => RegIntrinsic::Base64EncodeBytes,
                    ("Bytes", "concat") => RegIntrinsic::BytesConcat,
                    ("Bytes", "consume") => RegIntrinsic::BytesConsume,
                    ("Bytes", "from_buffer") => RegIntrinsic::BytesViewToBytes,
                    ("Bytes", "from_string") => RegIntrinsic::BytesFromString,
                    ("Bytes", "is_empty") => RegIntrinsic::BytesIsEmpty,
                    ("Bytes", "len") => RegIntrinsic::BytesLen,
                    ("Bytes", "slice") | ("Bytes", "view") => RegIntrinsic::BytesSlice,
                    ("Buffer", "clear") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Buffer.clear expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::BufferClear {
                            dst,
                            buffer: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("Buffer", "consume") => RegIntrinsic::BytesConsume,
                    ("Buffer", "is_empty") => RegIntrinsic::BytesIsEmpty,
                    ("Buffer", "len") => RegIntrinsic::BytesLen,
                    ("Buffer", "new") => RegIntrinsic::BufferNew,
                    ("Buffer", "view") => RegIntrinsic::BytesSlice,
                    ("BufferView", "is_empty") => RegIntrinsic::BytesIsEmpty,
                    ("BufferView", "len") => RegIntrinsic::BytesLen,
                    ("BufferView", "slice") => RegIntrinsic::BytesSlice,
                    ("BufferView", "to_bytes") => RegIntrinsic::BytesViewToBytes,
                    ("BytesView", "is_empty") => RegIntrinsic::BytesIsEmpty,
                    ("BytesView", "len") => RegIntrinsic::BytesLen,
                    ("BytesView", "slice") => RegIntrinsic::BytesSlice,
                    ("BytesView", "starts_with") => RegIntrinsic::BytesViewStartsWith,
                    ("BytesView", "to_bytes") => RegIntrinsic::BytesViewToBytes,
                    ("Cache", "insert") => {
                        if arg_regs.len() != 3 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Cache.insert expected 3 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::MapInsert {
                            dst,
                            map: arg_regs[0],
                            key: arg_regs[1],
                            value: arg_regs[2],
                        });
                        return Ok(dst);
                    }
                    ("Cache", "get") => RegIntrinsic::CacheGet,
                    ("Cache", "lookup") => RegIntrinsic::CacheLookup,
                    ("Cache", "new") => RegIntrinsic::MapNew,
                    ("CancellationSource", "cancel") => RegIntrinsic::CancellationSourceCancel,
                    ("CancellationSource", "new") => RegIntrinsic::CancellationSourceNew,
                    ("CancellationSource", "token") => RegIntrinsic::CancellationSourceToken,
                    ("CancellationToken", "is_cancelled") => {
                        RegIntrinsic::CancellationTokenIsCancelled
                    }
                    ("Channel", "bounded") => RegIntrinsic::ChannelBounded,
                    ("Channel", "receiver") => RegIntrinsic::ChannelReceiver,
                    ("Channel", "sender") => RegIntrinsic::ChannelSender,
                    ("ChannelError", "message") => RegIntrinsic::ChannelErrorMessage,
                    ("Char", "compare") => RegIntrinsic::CharCompare,
                    ("Char", "from_code") => RegIntrinsic::CharFromCode,
                    ("Char", "is_alphanumeric") => RegIntrinsic::CharIsAlphanumeric,
                    ("Char", "is_alpha") => RegIntrinsic::CharIsAlpha,
                    ("Char", "is_digit") => RegIntrinsic::CharIsDigit,
                    ("Char", "is_lower") => RegIntrinsic::CharIsLower,
                    ("Char", "is_upper") => RegIntrinsic::CharIsUpper,
                    ("Char", "is_whitespace") => RegIntrinsic::CharIsWhitespace,
                    ("Char", "to_code") => RegIntrinsic::CharToCode,
                    ("Char", "to_lower") => RegIntrinsic::CharToLower,
                    ("Char", "to_string") => RegIntrinsic::CharToString,
                    ("Char", "to_upper") => RegIntrinsic::CharToUpper,
                    ("Clock", "now") => RegIntrinsic::ClockNow,
                    ("Clock", "system_unix_ms") => RegIntrinsic::ClockSystemUnixMs,
                    ("Config", "load") => RegIntrinsic::ConfigLoad,
                    ("Capability", "from") => RegIntrinsic::CapabilityFrom,
                    ("Config", "name") => RegIntrinsic::ConfigName,
                    ("Config", "new") => RegIntrinsic::ConfigNew,
                    ("Config", "rule_count") => RegIntrinsic::ConfigRuleCount,
                    ("ConfigStore", "name") => RegIntrinsic::ConfigStoreName,
                    ("ConfigStore", "new") => RegIntrinsic::ConfigStoreNew,
                    ("ConfigStore", "replace") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM ConfigStore.replace expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ConfigStoreReplace {
                            dst,
                            store: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Counter", "add") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Counter.add expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::CounterAdd {
                            dst,
                            counter: arg_regs[0],
                            amount: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Counter", "new") => RegIntrinsic::CounterNew,
                    ("Counter", "value") => RegIntrinsic::CounterValue,
                    ("Csv", "open_read") => RegIntrinsic::CsvOpenRead,
                    ("Csv", "parse_row") => RegIntrinsic::CsvParseRow,
                    ("Csv", "read_into") => RegIntrinsic::CsvReadInto,
                    ("Csv", "rows") => RegIntrinsic::CsvRows,
                    ("Deadline", "after") => RegIntrinsic::DeadlineAfter,
                    ("Deadline", "after_ms") => RegIntrinsic::DeadlineAfterMs,
                    ("Deadline", "is_expired") => RegIntrinsic::DeadlineIsExpired,
                    ("Deadline", "remaining_ms") => RegIntrinsic::DeadlineRemainingMs,
                    ("DecodeError", "message") => RegIntrinsic::DecodeErrorMessage,
                    ("Deque", "clear") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Deque.clear expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::DequeClear {
                            dst,
                            deque: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("Deque", "is_empty") => RegIntrinsic::DequeIsEmpty,
                    ("Deque", "len") => RegIntrinsic::DequeLen,
                    ("Deque", "new") => RegIntrinsic::DequeNew,
                    ("Deque", "pop_back") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Deque.pop_back expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::DequePopBack {
                            dst,
                            deque: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("Deque", "pop_front") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Deque.pop_front expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::DequePopFront {
                            dst,
                            deque: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("Deque", "push_back") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Deque.push_back expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::DequePushBack {
                            dst,
                            deque: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Deque", "push_front") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Deque.push_front expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::DequePushFront {
                            dst,
                            deque: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Deque", "to_list") => RegIntrinsic::DequeToList,
                    ("Diff", "unified") => RegIntrinsic::DiffUnified,
                    ("Directory", "copy_file") => RegIntrinsic::DirectoryCopyFile,
                    ("Directory", "create") => RegIntrinsic::DirectoryCreate,
                    ("Directory", "create_all") => RegIntrinsic::DirectoryCreateAll,
                    ("Directory", "create_dir_all") => RegIntrinsic::DirectoryCreateDirAll,
                    ("Directory", "exists") => RegIntrinsic::DirectoryExists,
                    ("Directory", "is_dir") => RegIntrinsic::DirectoryIsDir,
                    ("Directory", "is_file") => RegIntrinsic::DirectoryIsFile,
                    ("Directory", "list_files") => RegIntrinsic::DirectoryListFiles,
                    ("Directory", "list_paths") => RegIntrinsic::DirectoryListPaths,
                    ("Directory", "metadata") => RegIntrinsic::DirectoryMetadata,
                    ("Directory", "read_string") => RegIntrinsic::DirectoryReadString,
                    ("Directory", "remove_dir_all") => RegIntrinsic::DirectoryRemoveDirAll,
                    ("Directory", "remove_file") => RegIntrinsic::DirectoryRemoveFile,
                    ("Directory", "rename") => RegIntrinsic::DirectoryRename,
                    ("Directory", "write_string") => RegIntrinsic::DirectoryWriteString,
                    ("Db", "close") => RegIntrinsic::DbClose,
                    ("DbConnection", "open") => RegIntrinsic::DbConnectionOpen,
                    ("DbConnection", "query") => RegIntrinsic::DbConnectionQuery,
                    ("DbConnection", "try_open") => RegIntrinsic::DbConnectionTryOpen,
                    ("Date", "add_days") => RegIntrinsic::DateAddDays,
                    ("Date", "add_ms") => RegIntrinsic::DateAddMs,
                    ("Date", "day") => RegIntrinsic::DateDay,
                    ("Date", "days_between") => RegIntrinsic::DateDaysBetween,
                    ("Date", "days_in_month") => RegIntrinsic::DateDaysInMonth,
                    ("Date", "format_iso") => RegIntrinsic::DateFormatIso,
                    ("Date", "format_ymd") => RegIntrinsic::DateFormatYmd,
                    ("Date", "hour") => RegIntrinsic::DateHour,
                    ("Date", "is_leap_year") => RegIntrinsic::DateIsLeapYear,
                    ("Date", "minute") => RegIntrinsic::DateMinute,
                    ("Date", "month") => RegIntrinsic::DateMonth,
                    ("Date", "parse_iso") => RegIntrinsic::DateParseIso,
                    ("Date", "parse_ymd") => RegIntrinsic::DateParseYmd,
                    ("Date", "second") => RegIntrinsic::DateSecond,
                    ("Date", "start_of_day") => RegIntrinsic::DateStartOfDay,
                    ("Date", "weekday") => RegIntrinsic::DateWeekday,
                    ("Date", "year") => RegIntrinsic::DateYear,
                    ("Duration", "add") => RegIntrinsic::DurationAdd,
                    ("Duration", "as_ms") => RegIntrinsic::DurationAsMs,
                    ("Duration", "as_seconds") => RegIntrinsic::DurationAsSeconds,
                    ("Duration", "ms") => RegIntrinsic::DurationMs,
                    ("Duration", "seconds") => RegIntrinsic::DurationSeconds,
                    ("Environment", "bind_function") => RegIntrinsic::EnvironmentBindFunction,
                    ("Environment", "child") => RegIntrinsic::EnvironmentChild,
                    ("Environment", "has_function") => RegIntrinsic::EnvironmentHasFunction,
                    ("Environment", "has_parent") => RegIntrinsic::EnvironmentHasParent,
                    ("Environment", "root") => RegIntrinsic::EnvironmentRoot,
                    ("Env", "current_dir") => RegIntrinsic::EnvCurrentDir,
                    ("Env", "get") => RegIntrinsic::EnvGet,
                    ("Env", "get_or_default") => RegIntrinsic::EnvGetOrDefault,
                    ("Env", "home_dir") => RegIntrinsic::EnvHomeDir,
                    ("Env", "run_workspace_root") => RegIntrinsic::EnvRunWorkspaceRoot,
                    ("Env", "set") => RegIntrinsic::EnvSet,
                    ("Env", "set_current_dir") => RegIntrinsic::EnvSetCurrentDir,
                    ("Env", "temp_dir") => RegIntrinsic::EnvTempDir,
                    ("File", "append_bytes") => RegIntrinsic::FileAppendBytes,
                    ("File", "append_string") => RegIntrinsic::FileAppendString,
                    ("File", "bytes_stream") => RegIntrinsic::FileBytesStream,
                    ("File", "exists") => RegIntrinsic::FileExists,
                    ("File", "open") => RegIntrinsic::FileOpen,
                    ("File", "open_read") => RegIntrinsic::FileOpenRead,
                    ("File", "open_write") => RegIntrinsic::FileOpenWrite,
                    ("File", "read_all") => RegIntrinsic::FileReadAll,
                    ("File", "read_all_async") => RegIntrinsic::FileReadAllAsync,
                    ("File", "read_all_string") => RegIntrinsic::FileReadAllString,
                    ("File", "read_all_string_async") => RegIntrinsic::FileReadAllStringAsync,
                    ("File", "read_bytes") => RegIntrinsic::FileReadBytes,
                    ("File", "read_into") => RegIntrinsic::FileReadInto,
                    ("File", "read_string") => RegIntrinsic::FileReadString,
                    ("File", "remove") => RegIntrinsic::FileRemove,
                    ("File", "write") => RegIntrinsic::FileWrite,
                    ("File", "write_async") => RegIntrinsic::FileWriteAsync,
                    ("File", "write_atomic") => RegIntrinsic::FileWriteAtomic,
                    ("File", "write_bytes") => RegIntrinsic::FileWriteBytes,
                    ("File", "write_bytes_view") => RegIntrinsic::FileWriteBytesView,
                    ("File", "write_buffer") => RegIntrinsic::FileWriteBuffer,
                    ("File", "write_buffer_view") => RegIntrinsic::FileWriteBufferView,
                    ("File", "write_string") => RegIntrinsic::FileWriteString,
                    ("File", "write_string_async") => RegIntrinsic::FileWriteStringAsync,
                    ("File", "write_string_to_path") => RegIntrinsic::FileWriteStringToPath,
                    ("FalliblePipeline", "collect") => RegIntrinsic::FalliblePipelineCollect,
                    ("FalliblePipeline", "each") => RegIntrinsic::FalliblePipelineEach,
                    ("FalliblePipeline", "filter") => RegIntrinsic::FalliblePipelineFilter,
                    ("FalliblePipeline", "map") => RegIntrinsic::FalliblePipelineMap,
                    ("FalliblePipeline", "try_map") => RegIntrinsic::FalliblePipelineTryMap,
                    ("FileError", "message") => RegIntrinsic::FileErrorMessage,
                    ("FunctionObject", "has_closure") => RegIntrinsic::FunctionObjectHasClosure,
                    ("FunctionObject", "new") => RegIntrinsic::FunctionObjectNew,
                    ("Hash", "sha256_bytes") => RegIntrinsic::HashSha256Bytes,
                    ("Hash", "sha256_file") => RegIntrinsic::HashSha256File,
                    ("Hash", "sha256_string") => RegIntrinsic::HashSha256String,
                    ("Hmac", "sha256_bytes") => RegIntrinsic::HmacSha256Bytes,
                    ("Hmac", "sha256_string") => RegIntrinsic::HmacSha256String,
                    ("GlobalConfig", "new") => RegIntrinsic::GlobalConfigNew,
                    ("GlobalConfig", "replace") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM GlobalConfig.replace expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::GlobalConfigReplace {
                            dst,
                            global: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("GlobalConfig", "rule_count") => RegIntrinsic::GlobalConfigRuleCount,
                    ("Hex", "decode") => RegIntrinsic::HexDecode,
                    ("Hex", "encode") => RegIntrinsic::HexEncode,
                    ("Hex", "encode_string") => RegIntrinsic::HexEncodeString,
                    ("HttpError", "message") => RegIntrinsic::HttpErrorMessage,
                    ("Http", "get") => RegIntrinsic::HttpGet,
                    ("Http", "get_async") => RegIntrinsic::HttpGetAsync,
                    ("Http", "get_retry_async") => RegIntrinsic::HttpGetRetryAsync,
                    ("Http", "get_timeout_async") => RegIntrinsic::HttpGetTimeoutAsync,
                    ("Http", "post_form") => RegIntrinsic::HttpPostForm,
                    ("Http", "post_form_async") => RegIntrinsic::HttpPostFormAsync,
                    ("Http", "post_json") => RegIntrinsic::HttpPostJson,
                    ("Http", "post_json_async") => RegIntrinsic::HttpPostJsonAsync,
                    ("Http", "post_json_bearer_retry_async") => {
                        RegIntrinsic::HttpPostJsonBearerRetryAsync
                    }
                    ("Http", "post_json_retry_async") => RegIntrinsic::HttpPostJsonRetryAsync,
                    ("Http", "post_json_timeout_async") => RegIntrinsic::HttpPostJsonTimeoutAsync,
                    ("Http", "send_async") => RegIntrinsic::HttpSendAsync,
                    ("HttpRequest", "json") => RegIntrinsic::HttpRequestJson,
                    ("HttpRequest", "with_header") => RegIntrinsic::HttpRequestWithHeader,
                    ("HttpRequest", "with_retry") => RegIntrinsic::HttpRequestWithRetry,
                    ("HttpRequest", "with_timeout") => RegIntrinsic::HttpRequestWithTimeout,
                    ("HttpResponse", "is_success") => RegIntrinsic::HttpResponseIsSuccess,
                    ("HttpResponse", "lines") => RegIntrinsic::HttpResponseLines,
                    ("HttpResponse", "status") => RegIntrinsic::HttpResponseStatus,
                    ("HttpResponse", "text") => RegIntrinsic::HttpResponseText,
                    ("Image", "inspect") => RegIntrinsic::ImageInspect,
                    ("Image", "load") => RegIntrinsic::ImageLoad,
                    ("Image", "normalize") => RegIntrinsic::ImageNormalize,
                    ("Image", "resize") => RegIntrinsic::ImageResize,
                    ("Image", "save") => RegIntrinsic::ImageSave,
                    ("Image", "sharpen") => RegIntrinsic::ImageSharpen,
                    ("Instant", "elapsed") => RegIntrinsic::InstantElapsed,
                    ("Float", "is_finite") => RegIntrinsic::FloatIsFinite,
                    ("Float", "is_infinite") => RegIntrinsic::FloatIsInfinite,
                    ("Float", "is_nan") => RegIntrinsic::FloatIsNan,
                    ("Float", "to_string") => RegIntrinsic::FloatToString,
                    ("Int", "bit_and") => RegIntrinsic::IntBitAnd,
                    ("Int", "bit_not") => RegIntrinsic::IntBitNot,
                    ("Int", "bit_or") => RegIntrinsic::IntBitOr,
                    ("Int", "bit_xor") => RegIntrinsic::IntBitXor,
                    ("Int", "shift_left") => RegIntrinsic::IntShiftLeft,
                    ("Int", "shift_right") => RegIntrinsic::IntShiftRight,
                    ("Int", "to_string") => RegIntrinsic::IntToString,
                    ("Math", "abs") => RegIntrinsic::MathAbs,
                    ("Math", "abs_float") => RegIntrinsic::MathAbsFloat,
                    ("Math", "ceil") => RegIntrinsic::MathCeil,
                    ("Math", "clamp") => RegIntrinsic::MathClamp,
                    ("Math", "clamp_float") => RegIntrinsic::MathClampFloat,
                    ("Math", "floor") => RegIntrinsic::MathFloor,
                    ("Math", "max") => RegIntrinsic::MathMax,
                    ("Math", "max_float") => RegIntrinsic::MathMaxFloat,
                    ("Math", "min") => RegIntrinsic::MathMin,
                    ("Math", "min_float") => RegIntrinsic::MathMinFloat,
                    ("Math", "pow") => RegIntrinsic::MathPow,
                    ("Math", "pow_float") => RegIntrinsic::MathPowFloat,
                    ("Math", "round") => RegIntrinsic::MathRound,
                    ("Math", "sqrt") => RegIntrinsic::MathSqrt,
                    ("Json", "array") => RegIntrinsic::JsonArray,
                    ("Json", "array_bools") => RegIntrinsic::JsonArrayBools,
                    ("Json", "array_contains_prefix") => RegIntrinsic::JsonArrayContainsPrefix,
                    ("Json", "array_contains_string") => RegIntrinsic::JsonArrayContainsString,
                    ("Json", "array_contains_substring") => {
                        RegIntrinsic::JsonArrayContainsSubstring
                    }
                    ("Json", "array_count_where") => RegIntrinsic::JsonArrayCountWhere,
                    ("Json", "array_fold") => RegIntrinsic::JsonArrayFold,
                    ("Json", "array_get") => RegIntrinsic::JsonArrayGet,
                    ("Json", "array_ints") => RegIntrinsic::JsonArrayInts,
                    ("Json", "array_len") => RegIntrinsic::JsonArrayLen,
                    ("Json", "array_strings") => RegIntrinsic::JsonArrayStrings,
                    ("Json", "at") | ("Json", "value_at") => RegIntrinsic::JsonAt,
                    ("Json", "at_bool") => RegIntrinsic::JsonAtBool,
                    ("Json", "at_bool_or") => RegIntrinsic::JsonAtBoolOr,
                    ("Json", "at_int") => RegIntrinsic::JsonAtInt,
                    ("Json", "at_int_or") => RegIntrinsic::JsonAtIntOr,
                    ("Json", "at_optional") => RegIntrinsic::JsonAtOptional,
                    ("Json", "at_optional_bool") => RegIntrinsic::JsonAtOptionalBool,
                    ("Json", "at_optional_int") => RegIntrinsic::JsonAtOptionalInt,
                    ("Json", "at_optional_string") => RegIntrinsic::JsonAtOptionalString,
                    ("Json", "at_or") => RegIntrinsic::JsonAtOr,
                    ("Json", "at_string") => RegIntrinsic::JsonAtString,
                    ("Json", "at_string_or") => RegIntrinsic::JsonAtStringOr,
                    ("Json", "at_to_string") => RegIntrinsic::JsonAtToString,
                    ("Json", "at_to_string_or") => RegIntrinsic::JsonAtToStringOr,
                    ("Json", "as_bool") => RegIntrinsic::JsonAsBool,
                    ("Json", "as_int") => RegIntrinsic::JsonAsInt,
                    ("Json", "as_string") => RegIntrinsic::JsonAsString,
                    ("Json", "bool_at") => RegIntrinsic::JsonBoolAt,
                    ("Json", "bool_at_or") | ("Json", "json_bool_at_or") => {
                        RegIntrinsic::JsonBoolAtOr
                    }
                    ("Json", "bool_field") => RegIntrinsic::JsonBoolField,
                    ("Json", "clone") => RegIntrinsic::JsonClone,
                    ("Json", "decode") => RegIntrinsic::JsonDecode,
                    ("Json", "decode_text") => RegIntrinsic::JsonDecodeText,
                    ("Json", "encode") => RegIntrinsic::JsonEncode,
                    ("Json", "field") => RegIntrinsic::JsonField,
                    ("Json", "field_bool") => RegIntrinsic::JsonFieldBool,
                    ("Json", "field_int") => RegIntrinsic::JsonFieldInt,
                    ("Json", "field_optional") => RegIntrinsic::JsonFieldOptional,
                    ("Json", "field_optional_bool") => RegIntrinsic::JsonFieldOptionalBool,
                    ("Json", "field_optional_int") => RegIntrinsic::JsonFieldOptionalInt,
                    ("Json", "field_optional_string") => RegIntrinsic::JsonFieldOptionalString,
                    ("Json", "field_string") => RegIntrinsic::JsonFieldString,
                    ("Json", "int_at") => RegIntrinsic::JsonIntAt,
                    ("Json", "int_at_or") | ("Json", "json_int_at_or") => RegIntrinsic::JsonIntAtOr,
                    ("Json", "is_array") => RegIntrinsic::JsonIsArray,
                    ("Json", "is_null") => RegIntrinsic::JsonIsNull,
                    ("Json", "is_object") => RegIntrinsic::JsonIsObject,
                    ("Json", "int_field") => RegIntrinsic::JsonIntField,
                    ("Json", "kind") => RegIntrinsic::JsonKind,
                    ("Json", "object") => RegIntrinsic::JsonObject,
                    ("Json", "json_parse") | ("Json", "parse") => RegIntrinsic::JsonParse,
                    ("Json", "parse_file") => RegIntrinsic::JsonParseFile,
                    ("Json", "object_keys") => RegIntrinsic::JsonObjectKeys,
                    ("Json", "object_len") => RegIntrinsic::JsonObjectLen,
                    ("Json", "quote_string") => RegIntrinsic::JsonQuoteString,
                    ("Json", "raw_field") => RegIntrinsic::JsonRawField,
                    ("Json", "string_at") => RegIntrinsic::JsonStringAt,
                    ("Json", "string_at_or") | ("Json", "json_string_at_or") => {
                        RegIntrinsic::JsonStringAtOr
                    }
                    ("Json", "string_array") => RegIntrinsic::JsonStringArray,
                    ("Json", "string_field") => RegIntrinsic::JsonStringField,
                    ("Json", "strings") => RegIntrinsic::JsonStrings,
                    ("Json", "to_string_at") => RegIntrinsic::JsonToStringAt,
                    ("Json", "to_string_at_or") => RegIntrinsic::JsonToStringAtOr,
                    ("Json", "to_string") => RegIntrinsic::JsonToString,
                    ("Json", "value") => RegIntrinsic::JsonValue,
                    ("Json", "values") => RegIntrinsic::JsonValues,
                    ("JsonError", "message") => RegIntrinsic::JsonErrorMessage,
                    ("List", "append") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.append expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListAppend {
                            dst,
                            list: arg_regs[0],
                            values: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("List", "clear") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.clear expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListClear {
                            dst,
                            list: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("List", "all") => RegIntrinsic::ListAll,
                    ("List", "any") => RegIntrinsic::ListAny,
                    ("List", "contains") => RegIntrinsic::ListContains,
                    ("List", "contains_value") => RegIntrinsic::ListContainsValue,
                    ("List", "count_where") => RegIntrinsic::ListCountWhere,
                    ("List", "consume") => RegIntrinsic::ListConsume,
                    ("List", "find") => RegIntrinsic::ListFind,
                    ("List", "flat_map") => RegIntrinsic::ListFlatMap,
                    ("List", "flatten") => RegIntrinsic::ListFlatten,
                    ("List", "first") => RegIntrinsic::ListFirst,
                    ("List", "filter") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.filter expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListFilter {
                            dst,
                            list: arg_regs[0],
                            predicate: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("List", "fold") => {
                        if arg_regs.len() != 3 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.fold expected 3 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListFold {
                            dst,
                            list: arg_regs[0],
                            state: arg_regs[1],
                            folder: arg_regs[2],
                        });
                        return Ok(dst);
                    }
                    ("List", "get") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.get expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListGet {
                            dst,
                            list: arg_regs[0],
                            index: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("List", "len") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.len expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListLen {
                            dst,
                            list: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("List", "map") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.map expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListMap {
                            dst,
                            list: arg_regs[0],
                            mapper: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("List", "is_empty") => RegIntrinsic::ListIsEmpty,
                    ("List", "join") => RegIntrinsic::ListJoin,
                    ("List", "group_by") => RegIntrinsic::ListGroupBy,
                    ("List", "last") => RegIntrinsic::ListLast,
                    ("List", "dedup") => RegIntrinsic::ListDedup,
                    ("List", "enumerate") => RegIntrinsic::ListEnumerate,
                    ("List", "max") => RegIntrinsic::ListMax,
                    ("List", "min") => RegIntrinsic::ListMin,
                    ("List", "new") => RegIntrinsic::ListNew,
                    ("List", "partition") => RegIntrinsic::ListPartition,
                    ("List", "pipeline") => RegIntrinsic::ListPipeline,
                    ("List", "pop") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.pop expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListPop {
                            dst,
                            list: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("List", "remove_at") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.remove_at expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListRemoveAt {
                            dst,
                            list: arg_regs[0],
                            index: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("List", "reverse") => RegIntrinsic::ListReverse,
                    ("List", "set") => {
                        if arg_regs.len() != 3 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.set expected 3 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListSet {
                            dst,
                            list: arg_regs[0],
                            index: arg_regs[1],
                            value: arg_regs[2],
                        });
                        return Ok(dst);
                    }
                    ("List", "skip") => RegIntrinsic::ListSkip,
                    ("List", "slice") => RegIntrinsic::ListSlice,
                    ("List", "sum") => RegIntrinsic::ListSum,
                    ("List", "zip") => RegIntrinsic::ListZip,
                    ("List", "sort") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.sort expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListSort {
                            dst,
                            list: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("List", "sort_by") => {
                        if arg_regs.len() != 3 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.sort_by expected 3 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListSortBy {
                            dst,
                            list: arg_regs[0],
                            key: arg_regs[1],
                            compare: arg_regs[2],
                        });
                        return Ok(dst);
                    }
                    ("List", "sort_with") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.sort_with expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListSortWith {
                            dst,
                            list: arg_regs[0],
                            compare: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("List", "push") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.push expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListPush {
                            dst,
                            list: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("List", "take") => RegIntrinsic::ListTake,
                    ("List", "to_json_strings") => RegIntrinsic::ListToJsonStrings,
                    ("List", "to_json_values") => RegIntrinsic::ListToJsonValues,
                    ("List", "try_fold") => RegIntrinsic::ListTryFold,
                    ("Log", "error") => RegIntrinsic::LogError,
                    ("Log", "error_json") => RegIntrinsic::LogErrorJson,
                    ("Log", "trace") => RegIntrinsic::LogTrace,
                    ("Log", "write") => RegIntrinsic::LogWrite,
                    ("Log", "write_json") => RegIntrinsic::LogWriteJson,
                    ("Map", "clear") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Map.clear expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::MapClear {
                            dst,
                            map: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("Map", "contains_key") => RegIntrinsic::MapContainsKey,
                    ("Map", "filter") => RegIntrinsic::MapFilter,
                    ("Map", "fold") => RegIntrinsic::MapFold,
                    ("Map", "for_each") => RegIntrinsic::MapForEach,
                    ("Map", "get") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Map.get expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::MapGet {
                            dst,
                            map: arg_regs[0],
                            key: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Map", "get_or_default") => RegIntrinsic::MapGetOrDefault,
                    ("Map", "is_empty") => RegIntrinsic::MapIsEmpty,
                    ("Map", "insert") => {
                        if arg_regs.len() != 3 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Map.insert expected 3 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::MapInsert {
                            dst,
                            map: arg_regs[0],
                            key: arg_regs[1],
                            value: arg_regs[2],
                        });
                        return Ok(dst);
                    }
                    ("Map", "insert_old") => {
                        if arg_regs.len() != 3 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Map.insert_old expected 3 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::MapInsertOld {
                            dst,
                            map: arg_regs[0],
                            key: arg_regs[1],
                            value: arg_regs[2],
                        });
                        return Ok(dst);
                    }
                    ("Map", "keys") => RegIntrinsic::MapKeys,
                    ("Map", "len") => RegIntrinsic::MapLen,
                    ("Map", "map_values") => RegIntrinsic::MapMapValues,
                    ("Map", "merge") => RegIntrinsic::MapMerge,
                    ("Map", "new") => RegIntrinsic::MapNew,
                    ("Map", "remove") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Map.remove expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::MapRemove {
                            dst,
                            map: arg_regs[0],
                            key: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Map", "try_fold") => RegIntrinsic::MapTryFold,
                    ("Map", "values") => RegIntrinsic::MapValues,
                    ("Option", "and_then") => RegIntrinsic::OptionAndThen,
                    ("Option", "filter") => RegIntrinsic::OptionFilter,
                    ("Option", "is_none") => RegIntrinsic::OptionIsNone,
                    ("Option", "is_some") => RegIntrinsic::OptionIsSome,
                    ("Option", "map") => RegIntrinsic::OptionMap,
                    ("Option", "ok_or") => RegIntrinsic::OptionOkOr,
                    ("Option", "or") => RegIntrinsic::OptionOr,
                    ("Option", "unwrap_or") => RegIntrinsic::OptionUnwrapOr,
                    ("Option", "unwrap_or_else") => RegIntrinsic::OptionUnwrapOrElse,
                    ("Ord", "compare") => RegIntrinsic::OrdCompare,
                    ("OS", "close") => RegIntrinsic::OsClose,
                    ("Patch", "apply_text") => RegIntrinsic::PatchApplyText,
                    ("Path", "exists") => RegIntrinsic::PathExists,
                    ("Path", "extension") => RegIntrinsic::PathExtension,
                    ("Path", "file_name") => RegIntrinsic::PathFileName,
                    ("Path", "from_string") => RegIntrinsic::PathFromString,
                    ("Path", "is_absolute") => RegIntrinsic::PathIsAbsolute,
                    ("Path", "is_dir") => RegIntrinsic::PathIsDir,
                    ("Path", "is_file") => RegIntrinsic::PathIsFile,
                    ("Path", "join") => RegIntrinsic::PathJoin,
                    ("Path", "list_files") => RegIntrinsic::PathListFiles,
                    ("Path", "list_paths") => RegIntrinsic::PathListPaths,
                    ("Path", "normalize") => RegIntrinsic::PathNormalize,
                    ("Path", "parent") => RegIntrinsic::PathParent,
                    ("Path", "read_string") => RegIntrinsic::PathReadString,
                    ("Path", "resolve_relative") => RegIntrinsic::PathResolveRelative,
                    ("Path", "safe_relative") => RegIntrinsic::PathSafeRelative,
                    ("Path", "starts_with") => RegIntrinsic::PathStartsWith,
                    ("Path", "to_string") => RegIntrinsic::PathToString,
                    ("Path", "with_extension") => RegIntrinsic::PathWithExtension,
                    ("Path", "write_string") => RegIntrinsic::PathWriteString,
                    ("PersistentMap", "clear") => RegIntrinsic::PersistentMapClear,
                    ("PersistentMap", "contains_key") => RegIntrinsic::PersistentMapContainsKey,
                    ("PersistentMap", "get") => RegIntrinsic::PersistentMapGet,
                    ("PersistentMap", "insert") => RegIntrinsic::PersistentMapInsert,
                    ("PersistentMap", "is_empty") => RegIntrinsic::PersistentMapIsEmpty,
                    ("PersistentMap", "len") => RegIntrinsic::PersistentMapLen,
                    ("PersistentMap", "new") => RegIntrinsic::PersistentMapNew,
                    ("PersistentMap", "remove") => RegIntrinsic::PersistentMapRemove,
                    ("Pipeline", "collect") => RegIntrinsic::PipelineCollect,
                    ("Pipeline", "each") => RegIntrinsic::PipelineEach,
                    ("Pipeline", "try_map") => RegIntrinsic::PipelineTryMap,
                    ("PoolError", "message") => RegIntrinsic::PoolErrorMessage,
                    ("PoolStats", "available") => RegIntrinsic::PoolStatsAvailable,
                    ("PoolStats", "capacity") => RegIntrinsic::PoolStatsCapacity,
                    ("PoolStats", "created") => RegIntrinsic::PoolStatsCreated,
                    ("PoolStats", "in_use") => RegIntrinsic::PoolStatsInUse,
                    ("Process", "run") => RegIntrinsic::ProcessRun,
                    ("Process", "run_async") => RegIntrinsic::ProcessRunAsync,
                    ("Process", "run_many_stdout") => RegIntrinsic::ProcessRunManyStdout,
                    ("Process", "run_many_stdout_async") => RegIntrinsic::ProcessRunManyStdoutAsync,
                    ("Process", "run_many_stdout_timeout") => {
                        RegIntrinsic::ProcessRunManyStdoutTimeout
                    }
                    ("Process", "run_many_stdout_timeout_async") => {
                        RegIntrinsic::ProcessRunManyStdoutTimeoutAsync
                    }
                    ("Process", "run_request") => RegIntrinsic::ProcessRunRequest,
                    ("Process", "run_request_async") => RegIntrinsic::ProcessRunRequestAsync,
                    ("Process", "run_request_cancellable_async") => {
                        RegIntrinsic::ProcessRunRequestCancellableAsync
                    }
                    ("Process", "run_stdout") => RegIntrinsic::ProcessRunStdout,
                    ("Process", "run_stdout_async") => RegIntrinsic::ProcessRunStdoutAsync,
                    ("Process", "run_stdout_timeout") => RegIntrinsic::ProcessRunStdoutTimeout,
                    ("Process", "run_stdout_timeout_async") => {
                        RegIntrinsic::ProcessRunStdoutTimeoutAsync
                    }
                    ("Process", "run_timeout") => RegIntrinsic::ProcessRunTimeout,
                    ("Process", "run_timeout_async") => RegIntrinsic::ProcessRunTimeoutAsync,
                    ("Process", "stream") => RegIntrinsic::ProcessStream,
                    ("Random", "bool") => RegIntrinsic::RandomBool,
                    ("Random", "bytes") => RegIntrinsic::RandomBytes,
                    ("Random", "float") => RegIntrinsic::RandomFloat,
                    ("Random", "int") => RegIntrinsic::RandomInt,
                    ("Random", "string") => RegIntrinsic::RandomString,
                    ("Pipeline", "filter") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Pipeline.filter expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListFilter {
                            dst,
                            list: arg_regs[0],
                            predicate: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Pipeline", "map") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Pipeline.map expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::ListMap {
                            dst,
                            list: arg_regs[0],
                            mapper: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Regex", "captures") => RegIntrinsic::RegexCaptures,
                    ("Regex", "compile") => RegIntrinsic::RegexCompile,
                    ("Regex", "find") => RegIntrinsic::RegexFind,
                    ("Regex", "is_match") => RegIntrinsic::RegexIsMatch,
                    ("Regex", "replace_all") => RegIntrinsic::RegexReplaceAll,
                    ("Regex", "split") => RegIntrinsic::RegexSplit,
                    ("RegexError", "message") => RegIntrinsic::RegexErrorMessage,
                    ("String", "concat") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM String.concat expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::StringConcat {
                            dst,
                            left: arg_regs[0],
                            right: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Result", "and_then") => RegIntrinsic::ResultAndThen,
                    ("Result", "err") => RegIntrinsic::ResultErr,
                    ("Result", "err_message") => RegIntrinsic::ResultErrMessage,
                    ("Result", "is_err") => RegIntrinsic::ResultIsErr,
                    ("Result", "is_ok") => RegIntrinsic::ResultIsOk,
                    ("Result", "map") => RegIntrinsic::ResultMap,
                    ("Result", "map_error") => RegIntrinsic::ResultMapError,
                    ("Result", "ok") => RegIntrinsic::ResultOk,
                    ("Result", "unwrap_or") => RegIntrinsic::ResultUnwrapOr,
                    ("Result", "unwrap_or_else") => RegIntrinsic::ResultUnwrapOrElse,
                    ("Request", "new") => RegIntrinsic::RequestNew,
                    ("Request", "path") => RegIntrinsic::RequestPath,
                    ("Receiver", "close") => RegIntrinsic::ReceiverClose,
                    ("Receiver", "into_stream") => RegIntrinsic::ReceiverIntoStream,
                    ("Receiver", "recv") => RegIntrinsic::ReceiverRecv,
                    ("Receiver", "recv_cancellable") => RegIntrinsic::ReceiverRecvCancellable,
                    ("Response", "body") => RegIntrinsic::ResponseBody,
                    ("Response", "ok") => RegIntrinsic::ResponseOk,
                    ("Response", "status") => RegIntrinsic::ResponseStatus,
                    ("Row", "field_string") => RegIntrinsic::RowFieldString,
                    ("RowBuffer", "new") => RegIntrinsic::RowBufferNew,
                    ("RuleLoader", "load_rules") => RegIntrinsic::RuleLoaderLoadRules,
                    ("ResourcePool", "borrow") => RegIntrinsic::ResourcePoolBorrow,
                    ("ResourcePool", "discard") => RegIntrinsic::ResourcePoolDiscard,
                    ("ResourcePool", "lazy") => RegIntrinsic::ResourcePoolLazy,
                    ("ResourcePool", "new") => RegIntrinsic::ResourcePoolNew,
                    ("ResourcePool", "stats") => RegIntrinsic::ResourcePoolStats,
                    ("ResourcePool", "try_borrow") => RegIntrinsic::ResourcePoolTryBorrow,
                    ("ResourcePool", "try_lazy") => RegIntrinsic::ResourcePoolTryLazy,
                    ("ResourcePool", "try_new") => RegIntrinsic::ResourcePoolTryNew,
                    ("Set", "clear") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Set.clear expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SetClear {
                            dst,
                            set: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("Set", "contains") => RegIntrinsic::SetContains,
                    ("Set", "difference") => RegIntrinsic::SetDifference,
                    ("Set", "for_each") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Set.for_each expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SetForEach {
                            dst,
                            set: arg_regs[0],
                            callback: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Set", "insert") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Set.insert expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SetInsert {
                            dst,
                            set: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Set", "intersection") => RegIntrinsic::SetIntersection,
                    ("Set", "is_empty") => RegIntrinsic::SetIsEmpty,
                    ("Set", "is_subset") => RegIntrinsic::SetIsSubset,
                    ("Set", "len") => RegIntrinsic::SetLen,
                    ("Set", "new") => RegIntrinsic::SetNew,
                    ("Set", "remove") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Set.remove expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SetRemove {
                            dst,
                            set: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Set", "to_list") => RegIntrinsic::SetToList,
                    ("Set", "union") => RegIntrinsic::SetUnion,
                    ("SortedSet", "clear") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM SortedSet.clear expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SortedSetClear {
                            dst,
                            set: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("SortedSet", "contains") => RegIntrinsic::SortedSetContains,
                    ("SortedSet", "insert") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM SortedSet.insert expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SortedSetInsert {
                            dst,
                            set: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("SortedSet", "is_empty") => RegIntrinsic::SortedSetIsEmpty,
                    ("SortedSet", "len") => RegIntrinsic::SortedSetLen,
                    ("SortedSet", "new") => RegIntrinsic::SortedSetNew,
                    ("SortedSet", "remove") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM SortedSet.remove expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SortedSetRemove {
                            dst,
                            set: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("SortedSet", "to_list") => RegIntrinsic::SortedSetToList,
                    ("SortedMap", "clear") => {
                        if arg_regs.len() != 1 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM SortedMap.clear expected 1 arg, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SortedMapClear {
                            dst,
                            map: arg_regs[0],
                        });
                        return Ok(dst);
                    }
                    ("SortedMap", "contains_key") => RegIntrinsic::SortedMapContainsKey,
                    ("SortedMap", "get") => RegIntrinsic::SortedMapGet,
                    ("SortedMap", "insert") => {
                        if arg_regs.len() != 3 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM SortedMap.insert expected 3 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SortedMapInsert {
                            dst,
                            map: arg_regs[0],
                            key: arg_regs[1],
                            value: arg_regs[2],
                        });
                        return Ok(dst);
                    }
                    ("SortedMap", "is_empty") => RegIntrinsic::SortedMapIsEmpty,
                    ("SortedMap", "keys") => RegIntrinsic::SortedMapKeys,
                    ("SortedMap", "len") => RegIntrinsic::SortedMapLen,
                    ("SortedMap", "new") => RegIntrinsic::SortedMapNew,
                    ("SortedMap", "remove") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM SortedMap.remove expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::SortedMapRemove {
                            dst,
                            map: arg_regs[0],
                            key: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("SortedMap", "values") => RegIntrinsic::SortedMapValues,
                    ("String", "after") => RegIntrinsic::StringAfter,
                    ("String", "before") => RegIntrinsic::StringBefore,
                    ("String", "char_at") => RegIntrinsic::StringCharAt,
                    ("String", "contains") => RegIntrinsic::StringContains,
                    ("String", "count") => RegIntrinsic::StringCount,
                    ("String", "copy") => RegIntrinsic::StringCopy,
                    ("String", "ends_with") => RegIntrinsic::StringEndsWith,
                    ("String", "env") => RegIntrinsic::EnvGet,
                    ("String", "env_or") => RegIntrinsic::EnvGetOrDefault,
                    ("String", "format") => RegIntrinsic::StringFormat,
                    ("String", "from_bool") => RegIntrinsic::StringFromBool,
                    ("String", "from_float") => RegIntrinsic::StringFromFloat,
                    ("String", "from_int") => RegIntrinsic::StringFromInt,
                    ("String", "index_of") => RegIntrinsic::StringIndexOf,
                    ("String", "is_empty") => RegIntrinsic::StringIsEmpty,
                    ("String", "join") => RegIntrinsic::StringJoin,
                    ("String", "lines") => RegIntrinsic::StringLines,
                    ("String", "chars") => RegIntrinsic::StringChars,
                    ("String", "len") => RegIntrinsic::StringLen,
                    ("String", "pad_left") => RegIntrinsic::StringPadLeft,
                    ("String", "pad_right") => RegIntrinsic::StringPadRight,
                    ("String", "parse_float") => RegIntrinsic::StringParseFloat,
                    ("String", "parse_int") => RegIntrinsic::StringParseInt,
                    ("String", "repeat") => RegIntrinsic::StringRepeat,
                    ("String", "replace") => RegIntrinsic::StringReplace,
                    ("String", "replace_first") => RegIntrinsic::StringReplaceFirst,
                    ("String", "reverse") => RegIntrinsic::StringReverse,
                    ("String", "slice") | ("String", "view") => RegIntrinsic::StringSlice,
                    ("String", "split") => RegIntrinsic::StringSplit,
                    ("String", "starts_with") => RegIntrinsic::StringStartsWith,
                    ("String", "strip_prefix") => RegIntrinsic::StringStripPrefix,
                    ("String", "safe_relative") => RegIntrinsic::PathSafeRelative,
                    ("String", "to_path") => RegIntrinsic::PathFromString,
                    ("String", "to_url") => RegIntrinsic::UrlFromString,
                    ("String", "to_bytes") => RegIntrinsic::BytesFromString,
                    ("String", "to_lowercase") => RegIntrinsic::StringToLowercase,
                    ("String", "to_uppercase") => RegIntrinsic::StringToUppercase,
                    ("String", "trim") => RegIntrinsic::StringTrim,
                    ("String", "trim_end") => RegIntrinsic::StringTrimEnd,
                    ("String", "trim_start") => RegIntrinsic::StringTrimStart,
                    ("TcpError", "message") => RegIntrinsic::TcpErrorMessage,
                    ("Toml", "parse_file") => RegIntrinsic::TomlParseFile,
                    ("StringBuilder", "finish") => RegIntrinsic::StringCopy,
                    ("StringBuilder", "new") => RegIntrinsic::StringBuilderNew,
                    ("StringBuilder", "push") => {
                        if arg_regs.len() != 2 {
                            return Err(EvalError::Runtime(format!(
                                "reg VM StringBuilder.push expected 2 args, got {}.",
                                arg_regs.len()
                            )));
                        }
                        self.emit(RegInstr::StringBuilderPush {
                            dst,
                            builder: arg_regs[0],
                            value: arg_regs[1],
                        });
                        return Ok(dst);
                    }
                    ("Stream", "collect_list") => RegIntrinsic::StreamCollectList,
                    ("Stream", "from_list") => RegIntrinsic::StreamFromList,
                    ("Stream", "next") => RegIntrinsic::StreamNext,
                    ("Sender", "close") => RegIntrinsic::SenderClose,
                    ("Sender", "send") => RegIntrinsic::SenderSend,
                    ("Sender", "send_cancellable") => RegIntrinsic::SenderSendCancellable,
                    ("StringView", "after") => RegIntrinsic::StringAfter,
                    ("StringView", "before") => RegIntrinsic::StringBefore,
                    ("StringView", "contains") => RegIntrinsic::StringContains,
                    ("StringView", "is_empty") => RegIntrinsic::StringIsEmpty,
                    ("StringView", "len") => RegIntrinsic::StringLen,
                    ("StringView", "slice") => RegIntrinsic::StringSlice,
                    ("StringView", "starts_with") => RegIntrinsic::StringStartsWith,
                    ("StringView", "to_string") => RegIntrinsic::StringCopy,
                    ("Tcp", "connect") => RegIntrinsic::TcpConnect,
                    ("TempDir", "keep") => RegIntrinsic::TempDirKeep,
                    ("TempDir", "new") => RegIntrinsic::TempDirNew,
                    ("TempDir", "new_in") => RegIntrinsic::TempDirNewIn,
                    ("TempDir", "path") => RegIntrinsic::TempDirPath,
                    ("TcpStream", "read") => RegIntrinsic::TcpStreamRead,
                    ("TcpStream", "shutdown") => RegIntrinsic::TcpStreamShutdown,
                    ("TcpStream", "write") => RegIntrinsic::TcpStreamWrite,
                    ("TcpStream", "write_all") => RegIntrinsic::TcpStreamWriteAll,
                    ("Timer", "sleep") => RegIntrinsic::TimerSleep,
                    ("Timer", "sleep_cancellable") => RegIntrinsic::TimerSleepCancellable,
                    ("Timer", "sleep_until") => RegIntrinsic::TimerSleepUntil,
                    ("Url", "decode_component") => RegIntrinsic::UrlDecodeComponent,
                    ("Url", "encode_component") => RegIntrinsic::UrlEncodeComponent,
                    ("Url", "from_string") => RegIntrinsic::UrlFromString,
                    ("Url", "to_string") => RegIntrinsic::UrlToString,
                    ("Uuid", "new_v4") => RegIntrinsic::UuidNewV4,
                    ("Workspace", "resolve") => RegIntrinsic::PathResolveRelative,
                    ("WebSocket", "close") => RegIntrinsic::WebSocketClose,
                    ("WebSocket", "connect") => RegIntrinsic::WebSocketConnect,
                    ("WebSocket", "recv_bytes") => RegIntrinsic::WebSocketRecvBytes,
                    ("WebSocket", "recv_text") => RegIntrinsic::WebSocketRecvText,
                    ("WebSocket", "send_bytes") => RegIntrinsic::WebSocketSendBytes,
                    ("WebSocket", "send_text") => RegIntrinsic::WebSocketSendText,
                    ("WebSocketError", "message") => RegIntrinsic::WebSocketErrorMessage,
                    ("Yaml", "parse") => RegIntrinsic::YamlParse,
                    ("Yaml", "parse_file") => RegIntrinsic::YamlParseFile,
                    ("Weak", "downgrade") => RegIntrinsic::WeakDowngrade,
                    ("Weak", "from") => RegIntrinsic::WeakFrom,
                    ("Weak", "upgrade") => RegIntrinsic::WeakUpgrade,
                    _ => {
                        if self.is_native_function(Some(namespace_root), name_root) {
                            self.emit(RegInstr::CallNative {
                                dst,
                                key: format!("{namespace_root}.{name_root}"),
                                args: arg_regs,
                            });
                            return Ok(dst);
                        }
                        return Err(EvalError::Runtime(format!(
                            "reg VM v0 does not support intrinsic `{namespace}.{name}`."
                        )));
                    }
                };
                match intrinsic {
                    RegIntrinsic::JsonDecode | RegIntrinsic::JsonDecodeText => {
                        let type_arg = type_arg_names(name)
                            .and_then(|args| args.first().copied())
                            .ok_or_else(|| {
                                EvalError::Runtime(format!(
                                    "reg VM {namespace}.{name} requires a concrete type argument."
                                ))
                            })?;
                        self.emit(RegInstr::CallTypedIntrinsic {
                            dst,
                            intrinsic,
                            type_arg: type_root_name(type_arg).to_string(),
                            args: arg_regs,
                        });
                    }
                    _ => {
                        self.emit(RegInstr::CallIntrinsic {
                            dst,
                            intrinsic,
                            args: arg_regs,
                        });
                    }
                }
            }
            Callee::ReceiverCall { .. } => {
                unreachable!("receiver calls return before arg lowering")
            }
        }
        Ok(dst)
    }

    fn is_native_function(&self, namespace: Option<&str>, name: &str) -> bool {
        self.hir
            .resolve_function(namespace, type_root_name(name))
            .is_some_and(|signature| signature.is_native && !signature.is_builtin)
    }

    fn variant_match(&mut self, value: &HirExpr, arms: &[HirMatchArm]) -> Result<bool, EvalError> {
        if arms.is_empty()
            || !arms
                .iter()
                .all(|arm| self.is_supported_match_pattern(&arm.pattern))
        {
            return Ok(false);
        }

        let src = self.expr(value)?;
        let mut failure_patches = Vec::new();
        let mut end_jumps = Vec::new();
        for arm in arms {
            let arm_ip = self.function.code.len();
            self.patch_match_failures(failure_patches, arm_ip);
            failure_patches = self.lower_match_pattern(&arm.pattern, src)?;
            if let Some(guard) = &arm.guard {
                let guard_failure = self.condition_jump(guard, false, usize::MAX)?;
                failure_patches.push(MatchFailurePatch::Jump(guard_failure));
            }
            self.block(&arm.body)?;
            end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
        }

        let no_match_ip = self.function.code.len();
        self.patch_match_failures(failure_patches, no_match_ip);
        self.emit(RegInstr::RuntimeError {
            message: "reg VM match did not match any arm.".to_string(),
        });
        let end_ip = self.function.code.len();
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(true)
    }

    fn match_expr(&mut self, value: &HirExpr, arms: &[HirMatchArm]) -> Result<Reg, EvalError> {
        if arms.is_empty()
            || !arms
                .iter()
                .all(|arm| self.is_supported_match_pattern(&arm.pattern))
        {
            return Err(EvalError::Runtime(
                "reg VM v0 does not support this match expression pattern.".to_string(),
            ));
        }

        let src = self.expr(value)?;
        let dst = self.temp();
        let mut failure_patches = Vec::new();
        let mut end_jumps = Vec::new();
        for arm in arms {
            let arm_ip = self.function.code.len();
            self.patch_match_failures(failure_patches, arm_ip);
            failure_patches = self.lower_match_pattern(&arm.pattern, src)?;
            if let Some(guard) = &arm.guard {
                let guard_failure = self.condition_jump(guard, false, usize::MAX)?;
                failure_patches.push(MatchFailurePatch::Jump(guard_failure));
            }
            let value = self.expr_block_value(&arm.body)?;
            self.emit(RegInstr::Move { dst, src: value });
            end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
        }

        let no_match_ip = self.function.code.len();
        self.patch_match_failures(failure_patches, no_match_ip);
        self.emit(RegInstr::RuntimeError {
            message: "reg VM match expression did not match any arm.".to_string(),
        });
        let end_ip = self.function.code.len();
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(dst)
    }

    fn lower_match_pattern(
        &mut self,
        pattern: &MatchPattern,
        src: Reg,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        match pattern {
            MatchPattern::Binding { name, .. } => {
                let dst = self.local(name);
                self.emit(RegInstr::Move { dst, src });
                Ok(Vec::new())
            }
            MatchPattern::Wildcard(_) => Ok(Vec::new()),
            MatchPattern::Variant { name, binding, .. } if name == "Some" => {
                self.lower_option_some_pattern(src, binding.as_deref())
            }
            MatchPattern::Variant { name, .. } if name == "None" => {
                let match_ip = self.emit(RegInstr::MatchOption {
                    src,
                    some_ip: usize::MAX,
                    none_ip: usize::MAX,
                });
                let pass_ip = self.function.code.len();
                self.patch_match_none(match_ip, pass_ip);
                Ok(vec![MatchFailurePatch::OptionSome(match_ip)])
            }
            MatchPattern::Variant { name, binding, .. } if name == "Ok" || name == "Err" => {
                self.lower_result_variant_pattern(src, name, binding.as_deref())
            }
            MatchPattern::Variant { name, binding, .. }
                if self.hir.sum_type_for_variant(name).is_some() =>
            {
                self.lower_user_variant_pattern(src, name, binding.as_deref())
            }
            MatchPattern::Struct { name, fields, .. }
                if self.hir.sum_type_for_variant(name).is_some() =>
            {
                self.lower_user_struct_variant_pattern(src, name, fields)
            }
            MatchPattern::Literal { value, .. } => self.lower_literal_pattern(src, value),
            _ => Err(EvalError::Runtime(
                "reg VM v0 does not support this match pattern.".to_string(),
            )),
        }
    }

    fn is_supported_match_pattern(&self, pattern: &MatchPattern) -> bool {
        match pattern {
            MatchPattern::Binding { .. }
            | MatchPattern::Literal { .. }
            | MatchPattern::Wildcard(_) => true,
            MatchPattern::Variant { name, binding, .. }
                if matches!(name.as_str(), "Some" | "None" | "Ok" | "Err") =>
            {
                binding
                    .as_deref()
                    .is_none_or(|binding| self.is_supported_match_pattern(binding))
            }
            MatchPattern::Variant { name, binding, .. } => {
                self.hir.sum_type_for_variant(name).is_some()
                    && binding
                        .as_deref()
                        .is_none_or(|binding| self.is_supported_match_pattern(binding))
            }
            MatchPattern::Struct { name, fields, .. } => {
                self.hir.sum_type_for_variant(name).is_some()
                    && fields.iter().all(|field| {
                        field.ignored
                            || field
                                .pattern
                                .as_deref()
                                .is_none_or(|pattern| self.is_supported_match_pattern(pattern))
                    })
            }
        }
    }

    fn lower_literal_pattern(
        &mut self,
        src: Reg,
        literal: &MatchLiteral,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let expected = self.temp();
        match literal {
            MatchLiteral::Int(value) => {
                let value = value.parse::<i64>().map_err(|error| {
                    EvalError::Runtime(format!(
                        "reg VM could not parse match int literal `{value}`: {error}"
                    ))
                })?;
                self.emit(RegInstr::LoadInt {
                    dst: expected,
                    value,
                });
            }
            MatchLiteral::String(value) => {
                self.emit(RegInstr::LoadString {
                    dst: expected,
                    value: Rc::new(decode_string_token(value)),
                });
            }
            MatchLiteral::Bool(value) => {
                self.emit(RegInstr::LoadBool {
                    dst: expected,
                    value: *value,
                });
            }
        }
        let matches = self.temp();
        self.emit(RegInstr::Equal {
            dst: matches,
            lhs: src,
            rhs: expected,
        });
        let failure = self.emit(RegInstr::JumpIfBool {
            cond: matches,
            expected: false,
            target: usize::MAX,
        });
        Ok(vec![MatchFailurePatch::Jump(failure)])
    }

    fn lower_option_some_pattern(
        &mut self,
        src: Reg,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchOption {
            src,
            some_ip: usize::MAX,
            none_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::OptionNone(match_ip)];
        if let Some(binding) = binding {
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapSome { dst, src });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapSome { dst: payload, src });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        }
        Ok(failures)
    }

    fn lower_result_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchResult {
            src,
            ok_ip: usize::MAX,
            err_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        let mut failures = match variant {
            "Ok" => {
                self.patch_jump(match_ip, pass_ip);
                vec![MatchFailurePatch::ResultErr(match_ip)]
            }
            "Err" => {
                self.patch_result_match_err(match_ip, pass_ip);
                vec![MatchFailurePatch::ResultOk(match_ip)]
            }
            _ => unreachable!("result variant was validated before lowering"),
        };
        if let Some(binding) = binding {
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst,
                        src,
                        expected: variant.to_string(),
                    });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src,
                        expected: variant.to_string(),
                    });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        }
        Ok(failures)
    }

    fn lower_user_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let fields = self.hir.sum_variant_fields(variant).unwrap_or(&[]);
        let match_ip = self.emit(RegInstr::MatchVariant {
            src,
            expected: variant.to_string(),
            match_ip: usize::MAX,
            else_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::VariantOther(match_ip)];
        if let Some(binding) = binding {
            if fields.len() != 1 {
                return Err(EvalError::Runtime(format!(
                    "reg VM variant `{variant}` binding requires exactly one field, got {}.",
                    fields.len()
                )));
            }
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst,
                        src,
                        expected: variant.to_string(),
                    });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src,
                        expected: variant.to_string(),
                    });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        } else if !fields.is_empty() {
            return Err(EvalError::Runtime(format!(
                "reg VM variant `{variant}` pattern requires a payload binding."
            )));
        }
        Ok(failures)
    }

    fn lower_user_struct_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        fields: &[MatchFieldPattern],
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchVariant {
            src,
            expected: variant.to_string(),
            match_ip: usize::MAX,
            else_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::VariantOther(match_ip)];
        for field in fields {
            if field.ignored {
                continue;
            }
            let field_reg = self.temp();
            self.emit(RegInstr::GetField {
                dst: field_reg,
                base: src,
                name: field.name.clone(),
            });
            if let Some(pattern) = field.pattern.as_deref() {
                failures.extend(self.lower_match_pattern(pattern, field_reg)?);
            } else if let Some(binding) = field.binding.as_ref() {
                let dst = self.local(binding);
                self.emit(RegInstr::Move {
                    dst,
                    src: field_reg,
                });
            } else {
                return Err(EvalError::Runtime(format!(
                    "reg VM struct variant pattern field `{}` has no binding or nested pattern.",
                    field.name
                )));
            }
        }
        Ok(failures)
    }

    fn patch_match_failures(&mut self, patches: Vec<MatchFailurePatch>, target: usize) {
        for patch in patches {
            match patch {
                MatchFailurePatch::Jump(ip) => self.patch_jump(ip, target),
                MatchFailurePatch::OptionSome(ip) | MatchFailurePatch::ResultOk(ip) => {
                    self.patch_jump(ip, target)
                }
                MatchFailurePatch::OptionNone(ip) => self.patch_match_none(ip, target),
                MatchFailurePatch::ResultErr(ip) => self.patch_result_match_err(ip, target),
                MatchFailurePatch::VariantOther(ip) => self.patch_variant_match_else(ip, target),
            }
        }
    }

    fn map_get_match(
        &mut self,
        value: &HirExpr,
        arms: &[crate::hir::HirMatchArm],
    ) -> Result<bool, EvalError> {
        let HirExpr::Call {
            callee: Callee::Qualified { namespace, name },
            args,
            receiver: None,
            ..
        } = value
        else {
            return Ok(false);
        };
        if type_root_name(namespace) != "Map" || type_root_name(name) != "get" || args.len() != 2 {
            return Ok(false);
        }
        if arms.len() != 2 {
            return Ok(false);
        }
        if arms.iter().any(|arm| arm.guard.is_some()) {
            return Ok(false);
        }

        let mut some_binding = None;
        let mut has_none = false;
        for arm in arms {
            match &arm.pattern {
                MatchPattern::Variant {
                    name,
                    binding: Some(binding),
                    ..
                } if name == "Some" => {
                    some_binding = binding.binding_names().into_iter().next();
                }
                MatchPattern::Variant { name, .. } if name == "None" => {
                    has_none = true;
                }
                _ => return Ok(false),
            }
        }
        let Some(some_binding) = some_binding else {
            return Ok(false);
        };
        if !has_none {
            return Ok(false);
        }

        let map = self.expr(&args[0].value)?;
        let key = self.expr(&args[1].value)?;
        let value_dst = self.local(&some_binding);
        let match_ip = self.emit(RegInstr::MatchMapGet {
            map,
            key,
            value_dst,
            some_ip: usize::MAX,
            none_ip: usize::MAX,
        });
        let mut some_ip = None;
        let mut none_ip = None;
        let mut end_jumps = Vec::new();
        for arm in arms {
            match &arm.pattern {
                MatchPattern::Variant { name, .. } if name == "Some" => {
                    let ip = self.function.code.len();
                    some_ip = Some(ip);
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
                }
                MatchPattern::Variant { name, .. } if name == "None" => {
                    let ip = self.function.code.len();
                    none_ip = Some(ip);
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
                }
                _ => unreachable!("Map.get match arms were validated before lowering"),
            }
        }
        let end_ip = self.function.code.len();
        self.patch_map_match_some(
            match_ip,
            some_ip.ok_or_else(|| {
                EvalError::Runtime("reg VM Map.get match is missing Some arm.".to_string())
            })?,
        );
        self.patch_map_match_none(
            match_ip,
            none_ip.ok_or_else(|| {
                EvalError::Runtime("reg VM Map.get match is missing None arm.".to_string())
            })?,
        );
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(true)
    }

    fn struct_match(
        &mut self,
        value: &HirExpr,
        arms: &[crate::hir::HirMatchArm],
    ) -> Result<bool, EvalError> {
        let [arm] = arms else {
            return Ok(false);
        };
        if arm.guard.is_some() {
            return Ok(false);
        }
        let MatchPattern::Struct { fields, .. } = &arm.pattern else {
            return Ok(false);
        };

        let src = self.expr(value)?;
        for field in fields {
            if field.ignored {
                continue;
            }
            let Some(binding) = field.binding.as_ref() else {
                return Ok(false);
            };
            if field.pattern.is_some() {
                return Ok(false);
            }
            let dst = self.local(binding);
            self.emit(RegInstr::GetField {
                dst,
                base: src,
                name: field.name.clone(),
            });
        }
        self.block(&arm.body)?;
        Ok(true)
    }
}

fn closure_capture_names(
    body: &HirBlock,
    params: &[String],
    explicit_captures: &[crate::hir::HirClosureCapture],
    outer_locals: &HashMap<String, Reg>,
) -> Vec<String> {
    let mut names = explicit_captures
        .iter()
        .map(|capture| capture.name.clone())
        .collect::<Vec<_>>();
    let mut seen = names.iter().cloned().collect::<HashSet<_>>();
    let mut bound = params.iter().cloned().collect::<HashSet<_>>();
    let mut free = BTreeSet::new();
    collect_free_locals_block(body, &mut bound, &mut free);
    for name in free {
        if outer_locals.contains_key(&name) && seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names
}

fn collect_free_locals_block(
    block: &HirBlock,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    for statement in &block.statements {
        collect_free_locals_stmt(statement, bound, free);
    }
}

fn collect_free_locals_stmt(
    statement: &HirStmt,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    match statement {
        HirStmt::Let { name, value, .. } => {
            if let Some(value) = value {
                collect_free_locals_expr(value, bound, free);
            }
            bound.insert(name.clone());
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_free_locals_expr(value, bound, free);
            }
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_free_locals_expr(resource, bound, free);
            let mut body_bound = bound.clone();
            body_bound.insert(binding.clone());
            collect_free_locals_block(body, &mut body_bound, free);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_free_locals_expr(condition, bound, free);
            collect_free_locals_block(&then_body.clone(), &mut bound.clone(), free);
            if let Some(else_body) = else_body {
                collect_free_locals_block(&else_body.clone(), &mut bound.clone(), free);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_free_locals_expr(condition, bound, free);
            }
            collect_free_locals_block(&body.clone(), &mut bound.clone(), free);
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_free_locals_expr(iterable, bound, free);
            let mut body_bound = bound.clone();
            body_bound.insert(binding.clone());
            collect_free_locals_block(body, &mut body_bound, free);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_free_locals_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                for binding in arm.pattern.binding_names() {
                    arm_bound.insert(binding.to_string());
                }
                if let Some(guard) = &arm.guard {
                    collect_free_locals_expr(guard, &mut arm_bound, free);
                }
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_free_locals_expr(&arm.operation, bound, free);
                let mut arm_bound = bound.clone();
                arm_bound.insert(arm.binding.clone());
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirStmt::Assign { target, value, .. } => {
            collect_free_locals_expr(target, bound, free);
            collect_free_locals_expr(value, bound, free);
        }
        HirStmt::Expr(value) => collect_free_locals_expr(value, bound, free),
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_free_locals_expr(
    expr: &HirExpr,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    match expr {
        HirExpr::Ident { name, .. } => {
            if !bound.contains(name) {
                free.insert(name.clone());
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_free_locals_expr(&field.value, bound, free);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_free_locals_expr(&entry.key, bound, free);
                collect_free_locals_expr(&entry.value, bound, free);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_free_locals_expr(item, bound, free);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_free_locals_expr(left, bound, free);
            collect_free_locals_expr(right, bound, free);
        }
        HirExpr::Field { base, .. } => collect_free_locals_expr(base, bound, free),
        HirExpr::Index { base, index, .. } => {
            collect_free_locals_expr(base, bound, free);
            collect_free_locals_expr(index, bound, free);
        }
        HirExpr::Call { receiver, args, .. } => {
            if let Some(receiver) = receiver {
                collect_free_locals_expr(&receiver.value, bound, free);
            }
            for arg in args {
                collect_free_locals_expr(&arg.value, bound, free);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_free_locals_expr(value, bound, free),
        HirExpr::Closure {
            params,
            captures,
            body,
            ..
        } => {
            for capture in captures {
                if !bound.contains(&capture.name) {
                    free.insert(capture.name.clone());
                }
            }
            let mut nested_bound = bound.clone();
            for param in params {
                nested_bound.insert(param.clone());
            }
            collect_free_locals_block(body, &mut nested_bound, free);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_free_locals_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                for binding in arm.pattern.binding_names() {
                    arm_bound.insert(binding.to_string());
                }
                if let Some(guard) = &arm.guard {
                    collect_free_locals_expr(guard, &mut arm_bound, free);
                }
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirExpr::Number { .. } | HirExpr::String { .. } | HirExpr::Unknown(_) => {}
    }
}

struct RegVm {
    unit: Rc<RegUnit>,
    args: Vec<String>,
    native_bindings: HashMap<String, NativeInterpreterFn>,
    stdout: String,
    stderr: String,
    stack: Vec<VmValue>,
    written: Vec<bool>,
    next_cancellation_id: i64,
    cancellation_flags: HashMap<i64, bool>,
    next_channel_id: i64,
    channels: HashMap<i64, VmChannel>,
    next_tcp_stream_id: i64,
    tcp_streams: HashMap<i64, TcpStream>,
    next_websocket_id: i64,
    websockets: HashMap<i64, TcpStream>,
    next_pool_id: i64,
    pools: HashMap<i64, VmResourcePool>,
}

#[derive(Debug, Clone)]
struct VmChannel {
    capacity: usize,
    queue: VecDeque<VmValue>,
    senders: i64,
    receiver_taken: bool,
    receiver_closed: bool,
}

impl VmChannel {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            queue: VecDeque::new(),
            senders: 0,
            receiver_taken: false,
            receiver_closed: false,
        }
    }
}

#[derive(Debug, Clone)]
struct VmResourcePool {
    capacity: i64,
    created: i64,
    in_use: i64,
    idle: Vec<VmValue>,
    factory: Option<Rc<VmClosure>>,
    factory_returns_result: bool,
}

struct VmResourcePoolLease {
    pool_id: i64,
    discarded: bool,
    value: VmValue,
}

impl RegVm {
    fn new(
        unit: Rc<RegUnit>,
        args: Vec<String>,
        native_bindings: HashMap<String, NativeInterpreterFn>,
    ) -> Self {
        Self {
            unit,
            args,
            native_bindings,
            stdout: String::new(),
            stderr: String::new(),
            stack: Vec::new(),
            written: Vec::new(),
            next_cancellation_id: 1,
            cancellation_flags: HashMap::new(),
            next_channel_id: 1,
            channels: HashMap::new(),
            next_tcp_stream_id: 1,
            tcp_streams: HashMap::new(),
            next_websocket_id: 1,
            websockets: HashMap::new(),
            next_pool_id: 1,
            pools: HashMap::new(),
        }
    }

    /// Grow the shared register stack so that `stack[..upto]` is addressable.
    /// The stack only ever grows; frames are reused in place.
    fn ensure_regs(&mut self, upto: usize) {
        if self.stack.len() < upto {
            self.stack.resize(upto, VmValue::Unit);
            self.written.resize(upto, false);
        }
    }

    fn prepare_frame(&mut self, base: usize, regs: usize) {
        self.ensure_regs(base + regs);
        for written in &mut self.written[base..base + regs] {
            *written = false;
        }
    }

    #[inline(always)]
    fn reg(&self, index: usize) -> &VmValue {
        debug_assert!(
            self.written.get(index).copied().unwrap_or(false),
            "reg VM read uninitialized register {index}"
        );
        &self.stack[index]
    }

    #[inline(always)]
    fn set_reg(&mut self, index: usize, value: VmValue) {
        self.stack[index] = value;
        self.written[index] = true;
    }

    #[inline(always)]
    fn take_reg(&mut self, index: usize) -> VmValue {
        debug_assert!(
            self.written.get(index).copied().unwrap_or(false),
            "reg VM take uninitialized register {index}"
        );
        self.written[index] = false;
        std::mem::replace(&mut self.stack[index], VmValue::Unit)
    }

    fn call_function(&mut self, name: &str, args: Vec<VmValue>) -> Result<VmValue, EvalError> {
        let function_id = self.unit.function_ids.get(name).copied().ok_or_else(|| {
            EvalError::Runtime(format!("reg VM cannot resolve function `{name}`."))
        })?;
        let unit = Rc::clone(&self.unit);
        let function = &unit.functions[function_id];
        let expected_args = function.captures + function.params;
        if args.len() != expected_args {
            return Err(EvalError::Runtime(format!(
                "reg VM function `{}` expected {} args, got {}.",
                function.name,
                expected_args,
                args.len()
            )));
        }
        let base = self.stack.len();
        self.prepare_frame(base, function.regs);
        for (index, arg) in args.into_iter().enumerate() {
            self.set_reg(base + index, arg);
        }
        self.run_frame(&unit, function, base)
    }

    // Shared register stack with frame windows. Each frame owns
    // `stack[base .. base + function.regs]`; a callee is placed immediately
    // above the caller at `base + function.regs`. The stack only grows
    // (`ensure_regs`) so recursion is bounded only by memory. Debug builds keep
    // a written-register bitmap so stale slots cannot mask lowering bugs.
    fn run_frame(
        &mut self,
        unit: &RegUnit,
        function: &RegFunction,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        self.ensure_regs(base + function.regs);
        let next_base = base + function.regs;
        let mut ip = 0usize;
        while let Some(instr) = function.code.get(ip) {
            ip += 1;
            match instr {
                RegInstr::LoadUnit { dst } => self.set_reg(base + *dst, VmValue::Unit),
                RegInstr::LoadInt { dst, value } => self.set_reg(base + *dst, VmValue::Int(*value)),
                RegInstr::LoadFloat { dst, value } => {
                    self.set_reg(base + *dst, VmValue::Float(*value))
                }
                RegInstr::LoadBool { dst, value } => {
                    self.set_reg(base + *dst, VmValue::Bool(*value))
                }
                RegInstr::LoadString { dst, value } => {
                    self.set_reg(base + *dst, VmValue::String(Rc::clone(value)));
                }
                RegInstr::Move { dst, src } => {
                    let value = self.reg(base + *src).clone();
                    self.set_reg(base + *dst, value);
                }
                RegInstr::Manage { dst, src } => {
                    let value = self.reg(base + *src).clone();
                    self.set_reg(base + *dst, VmValue::Managed(Rc::new(RefCell::new(value))));
                }
                RegInstr::GetField {
                    dst,
                    base: obj,
                    name,
                } => {
                    let value = read_field_ref(self.reg(base + *obj), name)?;
                    self.set_reg(base + *dst, value);
                }
                RegInstr::MakeStruct { dst, name, fields } => {
                    let mut field_values = HashMap::with_capacity(fields.len());
                    for (field, reg) in fields {
                        field_values.insert(field.clone(), self.reg(base + *reg).clone());
                    }
                    self.set_reg(
                        base + *dst,
                        VmValue::Struct(Rc::new(VmStruct {
                            name: Rc::from(name.as_str()),
                            fields: field_values,
                        })),
                    );
                }
                RegInstr::ResourceDrop { resource } => {
                    let value = self.reg(base + *resource).clone();
                    self.run_resource_drop(unit, value, next_base)?;
                }
                RegInstr::MakeVariant { dst, name, fields } => {
                    let mut field_values = HashMap::with_capacity(fields.len());
                    for (field, reg) in fields {
                        field_values.insert(field.clone(), self.reg(base + *reg).clone());
                    }
                    self.set_reg(
                        base + *dst,
                        VmValue::Variant(Rc::new(VmStruct {
                            name: Rc::from(name.as_str()),
                            fields: field_values,
                        })),
                    );
                }
                RegInstr::MakeList { dst, items } => {
                    let mut list = Vec::with_capacity(items.len());
                    for reg in items {
                        list.push(self.reg(base + *reg).clone());
                    }
                    self.set_reg(base + *dst, VmValue::List(Rc::new(RefCell::new(list))));
                }
                RegInstr::MakeObject { dst, fields } => {
                    let mut object = serde_json::Map::new();
                    for (field, reg) in fields {
                        let value = vm_value_to_json_literal(self.reg(base + *reg))?;
                        object.insert(field.clone(), value);
                    }
                    self.set_reg(
                        base + *dst,
                        VmValue::Json(Rc::new(serde_json::Value::Object(object))),
                    );
                }
                RegInstr::MakeMap { dst, entries } => {
                    let mut map = HashMap::with_capacity(entries.len());
                    for (key, value) in entries {
                        let key = map_key_from_value(self.reg(base + *key))?;
                        map.insert(key, self.reg(base + *value).clone());
                    }
                    self.set_reg(base + *dst, VmValue::Map(Rc::new(RefCell::new(map))));
                }
                RegInstr::AddInt { dst, lhs, rhs } => {
                    let value = eval_numeric_binary(
                        BinaryOp::Add,
                        self.reg(base + *lhs),
                        self.reg(base + *rhs),
                    )?;
                    self.set_reg(base + *dst, value);
                }
                RegInstr::SubInt { dst, lhs, rhs } => {
                    let value = eval_numeric_binary(
                        BinaryOp::Subtract,
                        self.reg(base + *lhs),
                        self.reg(base + *rhs),
                    )?;
                    self.set_reg(base + *dst, value);
                }
                RegInstr::MulInt { dst, lhs, rhs } => {
                    let value = eval_numeric_binary(
                        BinaryOp::Multiply,
                        self.reg(base + *lhs),
                        self.reg(base + *rhs),
                    )?;
                    self.set_reg(base + *dst, value);
                }
                RegInstr::DivInt { dst, lhs, rhs } => {
                    let value = eval_numeric_binary(
                        BinaryOp::Divide,
                        self.reg(base + *lhs),
                        self.reg(base + *rhs),
                    )?;
                    self.set_reg(base + *dst, value);
                }
                RegInstr::ModInt { dst, lhs, rhs } => {
                    let l = expect_int_ref(self.reg(base + *lhs))?;
                    let r = expect_int_ref(self.reg(base + *rhs))?;
                    self.set_reg(base + *dst, VmValue::Int(l % r));
                }
                RegInstr::BitAndInt { dst, lhs, rhs } => {
                    let l = expect_int_ref(self.reg(base + *lhs))?;
                    let r = expect_int_ref(self.reg(base + *rhs))?;
                    self.set_reg(base + *dst, VmValue::Int(l & r));
                }
                RegInstr::BitOrInt { dst, lhs, rhs } => {
                    let l = expect_int_ref(self.reg(base + *lhs))?;
                    let r = expect_int_ref(self.reg(base + *rhs))?;
                    self.set_reg(base + *dst, VmValue::Int(l | r));
                }
                RegInstr::BitXorInt { dst, lhs, rhs } => {
                    let l = expect_int_ref(self.reg(base + *lhs))?;
                    let r = expect_int_ref(self.reg(base + *rhs))?;
                    self.set_reg(base + *dst, VmValue::Int(l ^ r));
                }
                RegInstr::ShiftLeftInt { dst, lhs, rhs } => {
                    let l = expect_int_ref(self.reg(base + *lhs))?;
                    let r = expect_int_ref(self.reg(base + *rhs))?;
                    self.set_reg(base + *dst, VmValue::Int(l.wrapping_shl(r.max(0) as u32)));
                }
                RegInstr::ShiftRightInt { dst, lhs, rhs } => {
                    let l = expect_int_ref(self.reg(base + *lhs))?;
                    let r = expect_int_ref(self.reg(base + *rhs))?;
                    self.set_reg(base + *dst, VmValue::Int(l.wrapping_shr(r.max(0) as u32)));
                }
                RegInstr::LessInt { dst, lhs, rhs } => {
                    let value = eval_numeric_compare(
                        RegIntCompare::Less,
                        self.reg(base + *lhs),
                        self.reg(base + *rhs),
                    )?;
                    self.set_reg(base + *dst, VmValue::Bool(value));
                }
                RegInstr::LessEqualInt { dst, lhs, rhs } => {
                    let value = eval_numeric_compare(
                        RegIntCompare::LessEqual,
                        self.reg(base + *lhs),
                        self.reg(base + *rhs),
                    )?;
                    self.set_reg(base + *dst, VmValue::Bool(value));
                }
                RegInstr::GreaterInt { dst, lhs, rhs } => {
                    let value = eval_numeric_compare(
                        RegIntCompare::Greater,
                        self.reg(base + *lhs),
                        self.reg(base + *rhs),
                    )?;
                    self.set_reg(base + *dst, VmValue::Bool(value));
                }
                RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
                    let value = eval_numeric_compare(
                        RegIntCompare::GreaterEqual,
                        self.reg(base + *lhs),
                        self.reg(base + *rhs),
                    )?;
                    self.set_reg(base + *dst, VmValue::Bool(value));
                }
                RegInstr::Equal { dst, lhs, rhs } => {
                    let eq = self.reg(base + *lhs) == self.reg(base + *rhs);
                    self.set_reg(base + *dst, VmValue::Bool(eq));
                }
                RegInstr::NotEqual { dst, lhs, rhs } => {
                    let ne = self.reg(base + *lhs) != self.reg(base + *rhs);
                    self.set_reg(base + *dst, VmValue::Bool(ne));
                }
                RegInstr::Jump { target } => ip = *target,
                RegInstr::JumpIfBool {
                    cond,
                    expected,
                    target,
                } => {
                    if expect_bool_ref(self.reg(base + *cond))? == *expected {
                        ip = *target;
                    }
                }
                RegInstr::JumpIfIntCompare {
                    lhs,
                    rhs,
                    op,
                    expected,
                    target,
                } => {
                    let l = self.reg(base + *lhs);
                    let r = self.reg(base + *rhs);
                    if eval_numeric_compare(*op, l, r)? == *expected {
                        ip = *target;
                    }
                }
                RegInstr::MatchOption {
                    src,
                    some_ip,
                    none_ip,
                } => match self.reg(base + *src) {
                    VmValue::OptionSome(_) => ip = *some_ip,
                    VmValue::OptionNone => ip = *none_ip,
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Option match expected Option, got `{}`.",
                            other.display()
                        )));
                    }
                },
                RegInstr::MatchResult { src, ok_ip, err_ip } => match self.reg(base + *src) {
                    VmValue::Variant(data) if data.name.as_ref() == "Ok" => ip = *ok_ip,
                    VmValue::Variant(data) if data.name.as_ref() == "Err" => ip = *err_ip,
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Result match expected Result, got `{}`.",
                            other.display()
                        )));
                    }
                },
                RegInstr::MatchVariant {
                    src,
                    expected,
                    match_ip,
                    else_ip,
                } => match self.reg(base + *src) {
                    VmValue::Variant(data) if data.name.as_ref() == expected.as_str() => {
                        ip = *match_ip
                    }
                    VmValue::Variant(_) => ip = *else_ip,
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "reg VM variant match expected `{expected}`, got `{}`.",
                            other.display()
                        )));
                    }
                },
                RegInstr::RuntimeError { message } => {
                    return Err(EvalError::Runtime(message.clone()));
                }
                RegInstr::MatchMapGet {
                    map,
                    key,
                    value_dst,
                    some_ip,
                    none_ip,
                } => {
                    let map = expect_map_ref(self.reg(base + *map))?;
                    let key = map_key_from_value(self.reg(base + *key))?;
                    if let Some(value) = map.borrow().get(&key).cloned() {
                        self.set_reg(base + *value_dst, value);
                        ip = *some_ip;
                    } else {
                        ip = *none_ip;
                    }
                }
                RegInstr::UnwrapSome { dst, src } => {
                    let value = match self.reg(base + *src) {
                        VmValue::OptionSome(value) => (**value).clone(),
                        other => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM Some binding expected Some, got `{}`.",
                                other.display()
                            )));
                        }
                    };
                    self.set_reg(base + *dst, value);
                }
                RegInstr::UnwrapVariantValue { dst, src, expected } => {
                    let value = match self.reg(base + *src) {
                        VmValue::Variant(data) if data.name.as_ref() == expected.as_str() => data
                            .fields
                            .get("value")
                            .cloned()
                            .or_else(|| {
                                (data.fields.len() == 1)
                                    .then(|| data.fields.values().next().cloned())
                                    .flatten()
                            })
                            .ok_or_else(|| {
                                EvalError::Runtime(format!(
                                    "reg VM `{expected}` variant is missing value."
                                ))
                            })?,
                        other => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM expected `{expected}` variant, got `{}`.",
                                other.display()
                            )));
                        }
                    };
                    self.set_reg(base + *dst, value);
                }
                RegInstr::MakeClosure {
                    dst,
                    function: callee,
                    captures,
                } => {
                    let mut captured = Vec::with_capacity(captures.len());
                    for reg in captures {
                        captured.push(self.reg(base + *reg).clone());
                    }
                    self.set_reg(
                        base + *dst,
                        VmValue::Closure(Rc::new(VmClosure {
                            function: *callee,
                            captures: captured,
                        })),
                    );
                }
                RegInstr::MakeSome { dst, value } => {
                    let value = self.reg(base + *value).clone();
                    self.set_reg(base + *dst, VmValue::OptionSome(Box::new(value)));
                }
                RegInstr::LoadNone { dst } => {
                    self.set_reg(base + *dst, VmValue::OptionNone);
                }
                RegInstr::CallKnown {
                    dst,
                    function: callee_id,
                    args,
                } => {
                    let callee = &unit.functions[*callee_id];
                    self.prepare_frame(next_base, callee.regs);
                    for (index, reg) in args.iter().enumerate() {
                        let value = self.reg(base + *reg).clone();
                        self.set_reg(next_base + index, value);
                    }
                    let result = self.run_frame(unit, callee, next_base)?;
                    self.set_reg(base + *dst, result);
                }
                RegInstr::CallNative { dst, key, args } => {
                    let result = self.call_native_key(key, args, base)?;
                    self.set_reg(base + *dst, result);
                }
                RegInstr::CallClosure { dst, closure, args } => {
                    let closure = match self.reg(base + *closure) {
                        VmValue::Closure(closure) => Rc::clone(closure),
                        other => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM expected Closure, got `{}`.",
                                other.display()
                            )));
                        }
                    };
                    let result =
                        self.call_closure_from_regs(unit, &closure, args, base, next_base)?;
                    self.set_reg(base + *dst, result);
                }
                RegInstr::ListFilter {
                    dst,
                    list,
                    predicate,
                } => {
                    let list = expect_list_ref(self.reg(base + *list))?;
                    let predicate = expect_closure_rc(self.reg(base + *predicate))?;
                    let result = self.filter_list(unit, list, &predicate, next_base)?;
                    self.set_reg(base + *dst, result);
                }
                RegInstr::ListFold {
                    dst,
                    list,
                    state,
                    folder,
                } => {
                    let list = expect_list_ref(self.reg(base + *list))?;
                    let state = self.reg(base + *state).clone();
                    let folder = expect_closure_rc(self.reg(base + *folder))?;
                    let result = self.fold_list(unit, list, state, &folder, next_base)?;
                    self.set_reg(base + *dst, result);
                }
                RegInstr::ListGet { dst, list, index } => {
                    let list = expect_list_ref(self.reg(base + *list))?;
                    let index = expect_usize_ref(self.reg(base + *index))?;
                    let value = list.borrow().get(index).cloned().ok_or_else(|| {
                        EvalError::Runtime(format!("reg VM List.get index {index} out of bounds."))
                    })?;
                    self.set_reg(base + *dst, value);
                }
                RegInstr::ListLen { dst, list } => {
                    let len = expect_list_ref(self.reg(base + *list))?.borrow().len();
                    self.set_reg(base + *dst, VmValue::Int(len as i64));
                }
                RegInstr::ListMap { dst, list, mapper } => {
                    let list = expect_list_ref(self.reg(base + *list))?;
                    let mapper = expect_closure_rc(self.reg(base + *mapper))?;
                    let result = self.map_list(unit, list, &mapper, next_base)?;
                    self.set_reg(base + *dst, result);
                }
                RegInstr::ListPush { dst, list, value } => {
                    let list = expect_list_ref(self.reg(base + *list))?;
                    let value = self.reg(base + *value).clone();
                    list.borrow_mut().push(value);
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::ListAppend { dst, list, values } => {
                    let list_reg = base + *list;
                    let mut list_values = expect_list_ref(self.reg(list_reg))?.borrow().clone();
                    let append_values = expect_list_ref(self.reg(base + *values))?.borrow().clone();
                    list_values.extend(append_values);
                    self.set_reg(list_reg, VmValue::List(Rc::new(RefCell::new(list_values))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::ListClear { dst, list } => {
                    let list_reg = base + *list;
                    expect_list_ref(self.reg(list_reg))?;
                    self.set_reg(list_reg, VmValue::List(Rc::new(RefCell::new(Vec::new()))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::ListPop { dst, list } => {
                    let list_reg = base + *list;
                    let mut values = expect_list_ref(self.reg(list_reg))?.borrow().clone();
                    let value = values
                        .pop()
                        .map(|value| VmValue::OptionSome(Box::new(value)))
                        .unwrap_or(VmValue::OptionNone);
                    self.set_reg(list_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, value);
                }
                RegInstr::ListRemoveAt { dst, list, index } => {
                    let list_reg = base + *list;
                    let mut values = expect_list_ref(self.reg(list_reg))?.borrow().clone();
                    let index = expect_int_ref(self.reg(base + *index))?;
                    let value = if index < 0 || index as usize >= values.len() {
                        VmValue::OptionNone
                    } else {
                        VmValue::OptionSome(Box::new(values.remove(index as usize)))
                    };
                    self.set_reg(list_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, value);
                }
                RegInstr::ListSet {
                    dst,
                    list,
                    index,
                    value,
                } => {
                    let list_reg = base + *list;
                    let mut values = expect_list_ref(self.reg(list_reg))?.borrow().clone();
                    let index = expect_int_ref(self.reg(base + *index))?;
                    if index < 0 || index as usize >= values.len() {
                        return Err(EvalError::Runtime(format!(
                            "reg VM List.set index {index} out of bounds for length {}.",
                            values.len()
                        )));
                    }
                    values[index as usize] = self.reg(base + *value).clone();
                    self.set_reg(list_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::ListSort { dst, list } => {
                    let list_reg = base + *list;
                    let mut values = expect_list_ref(self.reg(list_reg))?.borrow().clone();
                    sort_vm_values(&mut values)?;
                    self.set_reg(list_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::ListSortBy {
                    dst,
                    list,
                    key,
                    compare,
                } => {
                    let values = expect_list_ref(self.reg(base + *list))?.borrow().clone();
                    let key = expect_closure_rc(self.reg(base + *key))?;
                    let compare = expect_closure_rc(self.reg(base + *compare))?;
                    let sorted =
                        self.sort_list_by_closure(unit, values, &key, &compare, next_base)?;
                    self.set_reg(base + *dst, VmValue::List(Rc::new(RefCell::new(sorted))));
                }
                RegInstr::ListSortWith { dst, list, compare } => {
                    let list_reg = base + *list;
                    let mut values = expect_list_ref(self.reg(list_reg))?.borrow().clone();
                    let compare = expect_closure_rc(self.reg(base + *compare))?;
                    self.sort_list_with_closure(unit, &mut values, &compare, next_base)?;
                    self.set_reg(list_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::DequeClear { dst, deque } => {
                    let deque_reg = base + *deque;
                    expect_list_ref(self.reg(deque_reg))?;
                    self.set_reg(deque_reg, VmValue::List(Rc::new(RefCell::new(Vec::new()))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::DequePopBack { dst, deque } => {
                    let deque_reg = base + *deque;
                    let mut values = expect_list_ref(self.reg(deque_reg))?.borrow().clone();
                    let value = values
                        .pop()
                        .map(|value| VmValue::OptionSome(Box::new(value)))
                        .unwrap_or(VmValue::OptionNone);
                    self.set_reg(deque_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, value);
                }
                RegInstr::DequePopFront { dst, deque } => {
                    let deque_reg = base + *deque;
                    let mut values = expect_list_ref(self.reg(deque_reg))?.borrow().clone();
                    let value = if values.is_empty() {
                        VmValue::OptionNone
                    } else {
                        VmValue::OptionSome(Box::new(values.remove(0)))
                    };
                    self.set_reg(deque_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, value);
                }
                RegInstr::DequePushBack { dst, deque, value } => {
                    let deque_reg = base + *deque;
                    let mut values = expect_list_ref(self.reg(deque_reg))?.borrow().clone();
                    let value = self.reg(base + *value).clone();
                    values.push(value);
                    self.set_reg(deque_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::DequePushFront { dst, deque, value } => {
                    let deque_reg = base + *deque;
                    let mut values = expect_list_ref(self.reg(deque_reg))?.borrow().clone();
                    let value = self.reg(base + *value).clone();
                    values.insert(0, value);
                    self.set_reg(deque_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::SetClear { dst, set } => {
                    let set_reg = base + *set;
                    expect_list_ref(self.reg(set_reg))?;
                    self.set_reg(set_reg, VmValue::List(Rc::new(RefCell::new(Vec::new()))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::SetForEach { dst, set, callback } => {
                    let set = expect_list_ref(self.reg(base + *set))?;
                    let callback = expect_closure_rc(self.reg(base + *callback))?;
                    let len = set.borrow().len();
                    for index in 0..len {
                        let value = set.borrow()[index].clone();
                        let _ = self.call_closure_one(unit, &callback, value, next_base)?;
                    }
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::SetInsert { dst, set, value } => {
                    let set_reg = base + *set;
                    let mut values = expect_list_ref(self.reg(set_reg))?.borrow().clone();
                    let value = self.reg(base + *value).clone();
                    let inserted = set_insert_vm(&mut values, value);
                    self.set_reg(set_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Bool(inserted));
                }
                RegInstr::SetRemove { dst, set, value } => {
                    let set_reg = base + *set;
                    let mut values = expect_list_ref(self.reg(set_reg))?.borrow().clone();
                    let removed = set_remove_vm(&mut values, self.reg(base + *value));
                    self.set_reg(set_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Bool(removed));
                }
                RegInstr::SortedSetClear { dst, set } => {
                    let set_reg = base + *set;
                    expect_list_ref(self.reg(set_reg))?;
                    self.set_reg(set_reg, VmValue::List(Rc::new(RefCell::new(Vec::new()))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::SortedSetInsert { dst, set, value } => {
                    let set_reg = base + *set;
                    let mut values = expect_list_ref(self.reg(set_reg))?.borrow().clone();
                    let value = self.reg(base + *value).clone();
                    let inserted = set_insert_vm(&mut values, value);
                    sort_vm_values(&mut values)?;
                    self.set_reg(set_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Bool(inserted));
                }
                RegInstr::SortedSetRemove { dst, set, value } => {
                    let set_reg = base + *set;
                    let mut values = expect_list_ref(self.reg(set_reg))?.borrow().clone();
                    let removed = set_remove_vm(&mut values, self.reg(base + *value));
                    self.set_reg(set_reg, VmValue::List(Rc::new(RefCell::new(values))));
                    self.set_reg(base + *dst, VmValue::Bool(removed));
                }
                RegInstr::SortedMapClear { dst, map } => {
                    let map_reg = base + *map;
                    expect_sorted_map_entries(self.reg(map_reg))?;
                    self.set_reg(map_reg, sorted_map_value(Vec::new()));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::SortedMapInsert {
                    dst,
                    map,
                    key,
                    value,
                } => {
                    let map_reg = base + *map;
                    let mut entries = expect_sorted_map_entries(self.reg(map_reg))?;
                    sorted_map_insert(
                        &mut entries,
                        self.reg(base + *key).clone(),
                        self.reg(base + *value).clone(),
                    );
                    sort_vm_map_entries(&mut entries)?;
                    self.set_reg(map_reg, sorted_map_value(entries));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::SortedMapRemove { dst, map, key } => {
                    let map_reg = base + *map;
                    let mut entries = expect_sorted_map_entries(self.reg(map_reg))?;
                    let removed = sorted_map_remove(&mut entries, self.reg(base + *key));
                    self.set_reg(map_reg, sorted_map_value(entries));
                    self.set_reg(
                        base + *dst,
                        removed
                            .map(|value| VmValue::OptionSome(Box::new(value)))
                            .unwrap_or(VmValue::OptionNone),
                    );
                }
                RegInstr::MapGet { dst, map, key } => {
                    let map = expect_map_ref(self.reg(base + *map))?;
                    let key = map_key_from_value(self.reg(base + *key))?;
                    let value = map
                        .borrow()
                        .get(&key)
                        .cloned()
                        .map(|value| VmValue::OptionSome(Box::new(value)))
                        .unwrap_or(VmValue::OptionNone);
                    self.set_reg(base + *dst, value);
                }
                RegInstr::MapClear { dst, map } => {
                    let map_reg = base + *map;
                    expect_map_ref(self.reg(map_reg))?;
                    self.set_reg(map_reg, VmValue::Map(Rc::new(RefCell::new(HashMap::new()))));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::BufferClear { dst, buffer } => {
                    expect_bytes_ref(self.reg(base + *buffer))?;
                    self.set_reg(base + *buffer, VmValue::Bytes(Rc::new(Vec::new())));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::CounterAdd {
                    dst,
                    counter,
                    amount,
                } => {
                    let counter_reg = base + *counter;
                    let value = expect_counter_value(self.reg(counter_reg))?
                        + expect_int_ref(self.reg(base + *amount))?;
                    self.set_reg(counter_reg, counter_value(value));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::ConfigStoreReplace { dst, store, value } => {
                    let store_reg = base + *store;
                    let name = expect_config_value_name(self.reg(base + *value))?;
                    self.set_reg(store_reg, config_store_value(name));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::GlobalConfigReplace { dst, global, value } => {
                    let global_reg = base + *global;
                    let rule_count = expect_config_rule_count(self.reg(base + *value))?;
                    self.set_reg(global_reg, global_config_value(rule_count));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::MapInsert {
                    dst,
                    map,
                    key,
                    value,
                } => {
                    let map = expect_map_ref(self.reg(base + *map))?;
                    let key = map_key_from_value(self.reg(base + *key))?;
                    let value = self.reg(base + *value).clone();
                    map.borrow_mut().insert(key, value);
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::MapInsertOld {
                    dst,
                    map,
                    key,
                    value,
                } => {
                    let map = expect_map_ref(self.reg(base + *map))?;
                    let key = map_key_from_value(self.reg(base + *key))?;
                    let value = self.reg(base + *value).clone();
                    let old = map.borrow_mut().insert(key, value);
                    self.set_reg(
                        base + *dst,
                        old.map(|value| VmValue::OptionSome(Box::new(value)))
                            .unwrap_or(VmValue::OptionNone),
                    );
                }
                RegInstr::MapRemove { dst, map, key } => {
                    let map = expect_map_ref(self.reg(base + *map))?;
                    let key = map_key_from_value(self.reg(base + *key))?;
                    let old = map.borrow_mut().remove(&key);
                    self.set_reg(
                        base + *dst,
                        old.map(|value| VmValue::OptionSome(Box::new(value)))
                            .unwrap_or(VmValue::OptionNone),
                    );
                }
                RegInstr::StringBuilderPush {
                    dst,
                    builder,
                    value,
                } => {
                    let mut builder_value =
                        expect_string_ref(self.reg(base + *builder))?.to_string();
                    let value = expect_string_ref(self.reg(base + *value))?;
                    builder_value.push_str(value);
                    self.set_reg(base + *builder, VmValue::string(builder_value));
                    self.set_reg(base + *dst, VmValue::Unit);
                }
                RegInstr::StringConcat { dst, left, right } => {
                    let value = {
                        let left = expect_string_ref(self.reg(base + *left))?;
                        let right = expect_string_ref(self.reg(base + *right))?;
                        let mut value = String::with_capacity(left.len() + right.len());
                        value.push_str(left);
                        value.push_str(right);
                        value
                    };
                    self.set_reg(base + *dst, VmValue::string(value));
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic,
                    args,
                } => {
                    let value = self.call_intrinsic(unit, *intrinsic, args, base, next_base)?;
                    self.set_reg(base + *dst, value);
                }
                RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic,
                    type_arg,
                    args,
                } => {
                    let value =
                        self.call_typed_intrinsic(unit, *intrinsic, type_arg, args, base)?;
                    self.set_reg(base + *dst, value);
                }
                RegInstr::TryResult { dst, src, cleanup } => {
                    let value = self.reg(base + *src);
                    let result = result_variant_payload(value)?;
                    match result {
                        Ok(value) => self.set_reg(base + *dst, value),
                        Err(error) => {
                            for resource in cleanup {
                                let value = self.reg(base + *resource).clone();
                                self.run_resource_drop(unit, value, next_base)?;
                            }
                            return Ok(value_err(error));
                        }
                    }
                }
                RegInstr::Return { src } => {
                    return Ok(self.take_reg(base + *src));
                }
            }
        }
        Ok(VmValue::Unit)
    }

    fn run_resource_drop(
        &mut self,
        unit: &RegUnit,
        value: VmValue,
        base: usize,
    ) -> Result<(), EvalError> {
        if self.finish_resource_pool_lease(value.clone())? {
            return Ok(());
        }
        let VmValue::Struct(data) = value else {
            return Ok(());
        };
        let Some(function_id) = unit
            .resource_drop_functions
            .get(data.name.as_ref())
            .copied()
        else {
            return Ok(());
        };
        let callee = &unit.functions[function_id];
        self.prepare_frame(base, callee.regs);
        for (field, value) in &data.fields {
            if let Some(reg) = callee.local_regs.get(field) {
                self.set_reg(base + *reg, value.clone());
            }
        }
        let result = self.run_frame(unit, callee, base)?;
        if matches!(result, VmValue::Unit) {
            Ok(())
        } else {
            Err(EvalError::Runtime(format!(
                "resource drop for `{}` returned unsupported value `{}`.",
                data.name,
                result.display()
            )))
        }
    }

    fn tcp_connect(&mut self, host: &str, port: i64) -> Result<VmValue, VmValue> {
        if port <= 0 || port > u16::MAX as i64 {
            return Err(tcp_error_value("TCP port must be between 1 and 65535"));
        }
        let stream = TcpStream::connect(format!("{host}:{port}")).map_err(|error| {
            tcp_error_value(format!("TCP connect to `{host}:{port}` failed: {error}"))
        })?;
        let timeout = Some(std::time::Duration::from_secs(5));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);
        let id = self.next_tcp_stream_id;
        self.next_tcp_stream_id = self.next_tcp_stream_id.saturating_add(1);
        self.tcp_streams.insert(id, stream);
        Ok(tcp_stream_value(id))
    }

    fn tcp_stream_mut(&mut self, id: i64) -> Result<&mut TcpStream, VmValue> {
        self.tcp_streams
            .get_mut(&id)
            .ok_or_else(|| tcp_error_value(format!("unknown TcpStream id `{id}`")))
    }

    fn tcp_stream_read(&mut self, id: i64, max_bytes: i64) -> Result<Vec<u8>, VmValue> {
        if max_bytes <= 0 {
            return Err(tcp_error_value("TCP read max_bytes must be positive"));
        }
        let mut buffer = vec![0; max_bytes as usize];
        let read = self
            .tcp_stream_mut(id)?
            .read(&mut buffer)
            .map_err(|error| tcp_error_value(format!("TCP read failed: {error}")))?;
        buffer.truncate(read);
        Ok(buffer)
    }

    fn tcp_stream_write(&mut self, id: i64, data: &[u8]) -> Result<i64, VmValue> {
        self.tcp_stream_mut(id)?
            .write(data)
            .map(|written| written as i64)
            .map_err(|error| tcp_error_value(format!("TCP write failed: {error}")))
    }

    fn tcp_stream_write_all(&mut self, id: i64, data: &[u8]) -> Result<(), VmValue> {
        self.tcp_stream_mut(id)?
            .write_all(data)
            .map_err(|error| tcp_error_value(format!("TCP write_all failed: {error}")))
    }

    fn tcp_stream_shutdown(&mut self, id: i64) -> Result<(), VmValue> {
        self.tcp_stream_mut(id)?
            .shutdown(Shutdown::Both)
            .map_err(|error| tcp_error_value(format!("TCP shutdown failed: {error}")))
    }

    fn websocket_connect(&mut self, url: &str) -> Result<VmValue, VmValue> {
        let (host_port, path) = parse_ws_url(url)?;
        let mut stream = TcpStream::connect(&host_port).map_err(|error| {
            websocket_error_value(format!("WebSocket connect to `{url}` failed: {error}"))
        })?;
        let timeout = Some(std::time::Duration::from_secs(5));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);
        let key = "cnNzY3JpcHQtcmVnLXZt";
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).map_err(|error| {
            websocket_error_value(format!("WebSocket handshake write failed: {error}"))
        })?;
        let mut response = Vec::new();
        let mut byte = [0; 1];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).map_err(|error| {
                websocket_error_value(format!("WebSocket handshake read failed: {error}"))
            })?;
            response.push(byte[0]);
            if response.len() > 8192 {
                return Err(websocket_error_value(
                    "WebSocket handshake response is too large",
                ));
            }
        }
        let response_text = String::from_utf8_lossy(&response);
        if !response_text.starts_with("HTTP/1.1 101 ")
            && !response_text.starts_with("HTTP/1.0 101 ")
        {
            return Err(websocket_error_value(format!(
                "WebSocket handshake failed: {}",
                response_text.lines().next().unwrap_or("")
            )));
        }
        let id = self.next_websocket_id;
        self.next_websocket_id = self.next_websocket_id.saturating_add(1);
        self.websockets.insert(id, stream);
        Ok(websocket_value(id))
    }

    fn websocket_stream_mut(&mut self, id: i64) -> Result<&mut TcpStream, VmValue> {
        self.websockets
            .get_mut(&id)
            .ok_or_else(|| websocket_error_value(format!("unknown WebSocket id `{id}`")))
    }

    fn websocket_send(&mut self, id: i64, opcode: u8, payload: &[u8]) -> Result<(), VmValue> {
        websocket_write_frame(self.websocket_stream_mut(id)?, opcode, payload)
    }

    fn websocket_recv(
        &mut self,
        id: i64,
        expected: WebSocketExpectedFrame,
    ) -> Result<Option<Vec<u8>>, VmValue> {
        loop {
            let frame = websocket_read_frame(self.websocket_stream_mut(id)?)?;
            match frame.opcode {
                0x1 if matches!(expected, WebSocketExpectedFrame::Text) => {
                    return Ok(Some(frame.payload));
                }
                0x2 if matches!(expected, WebSocketExpectedFrame::Binary) => {
                    return Ok(Some(frame.payload));
                }
                0x8 => return Ok(None),
                0x9 => {
                    websocket_write_frame(self.websocket_stream_mut(id)?, 0xA, &frame.payload)?;
                }
                0xA => {}
                0x1 => {
                    return Err(websocket_error_value(
                        "WebSocket received text frame while waiting for bytes",
                    ));
                }
                0x2 => {
                    return Err(websocket_error_value(
                        "WebSocket received binary frame while waiting for text",
                    ));
                }
                opcode => {
                    return Err(websocket_error_value(format!(
                        "WebSocket received unsupported opcode {opcode}"
                    )));
                }
            }
        }
    }

    fn websocket_close(&mut self, id: i64) -> Result<(), VmValue> {
        websocket_write_frame(self.websocket_stream_mut(id)?, 0x8, &[])
    }

    fn resource_pool_new(
        &mut self,
        unit: &RegUnit,
        args: &[Reg],
        base: usize,
        next_base: usize,
        lazy: bool,
        factory_returns_result: bool,
    ) -> Result<VmValue, EvalError> {
        let factory = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 0)?)?;
        let max_size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
        let capacity = max_size.max(0);
        let mut idle = Vec::new();
        if !lazy {
            idle.reserve(capacity as usize);
            for _ in 0..capacity {
                let value = self.call_closure_zero(unit, &factory, next_base)?;
                if factory_returns_result {
                    match result_variant_payload(&value)? {
                        Ok(value) => idle.push(value),
                        Err(error) => return Ok(value_err(error)),
                    }
                } else {
                    idle.push(value);
                }
            }
        }
        let id = self.next_pool_id;
        self.next_pool_id = self.next_pool_id.saturating_add(1);
        self.pools.insert(
            id,
            VmResourcePool {
                capacity,
                created: idle.len() as i64,
                in_use: 0,
                idle,
                factory: lazy.then_some(factory),
                factory_returns_result,
            },
        );
        let pool = resource_pool_value(id);
        if factory_returns_result && !lazy {
            Ok(value_ok(pool))
        } else {
            Ok(pool)
        }
    }

    fn resource_pool_borrow(
        &mut self,
        unit: &RegUnit,
        args: &[Reg],
        base: usize,
        next_base: usize,
        fallible: bool,
    ) -> Result<VmValue, EvalError> {
        let pool = expect_resource_pool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
        let borrowed = self.resource_pool_borrow_value(unit, pool.id, next_base);
        if fallible {
            return Ok(match borrowed {
                Ok(value) => value_ok(value),
                Err(error) => value_err(error),
            });
        }
        borrowed.map_err(|error| {
            EvalError::Runtime(format!(
                "ResourcePool.borrow failed: {}",
                pool_error_message(&error).unwrap_or_else(|| error.display())
            ))
        })
    }

    fn resource_pool_borrow_value(
        &mut self,
        unit: &RegUnit,
        pool_id: i64,
        next_base: usize,
    ) -> Result<VmValue, VmValue> {
        let idle = self
            .pools
            .get_mut(&pool_id)
            .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?
            .idle
            .pop();
        let value = if let Some(value) = idle {
            value
        } else {
            let factory = {
                let state = self.pools.get(&pool_id).ok_or_else(|| {
                    pool_error_value(format!("unknown ResourcePool id `{pool_id}`"))
                })?;
                if state.created >= state.capacity {
                    return Err(pool_error_value("resource pool exhausted"));
                }
                state
                    .factory
                    .clone()
                    .ok_or_else(|| pool_error_value("resource pool exhausted"))?
            };
            let value = self
                .call_closure_zero(unit, &factory, next_base)
                .map_err(|error| {
                    pool_error_value(format!("resource pool factory failed: {error:?}"))
                })?;
            let factory_returns_result = self
                .pools
                .get(&pool_id)
                .map(|state| state.factory_returns_result)
                .unwrap_or(false);
            let value = if factory_returns_result {
                match result_variant_payload(&value) {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => return Err(error),
                    Err(error) => {
                        return Err(pool_error_value(format!(
                            "resource pool factory returned non-Result value: {error:?}"
                        )));
                    }
                }
            } else {
                value
            };
            let state = self
                .pools
                .get_mut(&pool_id)
                .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?;
            state.created = state.created.saturating_add(1);
            value
        };
        let state = self
            .pools
            .get_mut(&pool_id)
            .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?;
        state.in_use = state.in_use.saturating_add(1);
        mark_pool_lease(value, pool_id).map_err(pool_error_value)
    }

    fn finish_resource_pool_lease(&mut self, value: VmValue) -> Result<bool, EvalError> {
        let Some(lease) = split_pool_lease(value)? else {
            return Ok(false);
        };
        let state = self.pools.get_mut(&lease.pool_id).ok_or_else(|| {
            EvalError::Runtime(format!("unknown ResourcePool id `{}`.", lease.pool_id))
        })?;
        state.in_use = state.in_use.saturating_sub(1);
        if lease.discarded {
            state.created = state.created.saturating_sub(1);
        } else {
            state.idle.push(lease.value);
        }
        Ok(true)
    }

    fn call_native_key(&self, key: &str, args: &[Reg], base: usize) -> Result<VmValue, EvalError> {
        let Some(function) = self.native_bindings.get(key).copied() else {
            return Err(EvalError::Runtime(format!(
                "reg VM native function `{key}` has no host binding."
            )));
        };
        let args = args
            .iter()
            .map(|reg| native_value_from_vm_value(self.reg(base + *reg).clone()))
            .collect::<Result<Vec<_>, _>>()?;
        function(args)
            .map(vm_value_from_native_value)
            .map_err(|error| EvalError::Runtime(format!("native host binding failed: {error}")))
    }

    fn call_closure_from_regs(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        arg_regs: &[Reg],
        caller_base: usize,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = &unit.functions[closure.function];
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        for (index, reg) in arg_regs.iter().enumerate() {
            let value = self.reg(caller_base + *reg).clone();
            self.set_reg(base + offset + index, value);
        }
        self.run_frame(unit, callee, base)
    }

    fn call_closure_one(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        arg: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = &unit.functions[closure.function];
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        self.set_reg(base + closure.captures.len(), arg);
        self.run_frame(unit, callee, base)
    }

    fn call_closure_two(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        first: VmValue,
        second: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = &unit.functions[closure.function];
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        self.set_reg(base + offset, first);
        self.set_reg(base + offset + 1, second);
        self.run_frame(unit, callee, base)
    }

    fn call_closure_three(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        first: VmValue,
        second: VmValue,
        third: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = &unit.functions[closure.function];
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        self.set_reg(base + offset, first);
        self.set_reg(base + offset + 1, second);
        self.set_reg(base + offset + 2, third);
        self.run_frame(unit, callee, base)
    }

    fn call_closure_zero(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = &unit.functions[closure.function];
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        self.run_frame(unit, callee, base)
    }

    fn channel_state_mut(&mut self, id: i64) -> Result<&mut VmChannel, EvalError> {
        self.channels
            .get_mut(&id)
            .ok_or_else(|| EvalError::Runtime(format!("unknown channel id `{id}`.")))
    }

    fn channel_send(&mut self, sender: VmSender, value: VmValue) -> Result<VmValue, VmValue> {
        if sender.closed {
            return Err(channel_error_value("channel sender closed"));
        }
        let state = self.channels.get_mut(&sender.channel_id).ok_or_else(|| {
            channel_error_value(format!("unknown channel id `{}`", sender.channel_id))
        })?;
        if state.receiver_closed {
            return Err(channel_error_value("channel closed"));
        }
        if state.queue.len() >= state.capacity {
            return Err(channel_error_value(
                "channel send would block on a full channel in the VM",
            ));
        }
        state.queue.push_back(value);
        Ok(VmValue::Unit)
    }

    fn channel_recv(&mut self, channel_id: i64) -> Result<VmValue, VmValue> {
        let state = self
            .channels
            .get_mut(&channel_id)
            .ok_or_else(|| channel_error_value(format!("unknown channel id `{channel_id}`")))?;
        if let Some(value) = state.queue.pop_front() {
            return Ok(VmValue::OptionSome(Box::new(value)));
        }
        if state.senders == 0 {
            return Ok(VmValue::OptionNone);
        }
        Err(channel_error_value(
            "channel recv would block on an open empty channel in the VM",
        ))
    }

    fn filter_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<Vec<VmValue>>>,
        predicate: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let len = list.borrow().len();
        let mut filtered = Vec::with_capacity(len);
        for index in 0..len {
            let item = list_item_at(&list, index, "List.filter")?;
            let keep = self.call_closure_one(unit, predicate, item.clone(), base)?;
            if expect_bool_ref(&keep)? {
                filtered.push(item);
            }
        }
        Ok(VmValue::List(Rc::new(RefCell::new(filtered))))
    }

    fn fold_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<Vec<VmValue>>>,
        mut state: VmValue,
        folder: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let len = list.borrow().len();
        for index in 0..len {
            let item = list_item_at(&list, index, "List.fold")?;
            state = self.call_closure_two(unit, folder, state, item, base)?;
        }
        Ok(state)
    }

    fn map_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<Vec<VmValue>>>,
        mapper: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let len = list.borrow().len();
        let mut mapped = Vec::with_capacity(len);
        for index in 0..len {
            let item = list_item_at(&list, index, "List.map")?;
            mapped.push(self.call_closure_one(unit, mapper, item, base)?);
        }
        Ok(VmValue::List(Rc::new(RefCell::new(mapped))))
    }

    fn sort_list_with_closure(
        &mut self,
        unit: &RegUnit,
        list: &mut [VmValue],
        compare: &VmClosure,
        base: usize,
    ) -> Result<(), EvalError> {
        for right_index in 1..list.len() {
            let mut index = right_index;
            while index > 0 {
                let ordering = self.call_closure_two(
                    unit,
                    compare,
                    list[index - 1].clone(),
                    list[index].clone(),
                    base,
                )?;
                if expect_int_ref(&ordering)? <= 0 {
                    break;
                }
                list.swap(index - 1, index);
                index -= 1;
            }
        }
        Ok(())
    }

    fn sort_list_by_closure(
        &mut self,
        unit: &RegUnit,
        mut list: Vec<VmValue>,
        key: &VmClosure,
        compare: &VmClosure,
        base: usize,
    ) -> Result<Vec<VmValue>, EvalError> {
        for right_index in 1..list.len() {
            let mut index = right_index;
            while index > 0 {
                let left_key = self.call_closure_one(unit, key, list[index - 1].clone(), base)?;
                let right_key = self.call_closure_one(unit, key, list[index].clone(), base)?;
                let ordering = self.call_closure_two(unit, compare, left_key, right_key, base)?;
                if expect_int_ref(&ordering)? <= 0 {
                    break;
                }
                list.swap(index - 1, index);
                index -= 1;
            }
        }
        Ok(list)
    }

    fn call_typed_intrinsic(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        type_arg: &str,
        args: &[Reg],
        base: usize,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            RegIntrinsic::JsonDecode => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_decode_struct_value(unit, type_arg, value)))
            }
            RegIntrinsic::JsonDecodeText => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(parse_json_text(text).and_then(|value| {
                    json_decode_struct_value(unit, type_arg, &value)
                })))
            }
            other => Err(EvalError::Runtime(format!(
                "reg VM typed intrinsic `{other:?}` is not implemented."
            ))),
        }
    }

    fn call_intrinsic(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            RegIntrinsic::ArgsAll => Ok(VmValue::List(Rc::new(RefCell::new(
                self.args.iter().cloned().map(VmValue::string).collect(),
            )))),
            RegIntrinsic::ArgsCount => Ok(VmValue::Int(self.args.len() as i64)),
            RegIntrinsic::ArgsGet => {
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(usize::try_from(index)
                    .ok()
                    .and_then(|index| self.args.get(index).cloned())
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::string(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ArgsGetOrDefault => {
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let default =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(VmValue::string(
                    usize::try_from(index)
                        .ok()
                        .and_then(|index| self.args.get(index).cloned())
                        .unwrap_or(default),
                ))
            }
            RegIntrinsic::AssertEqual => {
                let left = intrinsic_arg(&self.stack, base, args, 0)?;
                let right = intrinsic_arg(&self.stack, base, args, 1)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{}` does not equal right `{}`.",
                        left.display(),
                        right.display()
                    )))
                }
            }
            RegIntrinsic::AssertEqualBool => {
                let left = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` does not equal right `{right}`."
                    )))
                }
            }
            RegIntrinsic::AssertEqualInt => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` does not equal right `{right}`."
                    )))
                }
            }
            RegIntrinsic::Base64Decode => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    base64::engine::general_purpose::STANDARD
                        .decode(text)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::Base64DecodeString => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|error| decode_error_value(error.to_string()))
                    .and_then(|bytes| {
                        String::from_utf8(bytes)
                            .map(VmValue::string)
                            .map_err(|error| decode_error_value(error.to_string()))
                    });
                Ok(json_result(result))
            }
            RegIntrinsic::Base64Encode => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    base64::engine::general_purpose::STANDARD.encode(value.as_bytes()),
                ))
            }
            RegIntrinsic::Base64EncodeBytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    base64::engine::general_purpose::STANDARD.encode(value),
                ))
            }
            RegIntrinsic::BytesConcat => {
                let left = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut bytes = Vec::with_capacity(left.len() + right.len());
                bytes.extend_from_slice(left);
                bytes.extend_from_slice(right);
                Ok(VmValue::Bytes(Rc::new(bytes)))
            }
            RegIntrinsic::BytesConsume => {
                expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::BytesFromString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(value.as_bytes().to_vec())))
            }
            RegIntrinsic::BytesIsEmpty => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_empty()))
            }
            RegIntrinsic::BytesLen => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value.len() as i64))
            }
            RegIntrinsic::BytesSlice => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::Bytes(Rc::new(bytes_slice(value, start, len))))
            }
            RegIntrinsic::BytesViewStartsWith => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.starts_with(prefix)))
            }
            RegIntrinsic::BytesViewToBytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(value.to_vec())))
            }
            RegIntrinsic::BufferNew => {
                let size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(Vec::with_capacity(
                    size.max(0) as usize
                ))))
            }
            RegIntrinsic::CacheGet => {
                let cache = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bytes = cache
                    .borrow()
                    .values()
                    .find_map(|value| expect_string_ref(value).ok().map(str::to_owned))
                    .map(String::into_bytes)
                    .unwrap_or_default();
                Ok(image_value(
                    bytes,
                    None,
                    None,
                    vec!["cache-get".to_string()],
                ))
            }
            RegIntrinsic::CacheLookup => {
                let cache = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(cache
                    .borrow()
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| VmValue::string("")))
            }
            RegIntrinsic::CapabilityFrom => Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone()),
            RegIntrinsic::CancellationSourceCancel => {
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationSource",
                )?;
                self.cancellation_flags.insert(id, true);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::CancellationSourceNew => {
                let id = self.next_cancellation_id;
                self.next_cancellation_id = self.next_cancellation_id.saturating_add(1);
                self.cancellation_flags.insert(id, false);
                Ok(cancellation_source_value(id))
            }
            RegIntrinsic::CancellationSourceToken => {
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationSource",
                )?;
                Ok(cancellation_token_value(id))
            }
            RegIntrinsic::CancellationTokenIsCancelled => {
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationToken",
                )?;
                Ok(VmValue::Bool(
                    self.cancellation_flags.get(&id).copied().unwrap_or(false),
                ))
            }
            RegIntrinsic::ChannelBounded => {
                let capacity = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(if capacity <= 0 {
                    Err(channel_error_value("channel capacity must be positive"))
                } else {
                    let id = self.next_channel_id;
                    self.next_channel_id = self.next_channel_id.saturating_add(1);
                    self.channels.insert(id, VmChannel::new(capacity as usize));
                    Ok(channel_value(id, capacity, false))
                }))
            }
            RegIntrinsic::ChannelSender => {
                let channel = expect_channel_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let state = self.channel_state_mut(channel.id)?;
                state.senders = state.senders.saturating_add(1);
                Ok(sender_value(channel.id, false))
            }
            RegIntrinsic::ChannelReceiver => {
                let channel_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Channel.receiver missing channel.".to_string())
                })?;
                let mut channel = expect_channel_ref(self.reg(base + channel_reg))?;
                let already_taken = self
                    .channels
                    .get(&channel.id)
                    .map(|state| state.receiver_taken)
                    .unwrap_or(channel.receiver_taken);
                Ok(json_result(if already_taken {
                    Err(channel_error_value("channel receiver already taken"))
                } else {
                    channel.receiver_taken = true;
                    self.channel_state_mut(channel.id)?.receiver_taken = true;
                    self.set_reg(base + channel_reg, channel.to_value());
                    Ok(receiver_value(channel.id, false))
                }))
            }
            RegIntrinsic::ChannelErrorMessage
            | RegIntrinsic::DecodeErrorMessage
            | RegIntrinsic::FileErrorMessage
            | RegIntrinsic::HttpErrorMessage
            | RegIntrinsic::PoolErrorMessage
            | RegIntrinsic::TcpErrorMessage
            | RegIntrinsic::WebSocketErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::CharCompare => {
                let left = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_char_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let value = match left.cmp(&right) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                };
                Ok(VmValue::Int(value))
            }
            RegIntrinsic::CharFromCode => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(u32::try_from(value)
                    .ok()
                    .and_then(char::from_u32)
                    .map(VmValue::Char)
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::CharIsAlphanumeric => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_alphanumeric()))
            }
            RegIntrinsic::CharIsAlpha => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_alphabetic()))
            }
            RegIntrinsic::CharIsDigit => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_digit()))
            }
            RegIntrinsic::CharIsLower => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_lowercase()))
            }
            RegIntrinsic::CharIsUpper => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_uppercase()))
            }
            RegIntrinsic::CharIsWhitespace => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_whitespace()))
            }
            RegIntrinsic::CharToCode => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value as u32 as i64))
            }
            RegIntrinsic::CharToLower => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Char(value.to_lowercase().next().unwrap_or(value)))
            }
            RegIntrinsic::CharToString => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::CharToUpper => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Char(value.to_uppercase().next().unwrap_or(value)))
            }
            RegIntrinsic::ClockNow => Ok(instant_value(clock_system_unix_ms())),
            RegIntrinsic::ClockSystemUnixMs => Ok(VmValue::Int(clock_system_unix_ms())),
            RegIntrinsic::ConfigLoad => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(|text| config_value(config_name_from_text(&text)))
                        .map_err(|error| config_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::ConfigName => {
                let value = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::string(expect_config_value_name(value)?))
            }
            RegIntrinsic::ConfigNew => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let rules = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(config_rules_value(name, rules.borrow().len() as i64))
            }
            RegIntrinsic::ConfigRuleCount => {
                let config = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Int(expect_config_rule_count(config)?))
            }
            RegIntrinsic::ConfigStoreName => {
                let store = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::string(expect_config_store_name(store)?))
            }
            RegIntrinsic::ConfigStoreNew => {
                let value = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(config_store_value(expect_config_value_name(value)?))
            }
            RegIntrinsic::DateAddDays => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let days = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(
                    unix_ms.saturating_add(days.saturating_mul(MS_PER_DAY)),
                ))
            }
            RegIntrinsic::DateAddMs => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(unix_ms.saturating_add(ms)))
            }
            RegIntrinsic::DateDay => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).day() as i64))
            }
            RegIntrinsic::DateDaysBetween => {
                let start_unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let end_unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(
                    end_unix_ms.saturating_sub(start_unix_ms) / MS_PER_DAY,
                ))
            }
            RegIntrinsic::DateDaysInMonth => {
                let year = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let month = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(date_days_in_month(year, month)))
            }
            RegIntrinsic::DateFormatIso => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    utc_datetime(unix_ms).to_rfc3339_opts(SecondsFormat::Millis, true),
                ))
            }
            RegIntrinsic::DateFormatYmd => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    utc_datetime(unix_ms).format("%Y-%m-%d").to_string(),
                ))
            }
            RegIntrinsic::DateHour => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).hour() as i64))
            }
            RegIntrinsic::DateIsLeapYear => {
                let year = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(date_is_leap_year(year)))
            }
            RegIntrinsic::DateMinute => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).minute() as i64))
            }
            RegIntrinsic::DateMonth => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).month() as i64))
            }
            RegIntrinsic::DateParseIso => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(date_parse_iso(value)
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Int(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::DateParseYmd => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(date_parse_ymd(value)
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Int(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::DateSecond => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).second() as i64))
            }
            RegIntrinsic::DateStartOfDay => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = utc_datetime(unix_ms)
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is valid");
                Ok(VmValue::Int(
                    Utc.from_utc_datetime(&start).timestamp_millis(),
                ))
            }
            RegIntrinsic::DateWeekday => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    utc_datetime(unix_ms).weekday().number_from_monday() as i64,
                ))
            }
            RegIntrinsic::DateYear => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).year() as i64))
            }
            RegIntrinsic::CounterNew => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(counter_value(value))
            }
            RegIntrinsic::CounterValue => {
                let counter = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Int(expect_counter_value(counter)?))
            }
            RegIntrinsic::CsvOpenRead => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::File::open(path)
                        .map(|_| file_value(path, "read", 0))
                        .map_err(|error| csv_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::CsvParseRow => {
                let buffer = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(json_result(csv_parse_row_value(
                    &expect_row_buffer_bytes_ref(buffer)?,
                )))
            }
            RegIntrinsic::CsvReadInto => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Csv.read_into missing file.".to_string())
                })?;
                let buffer_reg = *args.get(1).ok_or_else(|| {
                    EvalError::Runtime("reg VM Csv.read_into missing buffer.".to_string())
                })?;
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_read_remaining(&mut file)
                    .map_err(|error| csv_error_value(error.to_string()));
                self.set_reg(base + file_reg, file.to_value());
                Ok(match result {
                    Ok(bytes) => {
                        self.set_reg(base + buffer_reg, row_buffer_value(bytes));
                        value_ok(VmValue::Unit)
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::CsvRows => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _buffer_size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    csv_rows_stream_value(path).map_err(csv_error_value),
                ))
            }
            RegIntrinsic::DeadlineAfter | RegIntrinsic::DeadlineAfterMs => {
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(deadline_value(deadline_after_ms(ms)))
            }
            RegIntrinsic::DeadlineIsExpired => {
                let deadline = expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(clock_system_unix_ms() >= deadline))
            }
            RegIntrinsic::DeadlineRemainingMs => {
                let deadline = expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    deadline.saturating_sub(clock_system_unix_ms()).max(0),
                ))
            }
            RegIntrinsic::DequeIsEmpty => {
                let deque = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(deque.borrow().is_empty()))
            }
            RegIntrinsic::DequeLen => {
                let deque = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(deque.borrow().len() as i64))
            }
            RegIntrinsic::DequeNew => Ok(VmValue::List(Rc::new(RefCell::new(Vec::new())))),
            RegIntrinsic::DequeToList => {
                let deque = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(deque.borrow().clone()))))
            }
            RegIntrinsic::DiffUnified => {
                let old = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let new = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(diff_unified_string(old, new)))
            }
            RegIntrinsic::DirectoryCopyFile => {
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::copy(from, to).map(|_| ())))
            }
            RegIntrinsic::DirectoryCreate => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::create_dir(path)))
            }
            RegIntrinsic::DirectoryCreateAll | RegIntrinsic::DirectoryCreateDirAll => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::create_dir_all(path)))
            }
            RegIntrinsic::DirectoryExists => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).exists()))
            }
            RegIntrinsic::DirectoryIsDir => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_dir()))
            }
            RegIntrinsic::DirectoryIsFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_file()))
            }
            RegIntrinsic::DirectoryListFiles => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    directory_list_files(Path::new(path))
                        .map(|files| {
                            VmValue::List(Rc::new(RefCell::new(
                                files.into_iter().map(VmValue::string).collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::DirectoryListPaths => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    directory_list_paths(Path::new(path))
                        .map(|paths| {
                            VmValue::List(Rc::new(RefCell::new(
                                paths
                                    .into_iter()
                                    .map(|path| VmValue::string(path.to_string_lossy()))
                                    .collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::DirectoryMetadata => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::metadata(path)
                        .map(file_metadata_value)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::DirectoryReadString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::DirectoryRemoveDirAll => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::remove_dir_all(path)))
            }
            RegIntrinsic::DirectoryRemoveFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::remove_file(path)))
            }
            RegIntrinsic::DirectoryRename => {
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::rename(from, to)))
            }
            RegIntrinsic::DirectoryWriteString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let content = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, content)))
            }
            RegIntrinsic::DbClose => {
                let _ = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::DbConnectionOpen => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(db_connection_value(url, Vec::new()))
            }
            RegIntrinsic::DbConnectionQuery => {
                let conn_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM DbConnection.query missing conn.".to_string())
                })?;
                let sql =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                let mut conn = expect_db_connection_ref(self.reg(base + conn_reg))?;
                Ok(json_result(if sql.trim().is_empty() {
                    Err(db_error_value("SQL query is empty"))
                } else {
                    self.stdout
                        .push_str(&format!("db query on {}: {sql}\n", conn.url));
                    conn.queries.push(sql);
                    self.set_reg(base + conn_reg, conn.to_value());
                    Ok(VmValue::Unit)
                }))
            }
            RegIntrinsic::DbConnectionTryOpen => {
                let url =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                Ok(json_result(if url.trim().is_empty() {
                    Err(db_error_value("database URL is empty"))
                } else {
                    Ok(db_connection_value(url, Vec::new()))
                }))
            }
            RegIntrinsic::DurationAdd => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left + right))
            }
            RegIntrinsic::DurationAsMs | RegIntrinsic::DurationMs => Ok(VmValue::Int(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?,
            )),
            RegIntrinsic::DurationAsSeconds => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value / 1000))
            }
            RegIntrinsic::DurationSeconds => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value * 1000))
            }
            RegIntrinsic::EnvironmentBindFunction => {
                let env_reg = args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Environment.bind_function missing env.".to_string())
                })?;
                let function = intrinsic_arg(&self.stack, base, args, 1)?;
                let _ = expect_function_has_closure(function)?;
                let env = intrinsic_arg(&self.stack, base, args, 0)?;
                let (has_parent, _) = expect_environment_state(env)?;
                self.set_reg(base + *env_reg, environment_value(has_parent, true));
                Ok(VmValue::Unit)
            }
            RegIntrinsic::EnvironmentChild => {
                let parent = intrinsic_arg(&self.stack, base, args, 0)?;
                let _ = expect_environment_state(parent)?;
                Ok(environment_value(true, false))
            }
            RegIntrinsic::EnvironmentHasFunction => {
                let env = intrinsic_arg(&self.stack, base, args, 0)?;
                let (_, has_function) = expect_environment_state(env)?;
                Ok(VmValue::Bool(has_function))
            }
            RegIntrinsic::EnvironmentHasParent => {
                let env = intrinsic_arg(&self.stack, base, args, 0)?;
                let (has_parent, _) = expect_environment_state(env)?;
                Ok(VmValue::Bool(has_parent))
            }
            RegIntrinsic::EnvironmentRoot => Ok(environment_value(false, false)),
            RegIntrinsic::EnvCurrentDir => Ok(json_result(
                std::env::current_dir()
                    .map(|path| VmValue::string(path.to_string_lossy()))
                    .map_err(|error| file_error_value(error.to_string())),
            )),
            RegIntrinsic::EnvGet => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(std::env::var(name)
                    .ok()
                    .map(VmValue::string)
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::EnvGetOrDefault => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let default = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(
                    std::env::var(name).unwrap_or_else(|_| default.to_string()),
                ))
            }
            RegIntrinsic::EnvHomeDir => Ok(std::env::var("HOME")
                .ok()
                .filter(|value| !value.is_empty())
                .map(VmValue::string)
                .map(|value| VmValue::OptionSome(Box::new(value)))
                .unwrap_or(VmValue::OptionNone)),
            RegIntrinsic::EnvRunWorkspaceRoot => Ok(VmValue::string(env!("CARGO_MANIFEST_DIR"))),
            RegIntrinsic::EnvSet => {
                let _ = intrinsic_arg(&self.stack, base, args, 0)?;
                let _ = intrinsic_arg(&self.stack, base, args, 1)?;
                self.stderr
                    .push_str("[rsscript] warning: Env.set is a no-op in the safe runtime\n");
                Ok(VmValue::Unit)
            }
            RegIntrinsic::EnvSetCurrentDir => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::env::set_current_dir(path)))
            }
            RegIntrinsic::EnvTempDir => Ok(VmValue::string(std::env::temp_dir().to_string_lossy())),
            RegIntrinsic::FileAppendBytes => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(file_result_unit(file_append(path, &data)))
            }
            RegIntrinsic::FileAppendString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?
                    .as_bytes()
                    .to_vec();
                Ok(file_result_unit(file_append(path, &text)))
            }
            RegIntrinsic::FileBytesStream => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let chunk_size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    file_bytes_stream_value(path, chunk_size).map_err(channel_error_value),
                ))
            }
            RegIntrinsic::FileExists => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).exists()))
            }
            RegIntrinsic::FileOpen | RegIntrinsic::FileOpenRead => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::File::open(path)
                        .map(|_| file_value(path, "read", 0))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileOpenWrite => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::File::create(path)
                        .map(|_| file_value(path, "write", 0))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileReadAllAsync => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read(path)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileReadAll => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.read_all missing file.".to_string())
                })?;
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_read_remaining(&mut file)
                    .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                    .map_err(|error| file_error_value(error.to_string()));
                self.set_reg(base + file_reg, file.to_value());
                Ok(json_result(result))
            }
            RegIntrinsic::FileReadAllString => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.read_all_string missing file.".to_string())
                })?;
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_read_remaining(&mut file)
                    .and_then(|bytes| {
                        String::from_utf8(bytes).map_err(|error| {
                            std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                        })
                    })
                    .map(VmValue::string)
                    .map_err(|error| file_error_value(error.to_string()));
                self.set_reg(base + file_reg, file.to_value());
                Ok(json_result(result))
            }
            RegIntrinsic::FileReadAllStringAsync => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileReadBytes => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read(path)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileReadInto => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.read_into missing file.".to_string())
                })?;
                let buffer_reg = *args.get(1).ok_or_else(|| {
                    EvalError::Runtime("reg VM File.read_into missing buffer.".to_string())
                })?;
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_read_remaining(&mut file)
                    .map(|bytes| {
                        let did_read = !bytes.is_empty();
                        (VmValue::Bytes(Rc::new(bytes)), VmValue::Bool(did_read))
                    })
                    .map_err(|error| file_error_value(error.to_string()));
                self.set_reg(base + file_reg, file.to_value());
                Ok(match result {
                    Ok((buffer, did_read)) => {
                        self.set_reg(base + buffer_reg, buffer);
                        value_ok(did_read)
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FileReadString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileRemove => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::remove_file(path)))
            }
            RegIntrinsic::FileWrite => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.write missing file.".to_string())
                })?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_write_at_cursor(&mut file, &data);
                self.set_reg(base + file_reg, file.to_value());
                Ok(file_result_unit(result))
            }
            RegIntrinsic::FileWriteAsync | RegIntrinsic::FileWriteBytes => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(file_result_unit(std::fs::write(path, data)))
            }
            RegIntrinsic::FileWriteAtomic => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_atomic_write_result(PathBuf::from(path), text))
            }
            RegIntrinsic::FileWriteBytesView
            | RegIntrinsic::FileWriteBuffer
            | RegIntrinsic::FileWriteBufferView => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM file write missing file.".to_string())
                })?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_write_at_cursor(&mut file, &data);
                self.set_reg(base + file_reg, file.to_value());
                Ok(file_result_unit(result))
            }
            RegIntrinsic::FileWriteString => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.write_string missing file.".to_string())
                })?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?
                    .as_bytes()
                    .to_vec();
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_write_at_cursor(&mut file, &text);
                self.set_reg(base + file_reg, file.to_value());
                Ok(file_result_unit(result))
            }
            RegIntrinsic::FileWriteStringAsync => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, text)))
            }
            RegIntrinsic::FileWriteStringToPath => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, text)))
            }
            RegIntrinsic::FalliblePipelineCollect => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::FalliblePipelineMap => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut mapped = Vec::with_capacity(len);
                        for index in 0..len {
                            let value = items.borrow()[index].clone();
                            mapped.push(self.call_closure_one(unit, &mapper, value, next_base)?);
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(mapped))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineFilter => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut filtered = Vec::new();
                        for index in 0..len {
                            let value = items.borrow()[index].clone();
                            let keep =
                                self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                            if expect_bool_ref(&keep)? {
                                filtered.push(value);
                            }
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(filtered))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineEach => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let action = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let values = items.borrow().clone();
                        for value in values.iter().cloned() {
                            let _ = self.call_closure_one(unit, &action, value, next_base)?;
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(values))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineTryMap => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut mapped = Vec::with_capacity(len);
                        for index in 0..len {
                            let value = items.borrow()[index].clone();
                            match result_variant_payload(
                                &self.call_closure_one(unit, &mapper, value, next_base)?,
                            )? {
                                Ok(value) => mapped.push(value),
                                Err(error) => return Ok(value_err(error)),
                            }
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(mapped))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FunctionObjectHasClosure => {
                let function = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Bool(expect_function_has_closure(function)?))
            }
            RegIntrinsic::FunctionObjectNew => {
                let closure = intrinsic_arg(&self.stack, base, args, 0)?;
                let _ = expect_environment_state(closure)?;
                Ok(function_object_value(true))
            }
            RegIntrinsic::HashSha256Bytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(sha256_digest(value)))
            }
            RegIntrinsic::HashSha256File => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read(path)
                        .map(|bytes| VmValue::string(sha256_digest(&bytes)))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::HashSha256String => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(sha256_digest(value.as_bytes())))
            }
            RegIntrinsic::HmacSha256Bytes => {
                let key = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(hmac_sha256_digest(key, value)))
            }
            RegIntrinsic::HmacSha256String => {
                let key = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(hmac_sha256_digest(
                    key.as_bytes(),
                    value.as_bytes(),
                )))
            }
            RegIntrinsic::GlobalConfigNew => {
                let value = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(global_config_value(expect_config_rule_count(value)?))
            }
            RegIntrinsic::GlobalConfigRuleCount => {
                let global = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Int(expect_global_config_rule_count(global)?))
            }
            RegIntrinsic::HexDecode => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    hex::decode(text)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::HexEncode => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(hex::encode(value)))
            }
            RegIntrinsic::HexEncodeString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(hex::encode(value.as_bytes())))
            }
            RegIntrinsic::HttpGet => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(http_get_local(url)))
            }
            RegIntrinsic::HttpGetAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(http_get_local(url)))
            }
            RegIntrinsic::HttpGetRetryAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let attempts = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let _backoff = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                let mut last = Err(http_error_value("HTTP retry attempts must be positive"));
                for _ in 0..attempts.max(1) {
                    last = http_get_local(url);
                    if last.is_ok() {
                        break;
                    }
                }
                Ok(json_result(last))
            }
            RegIntrinsic::HttpGetTimeoutAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(http_get_local(url)))
            }
            RegIntrinsic::HttpPostForm | RegIntrinsic::HttpPostFormAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _ = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP client runtime is not configured for POST form {url}"
                ))))
            }
            RegIntrinsic::HttpPostJson | RegIntrinsic::HttpPostJsonAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _ = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP client runtime is not configured for POST JSON {url}"
                ))))
            }
            RegIntrinsic::HttpPostJsonBearerRetryAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _body = intrinsic_arg(&self.stack, base, args, 1)?;
                let _token = intrinsic_arg(&self.stack, base, args, 2)?;
                let timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                let attempts = expect_int_ref(intrinsic_arg(&self.stack, base, args, 4)?)?;
                let backoff = expect_int_ref(intrinsic_arg(&self.stack, base, args, 5)?)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP async provider is not configured for POST JSON {url} with timeout {timeout}ms attempts {attempts} backoff {backoff}ms"
                ))))
            }
            RegIntrinsic::HttpPostJsonRetryAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _body = intrinsic_arg(&self.stack, base, args, 1)?;
                let timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let attempts = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                let backoff = expect_int_ref(intrinsic_arg(&self.stack, base, args, 4)?)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP async provider is not configured for POST JSON {url} with timeout {timeout}ms attempts {attempts} backoff {backoff}ms"
                ))))
            }
            RegIntrinsic::HttpPostJsonTimeoutAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _body = intrinsic_arg(&self.stack, base, args, 1)?;
                let timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP async provider is not configured for POST JSON {url} with timeout {timeout}ms"
                ))))
            }
            RegIntrinsic::HttpSendAsync => {
                let request = expect_http_request_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if request.method == "GET" {
                    Ok(json_result(http_get_local(&request.url)))
                } else {
                    Ok(value_err(http_error_value(format!(
                        "HTTP async provider is not configured for {} {}",
                        request.method, request.url
                    ))))
                }
            }
            RegIntrinsic::HttpRequestJson => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let body = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(http_request_value("POST", url, body, 0, 1, 0, 0))
            }
            RegIntrinsic::HttpRequestWithHeader => {
                let request = intrinsic_arg(&self.stack, base, args, 0)?;
                let _name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let _value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut request = expect_http_request_ref(request)?;
                request.header_count = request.header_count.saturating_add(1);
                Ok(request.to_value())
            }
            RegIntrinsic::HttpRequestWithRetry => {
                let request = intrinsic_arg(&self.stack, base, args, 0)?;
                let attempts = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let backoff_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut request = expect_http_request_ref(request)?;
                request.attempts = attempts;
                request.backoff_ms = backoff_ms;
                Ok(request.to_value())
            }
            RegIntrinsic::HttpRequestWithTimeout => {
                let request = intrinsic_arg(&self.stack, base, args, 0)?;
                let timeout_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut request = expect_http_request_ref(request)?;
                request.timeout_ms = timeout_ms;
                Ok(request.to_value())
            }
            RegIntrinsic::HttpResponseIsSuccess => {
                let response = intrinsic_arg(&self.stack, base, args, 0)?;
                let status = expect_int_ref(&read_field_ref(response, "status")?)?;
                Ok(VmValue::Bool((200..300).contains(&status)))
            }
            RegIntrinsic::HttpResponseLines => {
                let response = intrinsic_arg(&self.stack, base, args, 0)?;
                let text = read_field_ref(response, "body")?;
                let text = expect_string_ref(&text)?;
                Ok(VmValue::List(Rc::new(RefCell::new(
                    text.lines().map(VmValue::string).collect(),
                ))))
            }
            RegIntrinsic::HttpResponseStatus => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "status")
            }
            RegIntrinsic::HttpResponseText => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "body")
            }
            RegIntrinsic::ImageInspect => {
                let image = expect_image_state(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.stdout.push_str(&image.inspect_line());
                self.stdout.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ImageLoad => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read(path)
                        .map(|bytes| image_value(bytes, None, None, vec!["load".to_string()]))
                        .map_err(|error| image_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::ImageNormalize => {
                let image_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Image.normalize missing image.".to_string())
                })?;
                let mut image = expect_image_state(self.reg(base + image_reg))?;
                image.operations.push("normalize".to_string());
                self.set_reg(base + image_reg, image.to_value());
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ImageResize => {
                let image_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Image.resize missing image.".to_string())
                })?;
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let height = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut image = expect_image_state(self.reg(base + image_reg))?;
                image.width = Some(width);
                image.height = Some(height);
                image.operations.push("resize".to_string());
                self.set_reg(base + image_reg, image.to_value());
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ImageSave => {
                let image = expect_image_state(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    std::fs::write(path, image.saved_bytes())
                        .map(|_| VmValue::Unit)
                        .map_err(|error| image_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::ImageSharpen => {
                let image_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Image.sharpen missing image.".to_string())
                })?;
                let mut image = expect_image_state(self.reg(base + image_reg))?;
                image.operations.push("sharpen".to_string());
                self.set_reg(base + image_reg, image.to_value());
                Ok(VmValue::Unit)
            }
            RegIntrinsic::InstantElapsed => {
                let start = expect_instant_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    clock_system_unix_ms().saturating_sub(start).max(0),
                ))
            }
            RegIntrinsic::IntBitAnd => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left & right))
            }
            RegIntrinsic::IntBitNot => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(!value))
            }
            RegIntrinsic::IntBitOr => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left | right))
            }
            RegIntrinsic::IntBitXor => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left ^ right))
            }
            RegIntrinsic::IntShiftLeft => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bits = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value.wrapping_shl(bits.max(0) as u32)))
            }
            RegIntrinsic::IntShiftRight => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bits = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value.wrapping_shr(bits.max(0) as u32)))
            }
            RegIntrinsic::IntToString => Ok(VmValue::string(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::FloatToString => Ok(VmValue::string(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::FloatIsFinite => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_finite(),
            )),
            RegIntrinsic::FloatIsInfinite => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_infinite(),
            )),
            RegIntrinsic::FloatIsNan => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_nan(),
            )),
            RegIntrinsic::MathAbs => Ok(VmValue::Int(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.abs(),
            )),
            RegIntrinsic::MathAbsFloat => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.abs(),
            )),
            RegIntrinsic::MathCeil => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.ceil() as i64,
            )),
            RegIntrinsic::MathClamp => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let max = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::Int(value.clamp(min, max)))
            }
            RegIntrinsic::MathClampFloat => {
                let value = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let max = expect_float_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::Float(value.clamp(min, max)))
            }
            RegIntrinsic::MathFloor => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.floor() as i64,
            )),
            RegIntrinsic::MathMax => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.max(right)))
            }
            RegIntrinsic::MathMaxFloat => {
                let left = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(left.max(right)))
            }
            RegIntrinsic::MathMin => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.min(right)))
            }
            RegIntrinsic::MathMinFloat => {
                let left = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(left.min(right)))
            }
            RegIntrinsic::MathPow => {
                let base_value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let exponent = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(base_value.pow(exponent.max(0) as u32)))
            }
            RegIntrinsic::MathPowFloat => {
                let base_value = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let exponent = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(base_value.powf(exponent)))
            }
            RegIntrinsic::MathRound => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.round() as i64,
            )),
            RegIntrinsic::MathSqrt => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.sqrt(),
            )),
            RegIntrinsic::JsonArray => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(format!("[{}]", items.join(","))))
            }
            RegIntrinsic::JsonArrayBools => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_bools_value(value)))
            }
            RegIntrinsic::JsonArrayContainsPrefix => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    prefix,
                    JsonArrayStringMatch::Prefix,
                )))
            }
            RegIntrinsic::JsonArrayContainsString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let item = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    item,
                    JsonArrayStringMatch::Exact,
                )))
            }
            RegIntrinsic::JsonArrayContainsSubstring => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    text,
                    JsonArrayStringMatch::Substring,
                )))
            }
            RegIntrinsic::JsonArrayCountWhere => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let items = match json_array_items(value) {
                    Ok(items) => items.clone(),
                    Err(error) => return Ok(value_err(error)),
                };
                let mut count = 0_i64;
                for item in items {
                    let result = self.call_closure_one(
                        unit,
                        &predicate,
                        VmValue::Json(Rc::new(item)),
                        next_base,
                    )?;
                    match result_variant_payload(&result)? {
                        Ok(value) => {
                            if expect_bool_ref(&value)? {
                                count += 1;
                            }
                        }
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(VmValue::Int(count)))
            }
            RegIntrinsic::JsonArrayFold => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let items = match json_array_items(value) {
                    Ok(items) => items.clone(),
                    Err(error) => return Ok(value_err(error)),
                };
                for item in items {
                    let result = self.call_closure_two(
                        unit,
                        &folder,
                        state,
                        VmValue::Json(Rc::new(item)),
                        next_base,
                    )?;
                    match result_variant_payload(&result)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::JsonArrayGet => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_get_value(value, index)))
            }
            RegIntrinsic::JsonArrayInts => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_ints_value(value)))
            }
            RegIntrinsic::JsonArrayLen => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_array()
                    .map(|items| VmValue::Int(items.len() as i64))
                    .ok_or_else(|| json_error_value("JSON value is not an array"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonArrayStrings => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_strings_value(value)))
            }
            RegIntrinsic::JsonAt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).map(|value| VmValue::Json(Rc::new(value))),
                ))
            }
            RegIntrinsic::JsonAtBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_bool_value),
                ))
            }
            RegIntrinsic::JsonAtBoolOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_value_at(value, path)
                    .and_then(json_as_bool_value)
                    .unwrap_or(VmValue::Bool(fallback)))
            }
            RegIntrinsic::JsonAtInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_int_value),
                ))
            }
            RegIntrinsic::JsonAtIntOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_value_at(value, path)
                    .and_then(json_as_int_value)
                    .unwrap_or(VmValue::Int(fallback)))
            }
            RegIntrinsic::JsonAtOptional => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value_ok(json_optional_path_value(value, path)))
            }
            RegIntrinsic::JsonAtOptionalBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_bool_value,
                )))
            }
            RegIntrinsic::JsonAtOptionalInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_int_value,
                )))
            }
            RegIntrinsic::JsonAtOptionalString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_string_value,
                )))
            }
            RegIntrinsic::JsonAtOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_json_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::Json(Rc::new(
                    json_value_at(value, path).unwrap_or_else(|_| fallback.clone()),
                )))
            }
            RegIntrinsic::JsonAtString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_string_value),
                ))
            }
            RegIntrinsic::JsonAtStringOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(json_value_at(value, path)
                    .and_then(json_as_string_value)
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonAtToString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).map(|value| VmValue::string(value.to_string())),
                ))
            }
            RegIntrinsic::JsonAtToStringOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(json_value_at(value, path)
                    .map(|value| VmValue::string(value.to_string()))
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonAsBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_bool().map(VmValue::Bool).ok_or_else(
                    || json_error_value("JSON value is not a boolean"),
                )))
            }
            RegIntrinsic::JsonAsInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_i64().map(VmValue::Int).ok_or_else(
                    || json_error_value("JSON value is not an integer"),
                )))
            }
            RegIntrinsic::JsonAsString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_str().map(VmValue::string).ok_or_else(
                    || json_error_value("JSON value is not a string"),
                )))
            }
            RegIntrinsic::JsonBoolAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_bool_value),
                ))
            }
            RegIntrinsic::JsonBoolAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_bool_value)
                    .unwrap_or(VmValue::Bool(fallback)))
            }
            RegIntrinsic::JsonBoolField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonClone => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(value.clone())))
            }
            RegIntrinsic::JsonDecode | RegIntrinsic::JsonDecodeText => Err(EvalError::Runtime(
                "reg VM Json.decode requires typed intrinsic metadata.".to_string(),
            )),
            RegIntrinsic::JsonEncode => {
                let value = vm_value_to_json_literal(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::JsonErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::JsonField => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_field_value(value, name)))
            }
            RegIntrinsic::JsonFieldBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "boolean",
                    |field| field.as_bool().map(VmValue::Bool),
                )))
            }
            RegIntrinsic::JsonFieldInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "integer",
                    |field| field.as_i64().map(VmValue::Int),
                )))
            }
            RegIntrinsic::JsonFieldOptional => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value_ok(json_optional_field_value(value, name)))
            }
            RegIntrinsic::JsonFieldOptionalBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "boolean",
                    |field| field.as_bool().map(VmValue::Bool),
                )))
            }
            RegIntrinsic::JsonFieldOptionalInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "integer",
                    |field| field.as_i64().map(VmValue::Int),
                )))
            }
            RegIntrinsic::JsonFieldOptionalString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "string",
                    |field| field.as_str().map(VmValue::string),
                )))
            }
            RegIntrinsic::JsonFieldString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "string",
                    |field| field.as_str().map(VmValue::string),
                )))
            }
            RegIntrinsic::JsonIntAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_int_value),
                ))
            }
            RegIntrinsic::JsonIntAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_int_value)
                    .unwrap_or(VmValue::Int(fallback)))
            }
            RegIntrinsic::JsonIsArray => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_array()))
            }
            RegIntrinsic::JsonIsNull => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_null()))
            }
            RegIntrinsic::JsonIsObject => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_object()))
            }
            RegIntrinsic::JsonIntField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonKind => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(json_kind(value)))
            }
            RegIntrinsic::JsonObject => {
                let fields = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(format!("{{{}}}", fields.join(","))))
            }
            RegIntrinsic::JsonObjectKeys => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_object()
                    .map(|fields| {
                        let mut keys = fields.keys().map(VmValue::string).collect::<Vec<_>>();
                        keys.sort_by_key(VmValue::display);
                        VmValue::List(Rc::new(RefCell::new(keys)))
                    })
                    .ok_or_else(|| json_error_value("JSON value is not an object"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonObjectLen => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_object()
                    .map(|fields| VmValue::Int(fields.len() as i64))
                    .ok_or_else(|| json_error_value("JSON value is not an object"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonParse => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    serde_json::from_str::<serde_json::Value>(text)
                        .map(|value| VmValue::Json(Rc::new(value)))
                        .map_err(|error| json_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::JsonParseFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map_err(|error| json_error_value(error.to_string()))
                        .and_then(|text| {
                            serde_json::from_str::<serde_json::Value>(&text)
                                .map(|value| VmValue::Json(Rc::new(value)))
                                .map_err(|error| json_error_value(error.to_string()))
                        }),
                ))
            }
            RegIntrinsic::JsonQuoteString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(json_quote_string(value)?))
            }
            RegIntrinsic::JsonRawField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonStringAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_string_value),
                ))
            }
            RegIntrinsic::JsonStringAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_string_value)
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonStringArray => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let quoted = items
                    .iter()
                    .map(|item| json_quote_string(item))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::string(format!("[{}]", quoted.join(","))))
            }
            RegIntrinsic::JsonStringField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    json_quote_string(value)?
                )))
            }
            RegIntrinsic::JsonStrings => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(
                    items.into_iter().map(serde_json::Value::String).collect(),
                ))))
            }
            RegIntrinsic::JsonToStringAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .map(|value| VmValue::string(value.to_string())),
                ))
            }
            RegIntrinsic::JsonToStringAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .map(|value| VmValue::string(value.to_string()))
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonToString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::JsonValue => {
                let value = vm_value_to_json_literal(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(value)))
            }
            RegIntrinsic::JsonValues => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_json_ref(value).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            RegIntrinsic::ListAll => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values {
                    let keep = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if !expect_bool_ref(&keep)? {
                        return Ok(VmValue::Bool(false));
                    }
                }
                Ok(VmValue::Bool(true))
            }
            RegIntrinsic::ListAny | RegIntrinsic::ListContains => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values {
                    let matched = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if expect_bool_ref(&matched)? {
                        return Ok(VmValue::Bool(true));
                    }
                }
                Ok(VmValue::Bool(false))
            }
            RegIntrinsic::ListContainsValue => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(
                    list.borrow().iter().any(|item| item == value),
                ))
            }
            RegIntrinsic::ListCountWhere => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                let mut count = 0;
                for value in values {
                    let matched = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if expect_bool_ref(&matched)? {
                        count += 1;
                    }
                }
                Ok(VmValue::Int(count))
            }
            RegIntrinsic::ListConsume => {
                let _ = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ListFind => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values {
                    let matched =
                        self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                    if expect_bool_ref(&matched)? {
                        return Ok(VmValue::OptionSome(Box::new(value)));
                    }
                }
                Ok(VmValue::OptionNone)
            }
            RegIntrinsic::ListFirst => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(list
                    .borrow()
                    .first()
                    .cloned()
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListFlatMap => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                let mut flattened = Vec::new();
                for value in values {
                    let mapped = self.call_closure_one(unit, &mapper, value, next_base)?;
                    let mapped = expect_list_ref(&mapped)?;
                    flattened.extend(mapped.borrow().iter().cloned());
                }
                Ok(VmValue::List(Rc::new(RefCell::new(flattened))))
            }
            RegIntrinsic::ListFlatten => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut flattened = Vec::new();
                for value in list.borrow().iter() {
                    let nested = expect_list_ref(value)?;
                    flattened.extend(nested.borrow().iter().cloned());
                }
                Ok(VmValue::List(Rc::new(RefCell::new(flattened))))
            }
            RegIntrinsic::ListGroupBy => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key_fn = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                let mut groups: HashMap<VmMapKey, VmValue> = HashMap::new();
                for value in values {
                    let key_value =
                        self.call_closure_one(unit, &key_fn, value.clone(), next_base)?;
                    let key = map_key_from_value(&key_value)?;
                    match groups.get(&key) {
                        Some(VmValue::List(items)) => {
                            items.borrow_mut().push(value);
                        }
                        Some(other) => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.group_by expected List group, got `{}`.",
                                other.display()
                            )));
                        }
                        None => {
                            groups.insert(key, VmValue::List(Rc::new(RefCell::new(vec![value]))));
                        }
                    }
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(groups))))
            }
            RegIntrinsic::ListIsEmpty => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(list.borrow().is_empty()))
            }
            RegIntrinsic::ListJoin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let separator =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(VmValue::string(join_string_values(
                    &list.borrow(),
                    &separator,
                )?))
            }
            RegIntrinsic::ListLast => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(list
                    .borrow()
                    .last()
                    .cloned()
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListDedup => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut values = Vec::new();
                for value in list.borrow().iter() {
                    if !values.contains(value) {
                        values.push(value.clone());
                    }
                }
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListEnumerate => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut values = Vec::new();
                for (index, value) in list.borrow().iter().enumerate() {
                    values.push(VmValue::List(Rc::new(RefCell::new(vec![
                        VmValue::Int(index as i64),
                        VmValue::Int(expect_int_ref(value)?),
                    ]))));
                }
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListMax => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let max = list
                    .borrow()
                    .iter()
                    .map(expect_int_ref)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max();
                Ok(max
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Int(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListMin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = list
                    .borrow()
                    .iter()
                    .map(expect_int_ref)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .min();
                Ok(min
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Int(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListNew => Ok(VmValue::List(Rc::new(RefCell::new(Vec::new())))),
            RegIntrinsic::ListPipeline | RegIntrinsic::PipelineCollect => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::ListPartition => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                let mut matched = Vec::new();
                let mut unmatched = Vec::new();
                for value in values {
                    let keep = self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                    if expect_bool_ref(&keep)? {
                        matched.push(value);
                    } else {
                        unmatched.push(value);
                    }
                }
                Ok(VmValue::List(Rc::new(RefCell::new(vec![
                    VmValue::List(Rc::new(RefCell::new(matched))),
                    VmValue::List(Rc::new(RefCell::new(unmatched))),
                ]))))
            }
            RegIntrinsic::PipelineEach => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let action = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values.iter().cloned() {
                    let _ = self.call_closure_one(unit, &action, value, next_base)?;
                }
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::PipelineTryMap => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                let mut mapped = Vec::with_capacity(len);
                for index in 0..len {
                    let value = list.borrow()[index].clone();
                    match result_variant_payload(
                        &self.call_closure_one(unit, &mapper, value, next_base)?,
                    )? {
                        Ok(value) => mapped.push(value),
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(VmValue::List(Rc::new(RefCell::new(mapped)))))
            }
            RegIntrinsic::ListReverse => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut values = list.borrow().clone();
                values.reverse();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListSkip => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().iter().skip(count).cloned().collect();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListSlice => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = nonnegative_count(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let borrowed = list.borrow();
                if start >= borrowed.len() {
                    return Ok(VmValue::List(Rc::new(RefCell::new(Vec::new()))));
                }
                let end = start.saturating_add(len).min(borrowed.len());
                Ok(VmValue::List(Rc::new(RefCell::new(
                    borrowed[start..end].to_vec(),
                ))))
            }
            RegIntrinsic::ListSum => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let total = list
                    .borrow()
                    .iter()
                    .map(expect_int_ref)
                    .try_fold(0_i64, |total, value| value.map(|value| total + value))?;
                Ok(VmValue::Int(total))
            }
            RegIntrinsic::ListZip => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let left = left.borrow();
                let right = right.borrow();
                let values = left
                    .iter()
                    .zip(right.iter())
                    .map(|(left, right)| {
                        VmValue::List(Rc::new(RefCell::new(vec![left.clone(), right.clone()])))
                    })
                    .collect();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListTryFold => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let values = list.borrow().clone();
                for value in values {
                    let folded = self.call_closure_two(unit, &folder, state, value, next_base)?;
                    match result_variant_payload(&folded)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::ListTake => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().iter().take(count).cloned().collect();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListToJsonStrings => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_string_ref(value).map(|value| value.to_string()))
                    .map(|value| value.map(serde_json::Value::String))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            RegIntrinsic::ListToJsonValues => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_json_ref(value).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            RegIntrinsic::LogError => {
                let line =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                self.stderr.push_str(&line);
                self.stderr.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogErrorJson => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.stderr.push_str(&value.to_string());
                self.stderr.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogTrace => {
                let event =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                let message =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                self.stdout.push_str("trace ");
                self.stdout.push_str(&event);
                self.stdout.push_str(": ");
                self.stdout.push_str(&message);
                self.stdout.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogWrite => {
                let line =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                self.stdout.push_str(&line);
                self.stdout.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogWriteJson => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.stdout.push_str(&value.to_string());
                self.stdout.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::MapContainsKey => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(map.borrow().contains_key(&key)))
            }
            RegIntrinsic::MapFilter => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                let mut filtered = HashMap::new();
                for (key, value) in entries {
                    let keep = self.call_closure_two(
                        unit,
                        &predicate,
                        vm_value_from_map_key(&key),
                        value.clone(),
                        next_base,
                    )?;
                    if expect_bool_ref(&keep)? {
                        filtered.insert(key, value);
                    }
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(filtered))))
            }
            RegIntrinsic::MapFold => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    state = self.call_closure_three(
                        unit,
                        &folder,
                        state,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                }
                Ok(state)
            }
            RegIntrinsic::MapForEach => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let callback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    let _ = self.call_closure_two(
                        unit,
                        &callback,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                }
                Ok(VmValue::Unit)
            }
            RegIntrinsic::MapGetOrDefault => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let default = intrinsic_arg(&self.stack, base, args, 2)?.clone();
                Ok(map.borrow().get(&key).cloned().unwrap_or(default))
            }
            RegIntrinsic::MapIsEmpty => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(map.borrow().is_empty()))
            }
            RegIntrinsic::MapKeys => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let keys = map
                    .borrow()
                    .keys()
                    .map(vm_value_from_map_key)
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(keys))))
            }
            RegIntrinsic::MapLen => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(map.borrow().len() as i64))
            }
            RegIntrinsic::MapMapValues => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                let mut mapped = HashMap::new();
                for (key, value) in entries {
                    mapped.insert(key, self.call_closure_one(unit, &mapper, value, next_base)?);
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(mapped))))
            }
            RegIntrinsic::MapMerge => {
                let left = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_map_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let resolver = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut merged = left.borrow().clone();
                let right_entries = right
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, right_value) in right_entries {
                    if let Some(left_value) = merged.get(&key).cloned() {
                        let resolved = self.call_closure_two(
                            unit,
                            &resolver,
                            left_value,
                            right_value,
                            next_base,
                        )?;
                        merged.insert(key, resolved);
                    } else {
                        merged.insert(key, right_value);
                    }
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(merged))))
            }
            RegIntrinsic::MapNew => Ok(VmValue::Map(Rc::new(RefCell::new(HashMap::new())))),
            RegIntrinsic::MapTryFold => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    let folded = self.call_closure_three(
                        unit,
                        &folder,
                        state,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                    match result_variant_payload(&folded)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::MapValues => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = map.borrow().values().cloned().collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::OptionAndThen => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSome(value) => ensure_option_value(self.call_closure_one(
                        unit,
                        &mapper,
                        (**value).clone(),
                        next_base,
                    )?),
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.and_then expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionFilter => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSome(value) => {
                        let value = (**value).clone();
                        let keep =
                            self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                        if expect_bool_ref(&keep)? {
                            Ok(VmValue::OptionSome(Box::new(value)))
                        } else {
                            Ok(VmValue::OptionNone)
                        }
                    }
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.filter expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionIsNone => Ok(VmValue::Bool(matches!(
                intrinsic_arg(&self.stack, base, args, 0)?,
                VmValue::OptionNone
            ))),
            RegIntrinsic::OptionIsSome => Ok(VmValue::Bool(matches!(
                intrinsic_arg(&self.stack, base, args, 0)?,
                VmValue::OptionSome(_)
            ))),
            RegIntrinsic::OptionMap => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSome(value) => Ok(VmValue::OptionSome(Box::new(
                        self.call_closure_one(unit, &mapper, (**value).clone(), next_base)?,
                    ))),
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.map expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionOkOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let error = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSome(value) => Ok(value_ok((**value).clone())),
                    VmValue::OptionNone => Ok(value_err(error)),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.ok_or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let fallback = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSome(_) => Ok(option.clone()),
                    VmValue::OptionNone => Ok(fallback),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionUnwrapOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let default = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSome(value) => Ok((**value).clone()),
                    VmValue::OptionNone => Ok(default),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.unwrap_or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionUnwrapOrElse => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let fallback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSome(value) => Ok((**value).clone()),
                    VmValue::OptionNone => self.call_closure_zero(unit, &fallback, next_base),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.unwrap_or_else expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OrdCompare => {
                let left = intrinsic_arg(&self.stack, base, args, 0)?;
                let right = intrinsic_arg(&self.stack, base, args, 1)?;
                let value = match vm_value_cmp(left, right)? {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                };
                Ok(VmValue::Int(value))
            }
            RegIntrinsic::OsClose => {
                let _ = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::PatchApplyText => {
                let original = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let patch = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    patch_apply_text_string(original, patch)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                ))
            }
            RegIntrinsic::ProcessRun | RegIntrinsic::ProcessRunAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    process_run_output(command, &argv).map(process_output_value),
                ))
            }
            RegIntrinsic::ProcessRunStdout | RegIntrinsic::ProcessRunStdoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(process_run_output(command, &argv).and_then(
                    |output| process_stdout_result(command, output).map(VmValue::string),
                )))
            }
            RegIntrinsic::ProcessRunTimeout | RegIntrinsic::ProcessRunTimeoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_result(
                    process_run_output(command, &argv).map(process_output_value),
                ))
            }
            RegIntrinsic::ProcessRunStdoutTimeout | RegIntrinsic::ProcessRunStdoutTimeoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_result(process_run_output(command, &argv).and_then(
                    |output| process_stdout_result(command, output).map(VmValue::string),
                )))
            }
            RegIntrinsic::ProcessRunManyStdout | RegIntrinsic::ProcessRunManyStdoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let appended = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let _jobs = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                Ok(json_result(
                    process_run_many_stdout(command, &argv, &appended).map(|items| {
                        VmValue::List(Rc::new(RefCell::new(
                            items.into_iter().map(VmValue::string).collect(),
                        )))
                    }),
                ))
            }
            RegIntrinsic::ProcessRunManyStdoutTimeout
            | RegIntrinsic::ProcessRunManyStdoutTimeoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let appended = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let _jobs = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 4)?)?;
                Ok(json_result(
                    process_run_many_stdout(command, &argv, &appended).map(|items| {
                        VmValue::List(Rc::new(RefCell::new(
                            items.into_iter().map(VmValue::string).collect(),
                        )))
                    }),
                ))
            }
            RegIntrinsic::ProcessRunRequest
            | RegIntrinsic::ProcessRunRequestAsync
            | RegIntrinsic::ProcessRunRequestCancellableAsync => {
                let request =
                    expect_process_request_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if matches!(intrinsic, RegIntrinsic::ProcessRunRequestCancellableAsync) {
                    let _ = expect_cancellation_id_ref(
                        intrinsic_arg(&self.stack, base, args, 1)?,
                        "CancellationToken",
                    )?;
                }
                Ok(json_result(
                    process_run_request(&request).map(process_output_value),
                ))
            }
            RegIntrinsic::ProcessStream => {
                let request =
                    expect_process_request_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(process_run_request(&request).map(|output| {
                    let mut events = Vec::new();
                    if !output.stdout.is_empty() {
                        events.push(process_event_value("stdout", &output.stdout, output.status));
                    }
                    if !output.stderr.is_empty() {
                        events.push(process_event_value("stderr", &output.stderr, output.status));
                    }
                    events.push(process_event_value("exit", "", output.status));
                    stream_value(events)
                })))
            }
            RegIntrinsic::SetContains => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(set.borrow().iter().any(|item| item == value)))
            }
            RegIntrinsic::SetDifference => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let right = right.borrow().clone();
                let values = left
                    .borrow()
                    .iter()
                    .filter(|value| !right.iter().any(|item| item == *value))
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::SetIntersection => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let right = right.borrow().clone();
                let values = left
                    .borrow()
                    .iter()
                    .filter(|value| right.iter().any(|item| item == *value))
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::SetIsEmpty => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(set.borrow().is_empty()))
            }
            RegIntrinsic::SetIsSubset => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let right = right.borrow().clone();
                Ok(VmValue::Bool(
                    left.borrow()
                        .iter()
                        .all(|value| right.iter().any(|item| item == value)),
                ))
            }
            RegIntrinsic::SetLen => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(set.borrow().len() as i64))
            }
            RegIntrinsic::SetNew => Ok(VmValue::List(Rc::new(RefCell::new(Vec::new())))),
            RegIntrinsic::SetToList => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(set.borrow().clone()))))
            }
            RegIntrinsic::SetUnion => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut values = left.borrow().clone();
                for value in right.borrow().iter().cloned() {
                    set_insert_vm(&mut values, value);
                }
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::SortedSetContains => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(set.borrow().iter().any(|item| item == value)))
            }
            RegIntrinsic::SortedSetIsEmpty => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(set.borrow().is_empty()))
            }
            RegIntrinsic::SortedSetLen => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(set.borrow().len() as i64))
            }
            RegIntrinsic::SortedSetNew => Ok(VmValue::List(Rc::new(RefCell::new(Vec::new())))),
            RegIntrinsic::SortedSetToList => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(set.borrow().clone()))))
            }
            RegIntrinsic::SortedMapContainsKey => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(sorted_map_get(&entries, key).is_some()))
            }
            RegIntrinsic::SortedMapGet => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(sorted_map_get(&entries, key)
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::SortedMapIsEmpty => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(entries.is_empty()))
            }
            RegIntrinsic::SortedMapKeys => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let keys = entries.into_iter().map(|(key, _)| key).collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(keys))))
            }
            RegIntrinsic::SortedMapLen => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(entries.len() as i64))
            }
            RegIntrinsic::SortedMapNew => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::SortedMapValues => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = entries
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::PathExists => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).exists()))
            }
            RegIntrinsic::PathExtension => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(Path::new(path)
                    .extension()
                    .map(|extension| {
                        VmValue::OptionSome(Box::new(VmValue::string(extension.to_string_lossy())))
                    })
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PathFileName => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(Path::new(path)
                    .file_name()
                    .map(|name| {
                        VmValue::OptionSome(Box::new(VmValue::string(name.to_string_lossy())))
                    })
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PathFromString | RegIntrinsic::PathToString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value))
            }
            RegIntrinsic::PathIsAbsolute => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_absolute()))
            }
            RegIntrinsic::PathIsDir => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_dir()))
            }
            RegIntrinsic::PathIsFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_file()))
            }
            RegIntrinsic::PathJoin => {
                let base_path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let child = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(path_join_string(base_path, child)))
            }
            RegIntrinsic::PathListFiles => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    directory_list_files(Path::new(path))
                        .map(|files| {
                            VmValue::List(Rc::new(RefCell::new(
                                files.into_iter().map(VmValue::string).collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::PathListPaths => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    directory_list_paths(Path::new(path))
                        .map(|paths| {
                            VmValue::List(Rc::new(RefCell::new(
                                paths
                                    .into_iter()
                                    .map(|path| VmValue::string(path.to_string_lossy()))
                                    .collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::PathNormalize => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(path_normalize_string(path)))
            }
            RegIntrinsic::PathParent => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(Path::new(path)
                    .parent()
                    .map(|parent| {
                        VmValue::OptionSome(Box::new(VmValue::string(parent.to_string_lossy())))
                    })
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PathReadString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::PathResolveRelative => {
                let root = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let relative = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    path_resolve_relative_string(root, relative)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                ))
            }
            RegIntrinsic::PathSafeRelative => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    path_safe_relative_string(value)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                ))
            }
            RegIntrinsic::PathStartsWith => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let base_path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(
                    Path::new(path).starts_with(Path::new(base_path)),
                ))
            }
            RegIntrinsic::PathWithExtension => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let extension = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut path = PathBuf::from(path);
                path.set_extension(extension);
                Ok(VmValue::string(path.to_string_lossy()))
            }
            RegIntrinsic::PathWriteString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, text)))
            }
            RegIntrinsic::PersistentMapClear => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::PersistentMapContainsKey => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(sorted_map_get(&entries, key).is_some()))
            }
            RegIntrinsic::PersistentMapGet => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(sorted_map_get(&entries, key)
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PersistentMapInsert => {
                let mut entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let value = intrinsic_arg(&self.stack, base, args, 2)?.clone();
                sorted_map_insert(&mut entries, key, value);
                Ok(sorted_map_value(entries))
            }
            RegIntrinsic::PersistentMapIsEmpty => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(entries.is_empty()))
            }
            RegIntrinsic::PersistentMapLen => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(entries.len() as i64))
            }
            RegIntrinsic::PersistentMapNew => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::PersistentMapRemove => {
                let mut entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                sorted_map_remove(&mut entries, key);
                Ok(sorted_map_value(entries))
            }
            RegIntrinsic::PoolStatsAvailable => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "available")
            }
            RegIntrinsic::PoolStatsCapacity => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "capacity")
            }
            RegIntrinsic::PoolStatsCreated => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "created")
            }
            RegIntrinsic::PoolStatsInUse => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "in_use")
            }
            RegIntrinsic::ResourcePoolBorrow => {
                self.resource_pool_borrow(unit, args, base, next_base, false)
            }
            RegIntrinsic::ResourcePoolDiscard => {
                let lease_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM ResourcePool.discard missing lease.".to_string())
                })?;
                let lease = self.reg(base + lease_reg).clone();
                self.set_reg(base + lease_reg, mark_pool_lease_discarded(lease)?);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ResourcePoolLazy => {
                self.resource_pool_new(unit, args, base, next_base, true, false)
            }
            RegIntrinsic::ResourcePoolNew => {
                self.resource_pool_new(unit, args, base, next_base, false, false)
            }
            RegIntrinsic::ResourcePoolStats => {
                let pool = expect_resource_pool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let stats = self.pools.get(&pool.id).cloned().unwrap_or(VmResourcePool {
                    capacity: 0,
                    created: 0,
                    in_use: 0,
                    idle: Vec::new(),
                    factory: None,
                    factory_returns_result: false,
                });
                Ok(pool_stats_value(
                    stats.capacity,
                    stats.created,
                    stats.idle.len() as i64,
                    stats.in_use,
                ))
            }
            RegIntrinsic::ResourcePoolTryBorrow => {
                self.resource_pool_borrow(unit, args, base, next_base, true)
            }
            RegIntrinsic::ResourcePoolTryLazy => {
                self.resource_pool_new(unit, args, base, next_base, true, true)
            }
            RegIntrinsic::ResourcePoolTryNew => {
                self.resource_pool_new(unit, args, base, next_base, false, true)
            }
            RegIntrinsic::RandomBool => {
                let mut rng = rand::thread_rng();
                Ok(VmValue::Bool(rng.r#gen()))
            }
            RegIntrinsic::RandomBytes => {
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut rng = rand::thread_rng();
                let mut bytes = vec![0u8; len.max(0) as usize];
                rng.fill(bytes.as_mut_slice());
                Ok(VmValue::Bytes(Rc::new(bytes)))
            }
            RegIntrinsic::RandomFloat => {
                let mut rng = rand::thread_rng();
                Ok(VmValue::Float(rng.r#gen()))
            }
            RegIntrinsic::RandomInt => {
                let min = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let max = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut rng = rand::thread_rng();
                Ok(VmValue::Int(rng.gen_range(min..=max)))
            }
            RegIntrinsic::RandomString => {
                const CHARSET: &[u8] =
                    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut rng = rand::thread_rng();
                let value = (0..len.max(0))
                    .map(|_| {
                        let idx = rng.gen_range(0..CHARSET.len());
                        CHARSET[idx] as char
                    })
                    .collect::<String>();
                Ok(VmValue::string(value))
            }
            RegIntrinsic::RegexCaptures => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let captures = regex
                    .captures(value)
                    .map(|captures| {
                        captures
                            .iter()
                            .filter_map(|matched| {
                                matched.map(|matched| VmValue::string(matched.as_str()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Ok(VmValue::List(Rc::new(RefCell::new(captures))))
            }
            RegIntrinsic::RegexCompile => {
                let pattern = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match regex::Regex::new(pattern) {
                    Ok(_) => value_ok(regex_value(pattern)),
                    Err(error) => value_err(regex_error_value(error.to_string())),
                })
            }
            RegIntrinsic::RegexErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::RegexFind => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(regex
                    .find(value)
                    .map(|matched| VmValue::OptionSome(Box::new(VmValue::string(matched.as_str()))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::RegexIsMatch => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(regex.is_match(value)))
            }
            RegIntrinsic::RegexReplaceAll => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let replacement = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(
                    regex.replace_all(value, replacement).to_string(),
                ))
            }
            RegIntrinsic::RegexSplit => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let parts = regex.split(value).map(VmValue::string).collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(parts))))
            }
            RegIntrinsic::RequestNew => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(request_value(path))
            }
            RegIntrinsic::RequestPath => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "path")
            }
            RegIntrinsic::ReceiverClose => {
                let receiver_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Receiver.close missing receiver.".to_string())
                })?;
                let receiver = expect_receiver_ref(self.reg(base + receiver_reg))?;
                self.channel_state_mut(receiver.channel_id)?.receiver_closed = true;
                self.set_reg(
                    base + receiver_reg,
                    receiver_value(receiver.channel_id, true),
                );
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ReceiverIntoStream => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(stream_collect_error_value("channel receiver closed"));
                }
                Ok(stream_channel_value(receiver.channel_id))
            }
            RegIntrinsic::ReceiverRecv => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(value_err(channel_error_value("channel receiver closed")));
                }
                Ok(json_result(self.channel_recv(receiver.channel_id)))
            }
            RegIntrinsic::ReceiverRecvCancellable => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(value_err(channel_error_value("channel receiver closed")));
                }
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 1)?,
                    "CancellationToken",
                )?;
                if self.cancellation_flags.get(&id).copied().unwrap_or(false) {
                    return Ok(value_err(channel_error_value("channel recv cancelled")));
                }
                Ok(json_result(self.channel_recv(receiver.channel_id)))
            }
            RegIntrinsic::ResponseBody => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "body")
            }
            RegIntrinsic::ResponseOk => {
                let body = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(value_ok(response_value(200, body)))
            }
            RegIntrinsic::ResponseStatus => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "status")
            }
            RegIntrinsic::RowBufferNew => {
                let size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(row_buffer_value(Vec::with_capacity(size.max(0) as usize)))
            }
            RegIntrinsic::RowFieldString => {
                let row = intrinsic_arg(&self.stack, base, args, 0)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(row_field_string_value(
                    expect_row_fields_ref(row)?,
                    index,
                )))
            }
            RegIntrinsic::ResultErr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(_) => VmValue::OptionNone,
                    Err(error) => VmValue::OptionSome(Box::new(error)),
                })
            }
            RegIntrinsic::ResultErrMessage => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(_) => VmValue::OptionNone,
                    Err(error) => VmValue::OptionSome(Box::new(VmValue::string(error.display()))),
                })
            }
            RegIntrinsic::ResultIsErr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(result.is_err()))
            }
            RegIntrinsic::ResultIsOk => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(result.is_ok()))
            }
            RegIntrinsic::ResultOk => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(value) => VmValue::OptionSome(Box::new(value)),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::ResultAndThen => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => {
                        let mapped = self.call_closure_one(unit, &mapper, value, next_base)?;
                        let _ = result_variant_payload(&mapped)?;
                        Ok(mapped)
                    }
                    Err(error) => Ok(value_err(error)),
                }
            }
            RegIntrinsic::ResultMap => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value_ok(
                        self.call_closure_one(unit, &mapper, value, next_base)?,
                    )),
                    Err(error) => Ok(value_err(error)),
                }
            }
            RegIntrinsic::ResultMapError => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value_ok(value)),
                    Err(error) => Ok(value_err(
                        self.call_closure_one(unit, &mapper, error, next_base)?,
                    )),
                }
            }
            RegIntrinsic::ResultUnwrapOr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let default = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                Ok(match result {
                    Ok(value) => value,
                    Err(_) => default,
                })
            }
            RegIntrinsic::ResultUnwrapOrElse => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let fallback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value),
                    Err(error) => self.call_closure_one(unit, &fallback, error, next_base),
                }
            }
            RegIntrinsic::RuleLoaderLoadRules => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(|text| VmValue::List(Rc::new(RefCell::new(rules_from_text(&text)))))
                        .map_err(|error| config_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::StringAfter => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .split_once(delimiter)
                    .map(|(_, right)| VmValue::OptionSome(Box::new(VmValue::string(right))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringBefore => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .find(delimiter)
                    .map(|index| VmValue::OptionSome(Box::new(VmValue::string(&value[..index]))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringBuilderNew => Ok(VmValue::string("")),
            RegIntrinsic::StringCharAt => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(usize::try_from(index)
                    .ok()
                    .and_then(|index| value.chars().nth(index))
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Char(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringChars => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(
                    value.chars().map(VmValue::Char).collect(),
                ))))
            }
            RegIntrinsic::StringContains => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.contains(needle)))
            }
            RegIntrinsic::StringCount => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value.matches(needle).count() as i64))
            }
            RegIntrinsic::StringCopy => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value))
            }
            RegIntrinsic::StringEndsWith => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let suffix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.ends_with(suffix)))
            }
            RegIntrinsic::StringFormat => {
                let template = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let args = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(string_format(template, &args)))
            }
            RegIntrinsic::StringFromBool => {
                let value = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::StringFromFloat => Ok(VmValue::string(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::StringFromInt => Ok(VmValue::string(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::StringIndexOf => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .find(needle)
                    .map(|index| VmValue::OptionSome(Box::new(VmValue::Int(index as i64))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringIsEmpty => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_empty()))
            }
            RegIntrinsic::StringJoin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let separator =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(VmValue::string(join_string_values(
                    &list.borrow(),
                    &separator,
                )?))
            }
            RegIntrinsic::StringLines => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let lines = value.lines().map(VmValue::string).collect::<Vec<VmValue>>();
                Ok(VmValue::List(Rc::new(RefCell::new(lines))))
            }
            RegIntrinsic::StringLen => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value.len() as i64))
            }
            RegIntrinsic::StringPadLeft => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fill = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_pad(value, width, fill, true)))
            }
            RegIntrinsic::StringPadRight => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fill = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_pad(value, width, fill, false)))
            }
            RegIntrinsic::StringParseFloat => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match value.parse::<f64>() {
                    Ok(value) => VmValue::OptionSome(Box::new(VmValue::Float(value))),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::StringParseInt => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match value.parse::<i64>() {
                    Ok(value) => VmValue::OptionSome(Box::new(VmValue::Int(value))),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::StringRepeat => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(value.repeat(count)))
            }
            RegIntrinsic::StringReplace => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(value.replace(from, to)))
            }
            RegIntrinsic::StringReplaceFirst => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(value.replacen(from, to, 1)))
            }
            RegIntrinsic::StringReverse => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.chars().rev().collect::<String>()))
            }
            RegIntrinsic::StringSlice => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_slice_range(value, start, len)))
            }
            RegIntrinsic::StringSplit => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let parts = value
                    .split(delimiter)
                    .map(VmValue::string)
                    .collect::<Vec<VmValue>>();
                Ok(VmValue::List(Rc::new(RefCell::new(parts))))
            }
            RegIntrinsic::StringStartsWith => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.starts_with(prefix)))
            }
            RegIntrinsic::StringStripPrefix => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .strip_prefix(prefix)
                    .map(|rest| VmValue::OptionSome(Box::new(VmValue::string(rest))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringToLowercase => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_lowercase()))
            }
            RegIntrinsic::StringToUppercase => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_uppercase()))
            }
            RegIntrinsic::StringTrim => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim()))
            }
            RegIntrinsic::StringTrimEnd => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim_end()))
            }
            RegIntrinsic::StringTrimStart => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim_start()))
            }
            RegIntrinsic::StreamCollectList => {
                let stream = expect_stream_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if let Some(message) = stream.collect_error {
                    return Ok(value_err(channel_error_value(message)));
                }
                if let Some(channel_id) = stream.channel_id {
                    let state = self.channel_state_mut(channel_id)?;
                    let values = state.queue.drain(..).collect::<Vec<_>>();
                    if state.senders == 0 {
                        return Ok(value_ok(VmValue::List(Rc::new(RefCell::new(values)))));
                    }
                    return Ok(value_err(channel_error_value(
                        "stream collect_list would block on an open channel stream",
                    )));
                }
                let values = stream.items.borrow_mut().drain(..).collect::<Vec<_>>();
                Ok(value_ok(VmValue::List(Rc::new(RefCell::new(values)))))
            }
            RegIntrinsic::StreamFromList => {
                let items = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?
                    .borrow()
                    .clone();
                Ok(stream_value(items))
            }
            RegIntrinsic::StreamNext => {
                let stream = expect_stream_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if let Some(message) = stream.collect_error {
                    return Ok(value_err(channel_error_value(message)));
                }
                if let Some(channel_id) = stream.channel_id {
                    return Ok(json_result(self.channel_recv(channel_id)));
                }
                let value = if stream.items.borrow().is_empty() {
                    VmValue::OptionNone
                } else {
                    VmValue::OptionSome(Box::new(stream.items.borrow_mut().remove(0)))
                };
                Ok(value_ok(value))
            }
            RegIntrinsic::SenderClose => {
                let sender_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Sender.close missing sender.".to_string())
                })?;
                let sender = expect_sender_ref(self.reg(base + sender_reg))?;
                if !sender.closed {
                    let state = self.channel_state_mut(sender.channel_id)?;
                    state.senders = state.senders.saturating_sub(1);
                }
                self.set_reg(base + sender_reg, sender_value(sender.channel_id, true));
                Ok(VmValue::Unit)
            }
            RegIntrinsic::SenderSend => {
                let sender = expect_sender_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                Ok(json_result(self.channel_send(sender, value)))
            }
            RegIntrinsic::SenderSendCancellable => {
                let sender = expect_sender_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 2)?,
                    "CancellationToken",
                )?;
                if self.cancellation_flags.get(&id).copied().unwrap_or(false) {
                    return Ok(value_err(channel_error_value("channel send cancelled")));
                }
                Ok(json_result(self.channel_send(sender, value)))
            }
            RegIntrinsic::TcpConnect => {
                let host =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                let port = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(self.tcp_connect(&host, port)))
            }
            RegIntrinsic::TcpStreamRead => {
                let id = expect_tcp_stream_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let max_bytes = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    self.tcp_stream_read(id, max_bytes)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes))),
                ))
            }
            RegIntrinsic::TcpStreamShutdown => {
                let id = expect_tcp_stream_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    self.tcp_stream_shutdown(id).map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::TcpStreamWrite => {
                let id = expect_tcp_stream_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(json_result(
                    self.tcp_stream_write(id, &data).map(VmValue::Int),
                ))
            }
            RegIntrinsic::TcpStreamWriteAll => {
                let id = expect_tcp_stream_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(json_result(
                    self.tcp_stream_write_all(id, &data).map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::TempDirKeep | RegIntrinsic::TempDirPath => {
                let dir = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::string(expect_tempdir_path_ref(dir)?))
            }
            RegIntrinsic::TempDirNew => Ok(json_result(tempdir_new_value(std::env::temp_dir()))),
            RegIntrinsic::TempDirNewIn => {
                let parent = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(tempdir_new_value(PathBuf::from(parent))))
            }
            RegIntrinsic::TimerSleep => {
                let _ = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(value_ok(VmValue::Unit))
            }
            RegIntrinsic::TimerSleepCancellable => {
                let _ = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _ = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 1)?,
                    "CancellationToken",
                )?;
                Ok(value_ok(VmValue::Unit))
            }
            RegIntrinsic::TimerSleepUntil => {
                let _ = expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(value_ok(VmValue::Unit))
            }
            RegIntrinsic::TomlParseFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(toml_parse_file_value(path)))
            }
            RegIntrinsic::UrlDecodeComponent => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    percent_decode_str(value)
                        .decode_utf8()
                        .map(|value| VmValue::string(value.to_string()))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::UrlEncodeComponent => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    utf8_percent_encode(value, URL_COMPONENT_SET).to_string(),
                ))
            }
            RegIntrinsic::UrlFromString | RegIntrinsic::UrlToString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value))
            }
            RegIntrinsic::UuidNewV4 => Ok(VmValue::string(uuid::Uuid::new_v4().to_string())),
            RegIntrinsic::WebSocketConnect => {
                let url =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                Ok(json_result(self.websocket_connect(&url)))
            }
            RegIntrinsic::WebSocketClose => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    self.websocket_close(id).map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::WebSocketRecvBytes => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    self.websocket_recv(id, WebSocketExpectedFrame::Binary).map(
                        |value| match value {
                            Some(bytes) => value_some(VmValue::Bytes(Rc::new(bytes))),
                            None => value_none(),
                        },
                    ),
                ))
            }
            RegIntrinsic::WebSocketRecvText => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    self.websocket_recv(id, WebSocketExpectedFrame::Text).map(
                        |value| match value {
                            Some(bytes) => {
                                value_some(VmValue::string(String::from_utf8_lossy(&bytes)))
                            }
                            None => value_none(),
                        },
                    ),
                ))
            }
            RegIntrinsic::WebSocketSendBytes => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(json_result(
                    self.websocket_send(id, 0x2, &data).map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::WebSocketSendText => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(json_result(
                    self.websocket_send(id, 0x1, text.as_bytes())
                        .map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::YamlParse => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(yaml_parse_json_value(text)))
            }
            RegIntrinsic::YamlParseFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map_err(|error| json_error_value(error.to_string()))
                        .and_then(|text| yaml_parse_json_value(&text)),
                ))
            }
            RegIntrinsic::WeakDowngrade | RegIntrinsic::WeakFrom => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::WeakUpgrade => Ok(VmValue::OptionSome(Box::new(
                intrinsic_arg(&self.stack, base, args, 0)?.clone(),
            ))),
        }
    }
}

fn intrinsic_arg<'a>(
    stack: &'a [VmValue],
    base: usize,
    args: &[Reg],
    index: usize,
) -> Result<&'a VmValue, EvalError> {
    args.get(index)
        .and_then(|reg| stack.get(base + *reg))
        .ok_or_else(|| EvalError::Runtime(format!("reg VM missing argument {index}.")))
}

fn int_compare_op(op: BinaryOp) -> Option<RegIntCompare> {
    match op {
        BinaryOp::Less => Some(RegIntCompare::Less),
        BinaryOp::LessEqual => Some(RegIntCompare::LessEqual),
        BinaryOp::Greater => Some(RegIntCompare::Greater),
        BinaryOp::GreaterEqual => Some(RegIntCompare::GreaterEqual),
        _ => None,
    }
}

fn eval_int_compare(op: RegIntCompare, lhs: i64, rhs: i64) -> bool {
    match op {
        RegIntCompare::Less => lhs < rhs,
        RegIntCompare::LessEqual => lhs <= rhs,
        RegIntCompare::Greater => lhs > rhs,
        RegIntCompare::GreaterEqual => lhs >= rhs,
    }
}

fn eval_numeric_binary(op: BinaryOp, lhs: &VmValue, rhs: &VmValue) -> Result<VmValue, EvalError> {
    match (lhs, rhs) {
        (VmValue::Int(lhs), VmValue::Int(rhs)) => match op {
            BinaryOp::Add => Ok(VmValue::Int(lhs + rhs)),
            BinaryOp::Subtract => Ok(VmValue::Int(lhs - rhs)),
            BinaryOp::Multiply => Ok(VmValue::Int(lhs * rhs)),
            BinaryOp::Divide => Ok(VmValue::Int(lhs / rhs)),
            BinaryOp::Modulo => Ok(VmValue::Int(lhs % rhs)),
            _ => unreachable!("numeric binary helper called with non-arithmetic op"),
        },
        (VmValue::Float(lhs), VmValue::Float(rhs)) => match op {
            BinaryOp::Add => Ok(VmValue::Float(lhs + rhs)),
            BinaryOp::Subtract => Ok(VmValue::Float(lhs - rhs)),
            BinaryOp::Multiply => Ok(VmValue::Float(lhs * rhs)),
            BinaryOp::Divide => Ok(VmValue::Float(lhs / rhs)),
            BinaryOp::Modulo => Err(EvalError::Runtime(
                "reg VM modulo expects Int operands.".to_string(),
            )),
            _ => unreachable!("numeric binary helper called with non-arithmetic op"),
        },
        _ => Err(EvalError::Runtime(format!(
            "reg VM numeric operator expected matching Int or Float operands, got `{}` and `{}`.",
            lhs.display(),
            rhs.display()
        ))),
    }
}

fn eval_numeric_compare(
    op: RegIntCompare,
    lhs: &VmValue,
    rhs: &VmValue,
) -> Result<bool, EvalError> {
    match (lhs, rhs) {
        (VmValue::Int(lhs), VmValue::Int(rhs)) => Ok(eval_int_compare(op, *lhs, *rhs)),
        (VmValue::Float(lhs), VmValue::Float(rhs)) => Ok(match op {
            RegIntCompare::Less => lhs < rhs,
            RegIntCompare::LessEqual => lhs <= rhs,
            RegIntCompare::Greater => lhs > rhs,
            RegIntCompare::GreaterEqual => lhs >= rhs,
        }),
        _ => Err(EvalError::Runtime(format!(
            "reg VM numeric comparison expected matching Int or Float operands, got `{}` and `{}`.",
            lhs.display(),
            rhs.display()
        ))),
    }
}

fn expect_int_ref(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Int(value) => Ok(*value),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Int, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_float_ref(value: &VmValue) -> Result<f64, EvalError> {
    match value {
        VmValue::Float(value) => Ok(*value),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Float, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_char_ref(value: &VmValue) -> Result<char, EvalError> {
    match value {
        VmValue::Char(value) => Ok(*value),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Char, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_bytes_ref(value: &VmValue) -> Result<&[u8], EvalError> {
    match value {
        VmValue::Bytes(value) => Ok(value.as_slice()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Bytes, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_usize_ref(value: &VmValue) -> Result<usize, EvalError> {
    let value = expect_int_ref(value)?;
    usize::try_from(value).map_err(|_| {
        EvalError::Runtime(format!(
            "reg VM expected non-negative index, got `{value}`."
        ))
    })
}

fn nonnegative_count(value: &VmValue) -> Result<usize, EvalError> {
    Ok(expect_int_ref(value)?.max(0) as usize)
}

fn bytes_slice(value: &[u8], start: i64, len: i64) -> Vec<u8> {
    let start = start.max(0) as usize;
    if start >= value.len() {
        return Vec::new();
    }
    let len = len.max(0) as usize;
    let end = start.saturating_add(len).min(value.len());
    value[start..end].to_vec()
}

fn sha256_digest(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value);
    format!("{:x}", hasher.finalize())
}

fn hmac_sha256_digest(key: &[u8], value: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(value);
    format!("{:x}", mac.finalize().into_bytes())
}

fn utc_datetime(unix_ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(unix_ms)
        .single()
        .unwrap_or_else(|| {
            Utc.timestamp_millis_opt(0)
                .single()
                .expect("epoch is valid")
        })
}

fn date_parse_ymd(value: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let datetime = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&datetime).timestamp_millis())
}

fn date_parse_iso(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc).timestamp_millis())
}

fn date_is_leap_year(year: i64) -> bool {
    let Ok(year) = i32::try_from(year) else {
        return false;
    };
    NaiveDate::from_ymd_opt(year, 2, 29).is_some()
}

fn date_days_in_month(year: i64, month: i64) -> i64 {
    let Ok(year) = i32::try_from(year) else {
        return 0;
    };
    let Ok(month) = u32::try_from(month) else {
        return 0;
    };
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return 0;
    };
    let Some(next_month) = (if month == 12 {
        year.checked_add(1)
            .and_then(|year| NaiveDate::from_ymd_opt(year, 1, 1))
    } else {
        month
            .checked_add(1)
            .and_then(|month| NaiveDate::from_ymd_opt(year, month, 1))
    }) else {
        return 0;
    };
    (next_month - first).num_days()
}

fn set_insert_vm(items: &mut Vec<VmValue>, value: VmValue) -> bool {
    if items.iter().any(|item| item == &value) {
        return false;
    }
    items.push(value);
    true
}

fn set_remove_vm(items: &mut Vec<VmValue>, value: &VmValue) -> bool {
    let Some(index) = items.iter().position(|item| item == value) else {
        return false;
    };
    items.remove(index);
    true
}

fn sort_vm_values(items: &mut [VmValue]) -> Result<(), EvalError> {
    for item in items.iter() {
        vm_value_cmp(item, item)?;
    }
    let mut sorted = items.to_vec();
    sorted.sort_by(|left, right| vm_value_cmp(left, right).unwrap_or(Ordering::Equal));
    for pair in sorted.windows(2) {
        vm_value_cmp(&pair[0], &pair[1])?;
    }
    items.clone_from_slice(&sorted);
    Ok(())
}

fn vm_value_cmp(left: &VmValue, right: &VmValue) -> Result<Ordering, EvalError> {
    match (left, right) {
        (VmValue::Unit, VmValue::Unit) => Ok(Ordering::Equal),
        (VmValue::Int(left), VmValue::Int(right)) => Ok(left.cmp(right)),
        (VmValue::Bool(left), VmValue::Bool(right)) => Ok(left.cmp(right)),
        (VmValue::String(left), VmValue::String(right)) => Ok(left.cmp(right)),
        (VmValue::Char(left), VmValue::Char(right)) => Ok(left.cmp(right)),
        _ => Err(EvalError::Runtime(format!(
            "SortedSet value is not orderable: `{}`.",
            left.display()
        ))),
    }
}

fn expect_sorted_map_entries(value: &VmValue) -> Result<Vec<(VmValue, VmValue)>, EvalError> {
    let entries = expect_list_ref(value)?;
    entries
        .borrow()
        .iter()
        .map(|entry| {
            let pair = expect_list_ref(entry)?;
            let pair = pair.borrow();
            let [key, value] = pair.as_slice() else {
                return Err(EvalError::Runtime(format!(
                    "reg VM expected SortedMap entry, got `{}`.",
                    entry.display()
                )));
            };
            Ok((key.clone(), value.clone()))
        })
        .collect()
}

fn sorted_map_value(entries: Vec<(VmValue, VmValue)>) -> VmValue {
    VmValue::List(Rc::new(RefCell::new(
        entries
            .into_iter()
            .map(|(key, value)| VmValue::List(Rc::new(RefCell::new(vec![key, value]))))
            .collect(),
    )))
}

fn sorted_map_get(entries: &[(VmValue, VmValue)], key: &VmValue) -> Option<VmValue> {
    entries
        .iter()
        .find_map(|(entry_key, value)| (entry_key == key).then(|| value.clone()))
}

fn sorted_map_insert(entries: &mut Vec<(VmValue, VmValue)>, key: VmValue, value: VmValue) {
    if let Some((_, existing)) = entries.iter_mut().find(|(entry_key, _)| entry_key == &key) {
        *existing = value;
        return;
    }
    entries.push((key, value));
}

fn sorted_map_remove(entries: &mut Vec<(VmValue, VmValue)>, key: &VmValue) -> Option<VmValue> {
    let index = entries.iter().position(|(entry_key, _)| entry_key == key)?;
    Some(entries.remove(index).1)
}

fn sort_vm_map_entries(entries: &mut [(VmValue, VmValue)]) -> Result<(), EvalError> {
    for (key, _) in entries.iter() {
        vm_value_cmp(key, key)?;
    }
    let mut sorted = entries.to_vec();
    sorted.sort_by(|(left, _), (right, _)| vm_value_cmp(left, right).unwrap_or(Ordering::Equal));
    for pair in sorted.windows(2) {
        vm_value_cmp(&pair[0].0, &pair[1].0)?;
    }
    entries.clone_from_slice(&sorted);
    Ok(())
}

fn path_join_string(base: &str, child: &str) -> String {
    Path::new(base).join(child).to_string_lossy().to_string()
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

fn join_string_values(values: &[VmValue], separator: &str) -> Result<String, EvalError> {
    Ok(values
        .iter()
        .map(|value| expect_string_ref(value).map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?
        .join(separator))
}

fn expect_string_list_ref(value: &VmValue) -> Result<Vec<String>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow()
        .iter()
        .map(|value| expect_string_ref(value).map(str::to_string))
        .collect()
}

fn expect_bool_ref(value: &VmValue) -> Result<bool, EvalError> {
    match value {
        VmValue::Bool(value) => Ok(*value),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Bool, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_list_ref(value: &VmValue) -> Result<Rc<RefCell<Vec<VmValue>>>, EvalError> {
    match value {
        VmValue::List(value) => Ok(Rc::clone(value)),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected List, got `{}`.",
            other.display()
        ))),
    }
}

fn list_item_at(
    list: &Rc<RefCell<Vec<VmValue>>>,
    index: usize,
    operation: &str,
) -> Result<VmValue, EvalError> {
    let values = list.borrow();
    values.get(index).cloned().ok_or_else(|| {
        EvalError::Runtime(format!(
            "reg VM {operation} observed list length change at index {index}."
        ))
    })
}

fn expect_map_ref(value: &VmValue) -> Result<Rc<RefCell<HashMap<VmMapKey, VmValue>>>, EvalError> {
    match value {
        VmValue::Map(value) => Ok(Rc::clone(value)),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Map, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_json_ref(value: &VmValue) -> Result<&serde_json::Value, EvalError> {
    match value {
        VmValue::Json(value) => Ok(value.as_ref()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected JsonValue, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_closure_rc(value: &VmValue) -> Result<Rc<VmClosure>, EvalError> {
    match value {
        VmValue::Closure(value) => Ok(Rc::clone(value)),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Closure, got `{}`.",
            other.display()
        ))),
    }
}

fn ensure_option_value(value: VmValue) -> Result<VmValue, EvalError> {
    match value {
        VmValue::OptionSome(_) | VmValue::OptionNone => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Option, got `{}`.",
            other.display()
        ))),
    }
}

fn map_key_from_value(value: &VmValue) -> Result<VmMapKey, EvalError> {
    match value {
        VmValue::Bool(value) => Ok(VmMapKey::Bool(*value)),
        VmValue::Int(value) => Ok(VmMapKey::Int(*value)),
        VmValue::String(value) => Ok(VmMapKey::String(Rc::clone(value))),
        other => Err(EvalError::Runtime(format!(
            "reg VM Map key does not support `{}`.",
            other.display()
        ))),
    }
}

fn vm_value_from_map_key(key: &VmMapKey) -> VmValue {
    match key {
        VmMapKey::Bool(value) => VmValue::Bool(*value),
        VmMapKey::Int(value) => VmValue::Int(*value),
        VmMapKey::String(value) => VmValue::String(Rc::clone(value)),
    }
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

fn json_quote_string(value: &str) -> Result<String, EvalError> {
    serde_json::to_string(value).map_err(|error| EvalError::Runtime(error.to_string()))
}

fn json_array_get_value(value: &serde_json::Value, index: i64) -> Result<VmValue, VmValue> {
    if index < 0 {
        return Err(json_error_value(format!(
            "JSON array index `{index}` is negative"
        )));
    }
    let serde_json::Value::Array(items) = value else {
        return Err(json_error_value("JSON value is not an array"));
    };
    items
        .get(index as usize)
        .cloned()
        .map(|value| VmValue::Json(Rc::new(value)))
        .ok_or_else(|| json_error_value(format!("JSON array index `{index}` is out of bounds")))
}

fn json_array_items(value: &serde_json::Value) -> Result<&Vec<serde_json::Value>, VmValue> {
    let serde_json::Value::Array(items) = value else {
        return Err(json_error_value("JSON value is not an array"));
    };
    Ok(items)
}

fn json_array_bools_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut flags = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(flag) = item.as_bool() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not a boolean"
            )));
        };
        flags.push(VmValue::Bool(flag));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(flags))))
}

fn json_array_ints_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut numbers = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(number) = item.as_i64() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not an integer"
            )));
        };
        numbers.push(VmValue::Int(number));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(numbers))))
}

fn json_array_strings_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut strings = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(text) = item.as_str() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not a string"
            )));
        };
        strings.push(VmValue::string(text));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(strings))))
}

#[derive(Debug, Clone, Copy)]
enum JsonArrayStringMatch {
    Exact,
    Substring,
    Prefix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonPathPart {
    Field(String),
    Index(usize),
}

fn parse_json_path(path: &str) -> Result<Vec<JsonPathPart>, VmValue> {
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
                return Err(json_error_value(format!(
                    "JSON path `{path}` has an unterminated array index"
                )));
            }
            let raw_index = chars[start..index].iter().collect::<String>();
            let item_index = raw_index.parse::<usize>().map_err(|_| {
                json_error_value(format!(
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
            return Err(json_error_value(format!(
                "JSON path `{path}` contains an empty field"
            )));
        }
        parts.push(JsonPathPart::Field(field));
    }

    Ok(parts)
}

fn parse_json_text(text: &str) -> Result<serde_json::Value, VmValue> {
    serde_json::from_str::<serde_json::Value>(text)
        .map_err(|error| json_error_value(error.to_string()))
}

fn json_value_at(value: &serde_json::Value, path: &str) -> Result<serde_json::Value, VmValue> {
    let mut current = value;
    for part in parse_json_path(path)? {
        match part {
            JsonPathPart::Field(name) => {
                let Some(next) = current.get(&name) else {
                    return Err(json_error_value(format!(
                        "missing JSON field `{name}` at path `{path}`"
                    )));
                };
                current = next;
            }
            JsonPathPart::Index(index) => {
                let Some(items) = current.as_array() else {
                    return Err(json_error_value(format!(
                        "JSON path `{path}` expected an array before index `{index}`"
                    )));
                };
                let Some(next) = items.get(index) else {
                    return Err(json_error_value(format!(
                        "JSON array index `{index}` is out of bounds at path `{path}`"
                    )));
                };
                current = next;
            }
        }
    }
    Ok(current.clone())
}

fn json_optional_path_value(value: &serde_json::Value, path: &str) -> VmValue {
    match json_value_at(value, path) {
        Ok(value) if value.is_null() => VmValue::OptionNone,
        Ok(value) => VmValue::OptionSome(Box::new(VmValue::Json(Rc::new(value)))),
        Err(_) => VmValue::OptionNone,
    }
}

fn json_optional_typed_path_value(
    value: &serde_json::Value,
    path: &str,
    convert: fn(serde_json::Value) -> Result<VmValue, VmValue>,
) -> Result<VmValue, VmValue> {
    match json_value_at(value, path) {
        Ok(value) if value.is_null() => Ok(VmValue::OptionNone),
        Ok(value) => convert(value).map(|value| VmValue::OptionSome(Box::new(value))),
        Err(_) => Ok(VmValue::OptionNone),
    }
}

fn json_array_contains_string_value(
    value: &serde_json::Value,
    needle: &str,
    mode: JsonArrayStringMatch,
) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    Ok(VmValue::Bool(items.iter().any(|value| {
        value.as_str().is_some_and(|item| match mode {
            JsonArrayStringMatch::Exact => item == needle,
            JsonArrayStringMatch::Substring => item.contains(needle),
            JsonArrayStringMatch::Prefix => item.starts_with(needle),
        })
    })))
}

fn json_field_json(value: &serde_json::Value, name: &str) -> Result<serde_json::Value, VmValue> {
    value
        .get(name)
        .cloned()
        .ok_or_else(|| json_error_value(format!("missing JSON field `{name}`")))
}

fn json_field_value(value: &serde_json::Value, name: &str) -> Result<VmValue, VmValue> {
    json_field_json(value, name).map(|value| VmValue::Json(Rc::new(value)))
}

fn json_typed_field_value(
    value: &serde_json::Value,
    name: &str,
    type_name: &str,
    convert: fn(&serde_json::Value) -> Option<VmValue>,
) -> Result<VmValue, VmValue> {
    let field = json_field_json(value, name)?;
    convert(&field).ok_or_else(|| {
        json_error_value(format!(
            "JSON field `{name}` is not {} {type_name}",
            json_type_article(type_name)
        ))
    })
}

fn json_as_bool_value(value: serde_json::Value) -> Result<VmValue, VmValue> {
    value
        .as_bool()
        .map(VmValue::Bool)
        .ok_or_else(|| json_error_value("JSON value is not a boolean"))
}

fn json_as_int_value(value: serde_json::Value) -> Result<VmValue, VmValue> {
    value
        .as_i64()
        .map(VmValue::Int)
        .ok_or_else(|| json_error_value("JSON value is not an integer"))
}

fn json_as_string_value(value: serde_json::Value) -> Result<VmValue, VmValue> {
    value
        .as_str()
        .map(VmValue::string)
        .ok_or_else(|| json_error_value("JSON value is not a string"))
}

fn json_optional_field_value(value: &serde_json::Value, name: &str) -> VmValue {
    match value.get(name) {
        Some(value) if value.is_null() => VmValue::OptionNone,
        Some(value) => VmValue::OptionSome(Box::new(VmValue::Json(Rc::new(value.clone())))),
        None => VmValue::OptionNone,
    }
}

fn json_optional_typed_field_value(
    value: &serde_json::Value,
    name: &str,
    type_name: &str,
    convert: fn(&serde_json::Value) -> Option<VmValue>,
) -> Result<VmValue, VmValue> {
    match value.get(name) {
        Some(value) if value.is_null() => Ok(VmValue::OptionNone),
        Some(value) => convert(value)
            .map(|value| VmValue::OptionSome(Box::new(value)))
            .ok_or_else(|| {
                json_error_value(format!(
                    "JSON field `{name}` is not {} {type_name}",
                    json_type_article(type_name)
                ))
            }),
        None => Ok(VmValue::OptionNone),
    }
}

fn json_type_article(type_name: &str) -> &'static str {
    if matches!(
        type_name.as_bytes().first(),
        Some(b'a' | b'e' | b'i' | b'o' | b'u')
    ) {
        "an"
    } else {
        "a"
    }
}

fn json_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("JsonError"),
        fields,
    }))
}

fn json_decode_struct_value(
    unit: &RegUnit,
    type_name: &str,
    value: &serde_json::Value,
) -> Result<VmValue, VmValue> {
    let info = unit
        .types
        .get(type_root_name(type_name))
        .ok_or_else(|| json_error_value(format!("unknown JSON decode type `{type_name}`")))?;
    let object = value
        .as_object()
        .ok_or_else(|| json_error_value("JSON decode expected an object"))?;
    let mut fields = HashMap::with_capacity(info.fields_ordered.len());
    for field in &info.fields_ordered {
        let decoded = match object.get(&field.name) {
            Some(value) => json_decode_field_value(unit, &field.type_name, value)?,
            None if type_root_name(&field.type_name) == "Option" => value_none(),
            None => {
                return Err(json_error_value(format!(
                    "missing JSON field `{}`",
                    field.name
                )));
            }
        };
        fields.insert(field.name.clone(), decoded);
    }
    Ok(VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from(info.name.as_str()),
        fields,
    })))
}

fn json_decode_field_value(
    unit: &RegUnit,
    type_name: &str,
    value: &serde_json::Value,
) -> Result<VmValue, VmValue> {
    match type_root_name(type_name) {
        "Unit" => {
            if value.is_null() {
                Ok(VmValue::Unit)
            } else {
                Err(json_type_error(type_name, value))
            }
        }
        "Int" => value
            .as_i64()
            .map(VmValue::Int)
            .ok_or_else(|| json_type_error(type_name, value)),
        "Float" => value
            .as_f64()
            .map(VmValue::Float)
            .ok_or_else(|| json_type_error(type_name, value)),
        "Bool" => value
            .as_bool()
            .map(VmValue::Bool)
            .ok_or_else(|| json_type_error(type_name, value)),
        "String" => value
            .as_str()
            .map(VmValue::string)
            .ok_or_else(|| json_type_error(type_name, value)),
        "JsonValue" | "JsonLiteral" => Ok(VmValue::Json(Rc::new(value.clone()))),
        "Option" => {
            if value.is_null() {
                return Ok(value_none());
            }
            let Some(inner) = type_arg_names(type_name).and_then(|args| args.first().copied())
            else {
                return Err(json_error_value(format!(
                    "Option field `{type_name}` is missing a type argument"
                )));
            };
            json_decode_field_value(unit, inner, value).map(value_some)
        }
        "List" => {
            let Some(inner) = type_arg_names(type_name).and_then(|args| args.first().copied())
            else {
                return Err(json_error_value(format!(
                    "List field `{type_name}` is missing a type argument"
                )));
            };
            let items = value
                .as_array()
                .ok_or_else(|| json_type_error(type_name, value))?;
            let decoded = items
                .iter()
                .map(|item| json_decode_field_value(unit, inner, item))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(VmValue::List(Rc::new(RefCell::new(decoded))))
        }
        "Map" => {
            let args = type_arg_names(type_name).unwrap_or_default();
            if args.len() != 2 || type_root_name(args[0]) != "String" {
                return Err(json_error_value(format!(
                    "JSON decode only supports Map<String, T> fields in the VM, got `{type_name}`"
                )));
            }
            let object = value
                .as_object()
                .ok_or_else(|| json_type_error(type_name, value))?;
            let mut decoded = HashMap::with_capacity(object.len());
            for (key, value) in object {
                decoded.insert(
                    VmMapKey::String(Rc::new(key.clone())),
                    json_decode_field_value(unit, args[1], value)?,
                );
            }
            Ok(VmValue::Map(Rc::new(RefCell::new(decoded))))
        }
        other if unit.types.contains_key(other) => json_decode_struct_value(unit, other, value),
        _ => Err(json_error_value(format!(
            "JSON decode does not support VM field type `{type_name}`"
        ))),
    }
}

fn json_type_error(expected: &str, value: &serde_json::Value) -> VmValue {
    json_error_value(format!(
        "JSON decode expected `{}` but found `{}`",
        type_root_name(expected),
        json_kind(value)
    ))
}

fn decode_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("DecodeError"),
        fields,
    }))
}

fn config_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("ConfigError"),
        fields,
    }))
}

fn file_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("FileError"),
        fields,
    }))
}

fn channel_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("ChannelError"),
        fields,
    }))
}

fn http_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("HttpError"),
        fields,
    }))
}

fn http_response_value(status: i64, body: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), VmValue::Int(status));
    fields.insert("body".to_string(), VmValue::string(body.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("HttpResponse"),
        fields,
    }))
}

fn http_get_local(url: &str) -> Result<VmValue, VmValue> {
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(http_error_value(format!(
            "HTTP client runtime is not configured for GET {url}"
        )));
    };
    let (host_port, path) = match rest.split_once('/') {
        Some((host_port, path)) => (host_port, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if host_port.is_empty() {
        return Err(http_error_value(format!(
            "HTTP URL is missing a host: `{url}`"
        )));
    }
    let mut stream = TcpStream::connect(host_port).map_err(|error| {
        http_error_value(format!("HTTP connect to `{host_port}` failed: {error}"))
    })?;
    let timeout = Some(std::time::Duration::from_secs(5));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|error| {
        http_error_value(format!("HTTP request write failed for `{url}`: {error}"))
    })?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|error| {
        http_error_value(format!("HTTP response read failed for `{url}`: {error}"))
    })?;
    parse_http_response(&response).map_err(http_error_value)
}

fn parse_http_response(response: &[u8]) -> Result<VmValue, String> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "HTTP response is missing header terminator".to_string())?;
    let headers = String::from_utf8_lossy(&response[..header_end]);
    let status_line = headers
        .lines()
        .next()
        .ok_or_else(|| "HTTP response is missing status line".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("HTTP response status line is invalid: `{status_line}`"))?
        .parse::<i64>()
        .map_err(|error| format!("HTTP response status is invalid: {error}"))?;
    let body = String::from_utf8_lossy(&response[header_end + 4..]).to_string();
    Ok(http_response_value(status, body))
}

fn tcp_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("TcpError"),
        fields,
    }))
}

fn tcp_stream_value(id: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), VmValue::Int(id));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("TcpStream"),
        fields,
    }))
}

fn websocket_value(id: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), VmValue::Int(id));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("WebSocket"),
        fields,
    }))
}

fn websocket_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("WebSocketError"),
        fields,
    }))
}

#[derive(Debug, Clone)]
struct VmChannelState {
    id: i64,
    capacity: i64,
    receiver_taken: bool,
}

impl VmChannelState {
    fn to_value(&self) -> VmValue {
        channel_value(self.id, self.capacity, self.receiver_taken)
    }
}

fn channel_value(id: i64, capacity: i64, receiver_taken: bool) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), VmValue::Int(id));
    fields.insert("capacity".to_string(), VmValue::Int(capacity));
    fields.insert("receiver_taken".to_string(), VmValue::Bool(receiver_taken));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Channel"),
        fields,
    }))
}

#[derive(Debug, Clone)]
struct VmSender {
    channel_id: i64,
    closed: bool,
}

fn sender_value(channel_id: i64, closed: bool) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("channel_id".to_string(), VmValue::Int(channel_id));
    fields.insert("closed".to_string(), VmValue::Bool(closed));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Sender"),
        fields,
    }))
}

#[derive(Debug, Clone)]
struct VmReceiver {
    channel_id: i64,
    closed: bool,
}

fn receiver_value(channel_id: i64, closed: bool) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("channel_id".to_string(), VmValue::Int(channel_id));
    fields.insert("closed".to_string(), VmValue::Bool(closed));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Receiver"),
        fields,
    }))
}

#[derive(Debug, Clone)]
struct VmResourcePoolState {
    id: i64,
}

const POOL_LEASE_ID_FIELD: &str = "__rsscript_vm_pool_id";
const POOL_LEASE_DISCARDED_FIELD: &str = "__rsscript_vm_pool_discarded";

fn resource_pool_value(id: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), VmValue::Int(id));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("ResourcePool"),
        fields,
    }))
}

fn pool_stats_value(capacity: i64, created: i64, available: i64, in_use: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("capacity".to_string(), VmValue::Int(capacity));
    fields.insert("created".to_string(), VmValue::Int(created));
    fields.insert("available".to_string(), VmValue::Int(available));
    fields.insert("in_use".to_string(), VmValue::Int(in_use));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("PoolStats"),
        fields,
    }))
}

fn pool_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("PoolError"),
        fields,
    }))
}

fn pool_error_message(value: &VmValue) -> Option<String> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "PoolError" => data
            .fields
            .get("message")
            .and_then(|value| expect_string_ref(value).ok())
            .map(str::to_string),
        _ => None,
    }
}

fn mark_pool_lease(value: VmValue, pool_id: i64) -> Result<VmValue, String> {
    let VmValue::Struct(data) = value else {
        return Err(format!(
            "ResourcePool can only lease resource structs in the VM, got `{}`",
            value.display()
        ));
    };
    let mut fields = data.fields.clone();
    fields.insert(POOL_LEASE_ID_FIELD.to_string(), VmValue::Int(pool_id));
    fields.insert(POOL_LEASE_DISCARDED_FIELD.to_string(), VmValue::Bool(false));
    Ok(VmValue::Struct(Rc::new(VmStruct {
        name: Rc::clone(&data.name),
        fields,
    })))
}

fn mark_pool_lease_discarded(value: VmValue) -> Result<VmValue, EvalError> {
    let VmValue::Struct(data) = value else {
        return Err(EvalError::Runtime(format!(
            "ResourcePool.discard expected an active pool lease, got `{}`.",
            value.display()
        )));
    };
    if !data.fields.contains_key(POOL_LEASE_ID_FIELD) {
        return Err(EvalError::Runtime(format!(
            "ResourcePool.discard expected an active pool lease, got `{}`.",
            VmValue::Struct(Rc::clone(&data)).display()
        )));
    }
    let mut fields = data.fields.clone();
    fields.insert(POOL_LEASE_DISCARDED_FIELD.to_string(), VmValue::Bool(true));
    Ok(VmValue::Struct(Rc::new(VmStruct {
        name: Rc::clone(&data.name),
        fields,
    })))
}

fn split_pool_lease(value: VmValue) -> Result<Option<VmResourcePoolLease>, EvalError> {
    let VmValue::Struct(data) = value else {
        return Ok(None);
    };
    let mut fields = data.fields.clone();
    let Some(pool_id) = fields.remove(POOL_LEASE_ID_FIELD) else {
        return Ok(None);
    };
    let discarded = fields
        .remove(POOL_LEASE_DISCARDED_FIELD)
        .map(|value| expect_bool_ref(&value))
        .transpose()?
        .unwrap_or(false);
    Ok(Some(VmResourcePoolLease {
        pool_id: expect_int_ref(&pool_id)?,
        discarded,
        value: VmValue::Struct(Rc::new(VmStruct {
            name: Rc::clone(&data.name),
            fields,
        })),
    }))
}

fn clock_system_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_millis() as i64
}

fn instant_value(unix_ms: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("unix_ms".to_string(), VmValue::Int(unix_ms));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Instant"),
        fields,
    }))
}

fn deadline_after_ms(ms: i64) -> i64 {
    clock_system_unix_ms().saturating_add(ms.max(0))
}

fn deadline_value(unix_ms: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("unix_ms".to_string(), VmValue::Int(unix_ms));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Deadline"),
        fields,
    }))
}

fn counter_value(value: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("value".to_string(), VmValue::Int(value));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Counter"),
        fields,
    }))
}

fn config_value(name: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), VmValue::string(name.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("ConfigValue"),
        fields,
    }))
}

fn config_name_from_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("default")
        .to_string()
}

fn config_rules_value(name: impl Into<String>, rule_count: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), VmValue::string(name.into()));
    fields.insert("rule_count".to_string(), VmValue::Int(rule_count));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Config"),
        fields,
    }))
}

fn rule_value(name: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), VmValue::string(name.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Rule"),
        fields,
    }))
}

fn rules_from_text(text: &str) -> Vec<VmValue> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(rule_value)
        .collect()
}

fn environment_value(has_parent: bool, has_function: bool) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("has_parent".to_string(), VmValue::Bool(has_parent));
    fields.insert("has_function".to_string(), VmValue::Bool(has_function));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Environment"),
        fields,
    }))
}

fn function_object_value(has_closure: bool) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("has_closure".to_string(), VmValue::Bool(has_closure));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("FunctionObject"),
        fields,
    }))
}

fn config_store_value(name: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), VmValue::string(name.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("ConfigStore"),
        fields,
    }))
}

fn global_config_value(rule_count: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("rule_count".to_string(), VmValue::Int(rule_count));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("GlobalConfig"),
        fields,
    }))
}

fn request_value(path: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), VmValue::string(path.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Request"),
        fields,
    }))
}

fn response_value(status: i64, body: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), VmValue::Int(status));
    fields.insert("body".to_string(), VmValue::string(body.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Response"),
        fields,
    }))
}

#[derive(Debug, Clone)]
struct VmHttpRequest {
    method: String,
    url: String,
    body: String,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
    header_count: i64,
}

impl VmHttpRequest {
    fn to_value(&self) -> VmValue {
        http_request_value(
            &self.method,
            &self.url,
            &self.body,
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
) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("method".to_string(), VmValue::string(method.into()));
    fields.insert("url".to_string(), VmValue::string(url.into()));
    fields.insert("body".to_string(), VmValue::string(body.into()));
    fields.insert("timeout_ms".to_string(), VmValue::Int(timeout_ms));
    fields.insert("attempts".to_string(), VmValue::Int(attempts));
    fields.insert("backoff_ms".to_string(), VmValue::Int(backoff_ms));
    fields.insert("header_count".to_string(), VmValue::Int(header_count));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("HttpRequest"),
        fields,
    }))
}

enum WebSocketExpectedFrame {
    Text,
    Binary,
}

struct WebSocketFrame {
    opcode: u8,
    payload: Vec<u8>,
}

fn parse_ws_url(url: &str) -> Result<(String, String), VmValue> {
    let Some(rest) = url.strip_prefix("ws://") else {
        return Err(websocket_error_value(format!(
            "WebSocket VM only supports ws URLs, got `{url}`"
        )));
    };
    let (host_port, path) = match rest.split_once('/') {
        Some((host_port, path)) => (host_port, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if host_port.is_empty() {
        return Err(websocket_error_value(format!(
            "WebSocket URL is missing a host: `{url}`"
        )));
    }
    Ok((host_port.to_string(), path))
}

fn websocket_write_frame(
    stream: &mut TcpStream,
    opcode: u8,
    payload: &[u8],
) -> Result<(), VmValue> {
    let mut frame = Vec::new();
    frame.push(0x80 | (opcode & 0x0F));
    let mask_bit = 0x80;
    if payload.len() < 126 {
        frame.push(mask_bit | payload.len() as u8);
    } else if u16::try_from(payload.len()).is_ok() {
        frame.push(mask_bit | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(mask_bit | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    let mask = [0_u8; 4];
    frame.extend_from_slice(&mask);
    frame.extend_from_slice(payload);
    stream
        .write_all(&frame)
        .map_err(|error| websocket_error_value(format!("WebSocket send failed: {error}")))
}

fn websocket_read_frame(stream: &mut TcpStream) -> Result<WebSocketFrame, VmValue> {
    let mut header = [0; 2];
    stream
        .read_exact(&mut header)
        .map_err(|error| websocket_error_value(format!("WebSocket receive failed: {error}")))?;
    let opcode = header[0] & 0x0F;
    let masked = header[1] & 0x80 != 0;
    let mut len = u64::from(header[1] & 0x7F);
    if len == 126 {
        let mut bytes = [0; 2];
        stream.read_exact(&mut bytes).map_err(|error| {
            websocket_error_value(format!("WebSocket frame length read failed: {error}"))
        })?;
        len = u64::from(u16::from_be_bytes(bytes));
    } else if len == 127 {
        let mut bytes = [0; 8];
        stream.read_exact(&mut bytes).map_err(|error| {
            websocket_error_value(format!("WebSocket frame length read failed: {error}"))
        })?;
        len = u64::from_be_bytes(bytes);
    }
    let mut mask = [0; 4];
    if masked {
        stream.read_exact(&mut mask).map_err(|error| {
            websocket_error_value(format!("WebSocket frame mask read failed: {error}"))
        })?;
    }
    let len = usize::try_from(len)
        .map_err(|_| websocket_error_value("WebSocket frame payload is too large"))?;
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).map_err(|error| {
        websocket_error_value(format!("WebSocket frame payload read failed: {error}"))
    })?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(WebSocketFrame { opcode, payload })
}

#[derive(Debug, Clone)]
struct VmDbConnection {
    url: String,
    queries: Vec<String>,
}

impl VmDbConnection {
    fn to_value(&self) -> VmValue {
        db_connection_value(self.url.clone(), self.queries.clone())
    }
}

fn db_connection_value(url: impl Into<String>, queries: Vec<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("url".to_string(), VmValue::string(url.into()));
    fields.insert(
        "queries".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            queries.into_iter().map(VmValue::string).collect(),
        ))),
    );
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("DbConnection"),
        fields,
    }))
}

fn db_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("DbError"),
        fields,
    }))
}

#[derive(Debug, Clone)]
struct VmProcessOutput {
    status: i64,
    stdout: String,
    stderr: String,
    merged: String,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct VmProcessRequest {
    command: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    stdin: Option<String>,
    env: Vec<(String, String)>,
    timeout_ms: i64,
    merge_stderr: bool,
    output_cap_bytes: i64,
}

fn process_run_output(command: &str, args: &[String]) -> Result<VmProcessOutput, VmValue> {
    std::process::Command::new(command)
        .args(args)
        .output()
        .map(process_output_state)
        .map_err(|error| VmValue::string(error.to_string()))
}

fn process_run_request(request: &VmProcessRequest) -> Result<VmProcessOutput, VmValue> {
    if request.command.trim().is_empty() {
        return Err(VmValue::string("process command must not be empty"));
    }
    let mut command = std::process::Command::new(&request.command);
    command
        .args(&request.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if request.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    for (name, value) in &request.env {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|error| {
        VmValue::string(format!("failed to run `{}`: {error}", request.command))
    })?;
    if let Some(stdin) = &request.stdin
        && let Some(mut child_stdin) = child.stdin.take()
    {
        child_stdin.write_all(stdin.as_bytes()).map_err(|error| {
            VmValue::string(format!(
                "failed to write stdin for `{}`: {error}",
                request.command
            ))
        })?;
    }
    let output = child.wait_with_output().map_err(|error| {
        VmValue::string(format!("failed to wait for `{}`: {error}", request.command))
    })?;
    Ok(process_output_state_with_capture(
        output,
        request.output_cap_bytes,
        request.merge_stderr,
    ))
}

fn process_output_state(output: std::process::Output) -> VmProcessOutput {
    process_output_state_with_capture(output, 0, false)
}

fn process_output_state_with_capture(
    output: std::process::Output,
    output_cap_bytes: i64,
    merge_stderr: bool,
) -> VmProcessOutput {
    let status = output.status.code().unwrap_or(-1) as i64;
    let cap = usize::try_from(output_cap_bytes)
        .ok()
        .filter(|value| *value > 0);
    let mut capture = VmProcessCapture::new(cap, merge_stderr);
    capture.push(false, &output.stdout);
    capture.push(true, &output.stderr);
    let stdout = String::from_utf8_lossy(&capture.stdout).to_string();
    let stderr = String::from_utf8_lossy(&capture.stderr).to_string();
    let merged = String::from_utf8_lossy(&capture.merged).to_string();
    VmProcessOutput {
        status,
        stdout,
        stderr,
        merged,
        truncated: capture.truncated,
    }
}

struct VmProcessCapture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    merged: Vec<u8>,
    cap: Option<usize>,
    used: usize,
    merge_stderr: bool,
    truncated: bool,
}

impl VmProcessCapture {
    fn new(cap: Option<usize>, merge_stderr: bool) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            merged: Vec::new(),
            cap,
            used: 0,
            merge_stderr,
            truncated: false,
        }
    }

    fn push(&mut self, stderr: bool, bytes: &[u8]) {
        let bytes = self.capped_bytes(bytes).to_vec();
        if bytes.is_empty() {
            return;
        }
        if stderr {
            self.stderr.extend_from_slice(&bytes);
        } else {
            self.stdout.extend_from_slice(&bytes);
        }
        if self.merge_stderr || !stderr {
            self.merged.extend_from_slice(&bytes);
        }
    }

    fn capped_bytes<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        let Some(cap) = self.cap else {
            return bytes;
        };
        if self.used >= cap {
            self.truncated = true;
            return &bytes[..0];
        }
        let remaining = cap - self.used;
        if bytes.len() > remaining {
            self.truncated = true;
            self.used = cap;
            &bytes[..remaining]
        } else {
            self.used += bytes.len();
            bytes
        }
    }
}

fn process_stdout_result(command: &str, output: VmProcessOutput) -> Result<String, VmValue> {
    if output.status == 0 {
        Ok(output.stdout)
    } else {
        Err(VmValue::string(format!(
            "process `{command}` exited with status {}: {}",
            output.status,
            process_output_details(&output.stdout, &output.stderr)
        )))
    }
}

fn process_run_many_stdout(
    command: &str,
    args: &[String],
    appended: &[String],
) -> Result<Vec<String>, VmValue> {
    let mut values = Vec::with_capacity(appended.len());
    for item in appended {
        let mut command_args = args.to_vec();
        command_args.push(item.clone());
        let output = process_run_output(command, &command_args)?;
        values.push(process_stdout_result(command, output)?);
    }
    Ok(values)
}

fn process_output_details(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.trim().to_string(),
        (true, false) => stderr.trim().to_string(),
        (false, false) => format!("{} {}", stdout.trim(), stderr.trim()),
    }
}

fn process_output_value(output: VmProcessOutput) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), VmValue::Int(output.status));
    fields.insert("stdout".to_string(), VmValue::string(output.stdout));
    fields.insert("stderr".to_string(), VmValue::string(output.stderr));
    fields.insert("merged".to_string(), VmValue::string(output.merged));
    fields.insert("truncated".to_string(), VmValue::Bool(output.truncated));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("ProcessOutput"),
        fields,
    }))
}

fn process_event_value(kind: &str, data: &str, status: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("kind".to_string(), VmValue::string(kind));
    fields.insert("data".to_string(), VmValue::string(data));
    fields.insert("status".to_string(), VmValue::Int(status));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("ProcessEvent"),
        fields,
    }))
}

#[derive(Debug, Clone)]
struct VmFileState {
    path: String,
    mode: String,
    cursor: u64,
}

impl VmFileState {
    fn to_value(&self) -> VmValue {
        file_value(&self.path, &self.mode, self.cursor)
    }
}

fn file_value(path: impl Into<String>, mode: impl Into<String>, cursor: u64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), VmValue::string(path.into()));
    fields.insert("mode".to_string(), VmValue::string(mode.into()));
    fields.insert("cursor".to_string(), VmValue::Int(cursor as i64));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("File"),
        fields,
    }))
}

fn file_read_remaining(file: &mut VmFileState) -> std::io::Result<Vec<u8>> {
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

fn file_write_at_cursor(file: &mut VmFileState, data: &[u8]) -> std::io::Result<()> {
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

fn file_result_unit(value: std::io::Result<()>) -> VmValue {
    json_result(
        value
            .map(|_| VmValue::Unit)
            .map_err(|error| file_error_value(error.to_string())),
    )
}

fn file_atomic_write_result(path: PathBuf, text: &str) -> VmValue {
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

fn file_append(path: &str, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    handle.write_all(data)
}

fn file_bytes_stream_value(path: &str, chunk_size: i64) -> Result<VmValue, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("file byte stream open failed: {error}"))?;
    let chunk_size = chunk_size.max(1) as usize;
    Ok(stream_value(
        bytes
            .chunks(chunk_size)
            .map(|chunk| VmValue::Bytes(Rc::new(chunk.to_vec())))
            .collect(),
    ))
}

fn file_metadata_value(metadata: std::fs::Metadata) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("is_file".to_string(), VmValue::Bool(metadata.is_file()));
    fields.insert("is_dir".to_string(), VmValue::Bool(metadata.is_dir()));
    fields.insert("len".to_string(), VmValue::Int(metadata.len() as i64));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("FileMetadata"),
        fields,
    }))
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

fn tempdir_value(path: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), VmValue::string(path.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("TempDir"),
        fields,
    }))
}

fn tempdir_new_value(parent: PathBuf) -> Result<VmValue, VmValue> {
    let seed = clock_system_unix_ms();
    for attempt in 0..100 {
        let path = parent.join(format!("rsscript-{}-{seed}-{attempt}", std::process::id()));
        match std::fs::create_dir_all(&path) {
            Ok(()) => return Ok(tempdir_value(path.to_string_lossy())),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(file_error_value(error.to_string())),
        }
    }
    Err(file_error_value(
        "could not allocate unique TempDir path".to_string(),
    ))
}

struct ImageState {
    bytes: Vec<u8>,
    width: Option<i64>,
    height: Option<i64>,
    operations: Vec<String>,
}

impl ImageState {
    fn to_value(&self) -> VmValue {
        image_value(
            self.bytes.clone(),
            self.width,
            self.height,
            self.operations.clone(),
        )
    }

    fn saved_bytes(&self) -> Vec<u8> {
        let mut bytes = self.bytes.clone();
        bytes.extend_from_slice(b"\n# rsscript-image-ops:");
        bytes.extend_from_slice(self.operations.join(",").as_bytes());
        if let (Some(width), Some(height)) = (self.width, self.height) {
            bytes.extend_from_slice(format!(";size={width}x{height}").as_bytes());
        }
        bytes
    }

    fn inspect_line(&self) -> String {
        let size = self
            .width
            .zip(self.height)
            .map(|(width, height)| format!("{width}x{height}"))
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "image bytes={} size={} ops={}",
            self.bytes.len(),
            size,
            self.operations.join(",")
        )
    }
}

fn image_value(
    bytes: Vec<u8>,
    width: Option<i64>,
    height: Option<i64>,
    operations: Vec<String>,
) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("bytes".to_string(), VmValue::Bytes(Rc::new(bytes)));
    fields.insert(
        "width".to_string(),
        width
            .map(|value| value_some(VmValue::Int(value)))
            .unwrap_or_else(value_none),
    );
    fields.insert(
        "height".to_string(),
        height
            .map(|value| value_some(VmValue::Int(value)))
            .unwrap_or_else(value_none),
    );
    fields.insert(
        "operations".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            operations.into_iter().map(VmValue::string).collect(),
        ))),
    );
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Image"),
        fields,
    }))
}

fn image_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("ImageError"),
        fields,
    }))
}

fn cancellation_source_value(id: i64) -> VmValue {
    cancellation_handle_value("CancellationSource", id)
}

fn cancellation_token_value(id: i64) -> VmValue {
    cancellation_handle_value("CancellationToken", id)
}

fn cancellation_handle_value(name: &'static str, id: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), VmValue::Int(id));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from(name),
        fields,
    }))
}

fn stream_value(items: Vec<VmValue>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(items))),
    );
    fields.insert("collect_error".to_string(), VmValue::OptionNone);
    fields.insert("channel_id".to_string(), VmValue::OptionNone);
    fields.insert("stream_id".to_string(), VmValue::OptionNone);
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Stream"),
        fields,
    }))
}

fn stream_channel_value(channel_id: i64) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(Vec::new()))),
    );
    fields.insert("collect_error".to_string(), VmValue::OptionNone);
    fields.insert(
        "channel_id".to_string(),
        VmValue::OptionSome(Box::new(VmValue::Int(channel_id))),
    );
    fields.insert("stream_id".to_string(), VmValue::OptionNone);
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Stream"),
        fields,
    }))
}

fn stream_collect_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(Vec::new()))),
    );
    fields.insert(
        "collect_error".to_string(),
        VmValue::OptionSome(Box::new(VmValue::string(message.into()))),
    );
    fields.insert("channel_id".to_string(), VmValue::OptionNone);
    fields.insert("stream_id".to_string(), VmValue::OptionNone);
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Stream"),
        fields,
    }))
}

#[derive(Debug, Clone)]
struct VmStreamState {
    items: Rc<RefCell<Vec<VmValue>>>,
    collect_error: Option<String>,
    channel_id: Option<i64>,
}

fn regex_value(pattern: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("pattern".to_string(), VmValue::string(pattern.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Regex"),
        fields,
    }))
}

fn regex_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("RegexError"),
        fields,
    }))
}

fn csv_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("CsvError"),
        fields,
    }))
}

fn row_buffer_value(bytes: Vec<u8>) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("bytes".to_string(), VmValue::Bytes(Rc::new(bytes)));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("RowBuffer"),
        fields,
    }))
}

fn row_value(fields: Vec<String>) -> VmValue {
    let mut row_fields = HashMap::new();
    row_fields.insert(
        "fields".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            fields.into_iter().map(VmValue::string).collect(),
        ))),
    );
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Row"),
        fields: row_fields,
    }))
}

fn csv_parse_row_value(bytes: &[u8]) -> Result<VmValue, VmValue> {
    let text = std::str::from_utf8(bytes).map_err(|error| csv_error_value(error.to_string()))?;
    let Some(line) = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .nth(1)
        .or_else(|| text.lines().map(str::trim).find(|line| !line.is_empty()))
    else {
        return Err(csv_error_value("CSV buffer is empty"));
    };
    Ok(row_value(
        line.split(',')
            .map(|field| field.trim().to_string())
            .collect(),
    ))
}

fn csv_rows_stream_value(path: &str) -> Result<VmValue, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("CSV row stream open failed: {error}"))?;
    let mut skipped_header = false;
    let mut rows = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !skipped_header {
            skipped_header = true;
            continue;
        }
        rows.push(row_value(
            line.split(',')
                .map(|field| field.trim().to_string())
                .collect(),
        ));
    }
    Ok(stream_value(rows))
}

fn row_field_string_value(fields: Vec<String>, index: i64) -> Result<VmValue, VmValue> {
    let index = usize::try_from(index).map_err(|_| csv_error_value("negative CSV field index"))?;
    fields
        .get(index)
        .cloned()
        .map(VmValue::string)
        .ok_or_else(|| csv_error_value(format!("CSV field index `{index}` is out of bounds")))
}

fn yaml_parse_json_value(text: &str) -> Result<VmValue, VmValue> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|error| json_error_value(error.to_string()))?;
    serde_json::to_value(value)
        .map(|value| VmValue::Json(Rc::new(value)))
        .map_err(|error| json_error_value(error.to_string()))
}

fn toml_parse_file_value(path: &str) -> Result<VmValue, VmValue> {
    std::fs::read_to_string(path)
        .map_err(|error| json_error_value(error.to_string()))
        .and_then(|text| {
            text.parse::<toml::Value>()
                .map_err(|error| json_error_value(error.to_string()))
        })
        .and_then(|value| {
            serde_json::to_value(value)
                .map(|value| VmValue::Json(Rc::new(value)))
                .map_err(|error| json_error_value(error.to_string()))
        })
}

fn split_text_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let trimmed = value.strip_suffix('\n').unwrap_or(value);
    trimmed.split('\n').map(ToString::to_string).collect()
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
            if hunk_line.starts_with("@@ ")
                || hunk_line.starts_with("--- ")
                || hunk_line.starts_with("+++ ")
            {
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

fn json_result(value: Result<VmValue, VmValue>) -> VmValue {
    match value {
        Ok(value) => value_ok(value),
        Err(error) => value_err(error),
    }
}

fn vm_value_to_json_literal(value: &VmValue) -> Result<serde_json::Value, EvalError> {
    match value {
        VmValue::Unit => Ok(serde_json::Value::Null),
        VmValue::Int(value) => Ok(serde_json::Value::Number((*value).into())),
        VmValue::Bool(value) => Ok(serde_json::Value::Bool(*value)),
        VmValue::Char(value) => Ok(serde_json::Value::String(value.to_string())),
        VmValue::Bytes(value) => Ok(serde_json::Value::Array(
            value
                .iter()
                .map(|value| serde_json::Value::Number((*value).into()))
                .collect(),
        )),
        VmValue::String(value) => Ok(serde_json::Value::String(value.to_string())),
        VmValue::Json(value) => Ok(value.as_ref().clone()),
        VmValue::List(items) => items
            .borrow()
            .iter()
            .map(vm_value_to_json_literal)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        VmValue::Map(entries) => {
            let mut object = serde_json::Map::new();
            for (key, value) in entries.borrow().iter() {
                let VmMapKey::String(key) = key else {
                    return Err(EvalError::Runtime(format!(
                        "cannot convert map key `{}` to a JSON literal.",
                        key.display()
                    )));
                };
                object.insert(key.to_string(), vm_value_to_json_literal(value)?);
            }
            Ok(serde_json::Value::Object(object))
        }
        other => Err(EvalError::Runtime(format!(
            "cannot convert `{}` to a JSON literal.",
            other.display()
        ))),
    }
}

fn read_field_ref(value: &VmValue, field: &str) -> Result<VmValue, EvalError> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            data.fields.get(field).cloned().ok_or_else(|| {
                EvalError::Runtime(format!("reg VM struct value is missing field `{field}`."))
            })
        }
        VmValue::Managed(value) => read_field_ref(&value.borrow(), field),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Struct for field `{field}`, got `{}`.",
            other.display()
        ))),
    }
}

fn unmanage_vm_value(value: VmValue) -> VmValue {
    match value {
        VmValue::Managed(value) => unmanage_vm_value(value.borrow().clone()),
        other => other,
    }
}

fn native_value_from_vm_value(value: VmValue) -> Result<NativeValue, EvalError> {
    match unmanage_vm_value(value) {
        VmValue::Unit => Ok(NativeValue::Unit),
        VmValue::Int(value) => Ok(NativeValue::Int(value)),
        VmValue::Float(value) => Ok(NativeValue::Float(value)),
        VmValue::Bool(value) => Ok(NativeValue::Bool(value)),
        VmValue::Char(value) => Ok(NativeValue::Char(value)),
        VmValue::Bytes(value) => Ok(NativeValue::Bytes(value.as_ref().clone())),
        VmValue::String(value) => Ok(NativeValue::String(value.to_string())),
        VmValue::Json(value) => Ok(NativeValue::Json(value.as_ref().clone())),
        VmValue::List(items) => items
            .borrow()
            .iter()
            .cloned()
            .map(native_value_from_vm_value)
            .collect::<Result<Vec<_>, _>>()
            .map(NativeValue::List),
        VmValue::Map(entries) => entries
            .borrow()
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.native_value(),
                    native_value_from_vm_value(value.clone())?,
                ))
            })
            .collect::<Result<Vec<_>, EvalError>>()
            .map(NativeValue::Map),
        VmValue::Struct(data) => data
            .fields
            .iter()
            .map(|(field, value)| Ok((field.clone(), native_value_from_vm_value(value.clone())?)))
            .collect::<Result<BTreeMap<_, _>, EvalError>>()
            .map(|fields| NativeValue::Struct {
                name: data.name.to_string(),
                fields,
            }),
        VmValue::Variant(data) => data
            .fields
            .iter()
            .map(|(field, value)| Ok((field.clone(), native_value_from_vm_value(value.clone())?)))
            .collect::<Result<BTreeMap<_, _>, EvalError>>()
            .map(|fields| NativeValue::Variant {
                name: data.name.to_string(),
                fields,
            }),
        VmValue::Native(data) => Ok(NativeValue::Native {
            type_name: data.type_name.to_string(),
            id: data.id,
        }),
        VmValue::Managed(_) => Err(EvalError::Runtime(
            "reg VM native argument stayed managed after unwrapping.".to_string(),
        )),
        VmValue::OptionSome(_) | VmValue::OptionNone | VmValue::Closure(_) => {
            Err(EvalError::Runtime(
                "reg VM cannot pass Option or Closure to native host binding.".to_string(),
            ))
        }
    }
}

fn vm_value_from_native_value(value: NativeValue) -> VmValue {
    match value {
        NativeValue::Unit => VmValue::Unit,
        NativeValue::Int(value) => VmValue::Int(value),
        NativeValue::Float(value) => VmValue::Float(value),
        NativeValue::Bool(value) => VmValue::Bool(value),
        NativeValue::String(value) => VmValue::string(value),
        NativeValue::Char(value) => VmValue::Char(value),
        NativeValue::Bytes(value) => VmValue::Bytes(Rc::new(value)),
        NativeValue::List(items) => VmValue::List(Rc::new(RefCell::new(
            items.into_iter().map(vm_value_from_native_value).collect(),
        ))),
        NativeValue::Map(entries) => VmValue::Map(Rc::new(RefCell::new(
            entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        vm_map_key_from_native_value(key),
                        vm_value_from_native_value(value),
                    )
                })
                .collect(),
        ))),
        NativeValue::Json(value) => VmValue::Json(Rc::new(value)),
        NativeValue::Struct { name, fields } => VmValue::Struct(Rc::new(VmStruct {
            name: Rc::from(name.as_str()),
            fields: fields
                .into_iter()
                .map(|(field, value)| (field, vm_value_from_native_value(value)))
                .collect(),
        })),
        NativeValue::Variant { name, fields } => VmValue::Variant(Rc::new(VmStruct {
            name: Rc::from(name.as_str()),
            fields: fields
                .into_iter()
                .map(|(field, value)| (field, vm_value_from_native_value(value)))
                .collect(),
        })),
        NativeValue::Native { type_name, id } => VmValue::Native(Rc::new(VmNative {
            type_name: Rc::from(type_name.as_str()),
            id,
        })),
    }
}

fn vm_map_key_from_native_value(value: NativeValue) -> VmMapKey {
    match value {
        NativeValue::Bool(value) => VmMapKey::Bool(value),
        NativeValue::Int(value) => VmMapKey::Int(value),
        NativeValue::String(value) => VmMapKey::String(Rc::new(value)),
        other => VmMapKey::String(Rc::new(format!("{other:?}"))),
    }
}

fn expect_environment_state(value: &VmValue) -> Result<(bool, bool), EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Environment" => {
            let has_parent = data.fields.get("has_parent").ok_or_else(|| {
                EvalError::Runtime("Environment value is missing has_parent.".to_string())
            })?;
            let has_function = data.fields.get("has_function").ok_or_else(|| {
                EvalError::Runtime("Environment value is missing has_function.".to_string())
            })?;
            Ok((expect_bool_ref(has_parent)?, expect_bool_ref(has_function)?))
        }
        VmValue::Managed(value) => expect_environment_state(&value.borrow()),
        other => Err(EvalError::Runtime(format!(
            "expected Environment, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_function_has_closure(value: &VmValue) -> Result<bool, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "FunctionObject" => {
            let value = data.fields.get("has_closure").ok_or_else(|| {
                EvalError::Runtime("FunctionObject value is missing has_closure.".to_string())
            })?;
            expect_bool_ref(value)
        }
        VmValue::Managed(value) => expect_function_has_closure(&value.borrow()),
        other => Err(EvalError::Runtime(format!(
            "expected FunctionObject, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_counter_value(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Counter" => {
            let value = data
                .fields
                .get("value")
                .ok_or_else(|| EvalError::Runtime("Counter value is missing.".to_string()))?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Counter, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_instant_unix_ms(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Instant" => {
            let value = data.fields.get("unix_ms").ok_or_else(|| {
                EvalError::Runtime("Instant value is missing unix_ms.".to_string())
            })?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Instant, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_deadline_unix_ms(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Deadline" => {
            let value = data.fields.get("unix_ms").ok_or_else(|| {
                EvalError::Runtime("Deadline value is missing unix_ms.".to_string())
            })?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Deadline, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_db_connection_ref(value: &VmValue) -> Result<VmDbConnection, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "DbConnection" => {
            let url = data.fields.get("url").ok_or_else(|| {
                EvalError::Runtime("DbConnection value is missing url.".to_string())
            })?;
            let queries = data.fields.get("queries").ok_or_else(|| {
                EvalError::Runtime("DbConnection value is missing queries.".to_string())
            })?;
            Ok(VmDbConnection {
                url: expect_string_ref(url)?.to_string(),
                queries: expect_string_list_ref(queries)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected DbConnection, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_cancellation_id_ref(value: &VmValue, expected_name: &str) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == expected_name => {
            let value = data.fields.get("id").ok_or_else(|| {
                EvalError::Runtime(format!("{expected_name} value is missing id."))
            })?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected {expected_name}, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_channel_ref(value: &VmValue) -> Result<VmChannelState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Channel" => {
            let int_field = |name: &str| {
                data.fields
                    .get(name)
                    .ok_or_else(|| EvalError::Runtime(format!("Channel value is missing {name}.")))
                    .and_then(expect_int_ref)
            };
            let receiver_taken = data
                .fields
                .get("receiver_taken")
                .ok_or_else(|| {
                    EvalError::Runtime("Channel value is missing receiver_taken.".to_string())
                })
                .and_then(expect_bool_ref)?;
            Ok(VmChannelState {
                id: int_field("id")?,
                capacity: int_field("capacity")?,
                receiver_taken,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Channel, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_sender_ref(value: &VmValue) -> Result<VmSender, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Sender" => {
            let channel_id = data.fields.get("channel_id").ok_or_else(|| {
                EvalError::Runtime("Sender value is missing channel_id.".to_string())
            })?;
            let closed = data
                .fields
                .get("closed")
                .ok_or_else(|| EvalError::Runtime("Sender value is missing closed.".to_string()))?;
            Ok(VmSender {
                channel_id: expect_int_ref(channel_id)?,
                closed: expect_bool_ref(closed)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Sender, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_receiver_ref(value: &VmValue) -> Result<VmReceiver, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Receiver" => {
            let channel_id = data.fields.get("channel_id").ok_or_else(|| {
                EvalError::Runtime("Receiver value is missing channel_id.".to_string())
            })?;
            let closed = data.fields.get("closed").ok_or_else(|| {
                EvalError::Runtime("Receiver value is missing closed.".to_string())
            })?;
            Ok(VmReceiver {
                channel_id: expect_int_ref(channel_id)?,
                closed: expect_bool_ref(closed)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Receiver, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_resource_pool_ref(value: &VmValue) -> Result<VmResourcePoolState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "ResourcePool" => {
            let id = data.fields.get("id").ok_or_else(|| {
                EvalError::Runtime("ResourcePool value is missing id.".to_string())
            })?;
            Ok(VmResourcePoolState {
                id: expect_int_ref(id)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected ResourcePool, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_stream_ref(value: &VmValue) -> Result<VmStreamState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Stream" => {
            let items = data
                .fields
                .get("items")
                .ok_or_else(|| EvalError::Runtime("Stream value is missing items.".to_string()))
                .and_then(expect_list_ref)?;
            let collect_error = data
                .fields
                .get("collect_error")
                .map(option_payload_ref)
                .transpose()?
                .flatten()
                .map(|value| expect_string_ref(value).map(str::to_string))
                .transpose()?;
            let channel_id = data
                .fields
                .get("channel_id")
                .map(option_payload_ref)
                .transpose()?
                .flatten()
                .map(expect_int_ref)
                .transpose()?;
            Ok(VmStreamState {
                items,
                collect_error,
                channel_id,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Stream, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_tcp_stream_id_ref(value: &VmValue) -> Result<i64, EvalError> {
    expect_id_struct_ref(value, "TcpStream")
}

fn expect_websocket_id_ref(value: &VmValue) -> Result<i64, EvalError> {
    expect_id_struct_ref(value, "WebSocket")
}

fn expect_id_struct_ref(value: &VmValue, expected_name: &str) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == expected_name => {
            let id = data.fields.get("id").ok_or_else(|| {
                EvalError::Runtime(format!("{expected_name} value is missing id."))
            })?;
            expect_int_ref(id)
        }
        other => Err(EvalError::Runtime(format!(
            "expected {expected_name}, got `{}`.",
            other.display()
        ))),
    }
}

fn option_payload_ref(value: &VmValue) -> Result<Option<&VmValue>, EvalError> {
    match value {
        VmValue::OptionSome(value) => Ok(Some(value.as_ref())),
        VmValue::OptionNone => Ok(None),
        VmValue::Variant(data) if data.name.as_ref() == "Some" => data
            .fields
            .get("value")
            .map(Some)
            .ok_or_else(|| EvalError::Runtime("Some value is missing.".to_string())),
        VmValue::Variant(data) if data.name.as_ref() == "None" => Ok(None),
        other => Err(EvalError::Runtime(format!(
            "expected Option, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_process_request_ref(value: &VmValue) -> Result<VmProcessRequest, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "ProcessRequest" => {
            let command = data.fields.get("command").ok_or_else(|| {
                EvalError::Runtime("ProcessRequest command is missing.".to_string())
            })?;
            let args = data.fields.get("args").ok_or_else(|| {
                EvalError::Runtime("ProcessRequest args are missing.".to_string())
            })?;
            let cwd = data
                .fields
                .get("cwd")
                .map(option_payload_ref)
                .transpose()?
                .flatten()
                .map(|value| expect_string_ref(value).map(PathBuf::from))
                .transpose()?;
            let stdin = data
                .fields
                .get("stdin")
                .map(option_payload_ref)
                .transpose()?
                .flatten()
                .map(|value| expect_string_ref(value).map(str::to_string))
                .transpose()?;
            let env = data
                .fields
                .get("env")
                .map(expect_process_env_list_ref)
                .transpose()?
                .unwrap_or_default();
            let timeout_ms = data
                .fields
                .get("timeout_ms")
                .map(expect_int_ref)
                .transpose()?
                .unwrap_or(0);
            let merge_stderr = data
                .fields
                .get("merge_stderr")
                .map(expect_bool_ref)
                .transpose()?
                .unwrap_or(false);
            let output_cap_bytes = data
                .fields
                .get("output_cap_bytes")
                .map(expect_int_ref)
                .transpose()?
                .unwrap_or(0);
            Ok(VmProcessRequest {
                command: expect_string_ref(command)?.to_string(),
                args: expect_string_list_ref(args)?,
                cwd,
                stdin,
                env,
                timeout_ms,
                merge_stderr,
                output_cap_bytes,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected ProcessRequest, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_process_env_list_ref(value: &VmValue) -> Result<Vec<(String, String)>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow()
        .iter()
        .map(|value| match value {
            VmValue::Struct(data) if data.name.as_ref() == "ProcessEnv" => {
                let name = data
                    .fields
                    .get("name")
                    .ok_or_else(|| EvalError::Runtime("ProcessEnv name is missing.".to_string()))?;
                let value = data.fields.get("value").ok_or_else(|| {
                    EvalError::Runtime("ProcessEnv value is missing.".to_string())
                })?;
                Ok((
                    expect_string_ref(name)?.to_string(),
                    expect_string_ref(value)?.to_string(),
                ))
            }
            other => Err(EvalError::Runtime(format!(
                "expected ProcessEnv, got `{}`.",
                other.display()
            ))),
        })
        .collect()
}

fn expect_row_buffer_bytes_ref(value: &VmValue) -> Result<Vec<u8>, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "RowBuffer" => data
            .fields
            .get("bytes")
            .ok_or_else(|| EvalError::Runtime("RowBuffer value is missing bytes.".to_string()))
            .and_then(expect_bytes_ref)
            .map(|bytes| bytes.to_vec()),
        other => Err(EvalError::Runtime(format!(
            "expected RowBuffer, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_row_fields_ref(value: &VmValue) -> Result<Vec<String>, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Row" => {
            let fields = data
                .fields
                .get("fields")
                .ok_or_else(|| EvalError::Runtime("Row value is missing fields.".to_string()))?;
            expect_string_list_ref(fields)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Row, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_file_ref(value: &VmValue) -> Result<VmFileState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "File" => {
            let string_field = |name: &str| {
                data.fields
                    .get(name)
                    .ok_or_else(|| EvalError::Runtime(format!("File {name} is missing.")))
                    .and_then(expect_string_ref)
                    .map(str::to_string)
            };
            let cursor = data
                .fields
                .get("cursor")
                .ok_or_else(|| EvalError::Runtime("File cursor is missing.".to_string()))
                .and_then(expect_int_ref)?;
            Ok(VmFileState {
                path: string_field("path")?,
                mode: string_field("mode")?,
                cursor: cursor.max(0) as u64,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected File, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_tempdir_path_ref(value: &VmValue) -> Result<String, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "TempDir" => data
            .fields
            .get("path")
            .ok_or_else(|| EvalError::Runtime("TempDir path is missing.".to_string()))
            .and_then(expect_string_ref)
            .map(str::to_string),
        other => Err(EvalError::Runtime(format!(
            "expected TempDir, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_config_value_name(value: &VmValue) -> Result<String, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "ConfigValue" => {
            let value = data
                .fields
                .get("name")
                .ok_or_else(|| EvalError::Runtime("ConfigValue name is missing.".to_string()))?;
            Ok(expect_string_ref(value)?.to_string())
        }
        other => Err(EvalError::Runtime(format!(
            "expected ConfigValue, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_config_store_name(value: &VmValue) -> Result<String, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "ConfigStore" => {
            let value = data
                .fields
                .get("name")
                .ok_or_else(|| EvalError::Runtime("ConfigStore name is missing.".to_string()))?;
            Ok(expect_string_ref(value)?.to_string())
        }
        other => Err(EvalError::Runtime(format!(
            "expected ConfigStore, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_config_rule_count(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Config" => {
            let value = data
                .fields
                .get("rule_count")
                .ok_or_else(|| EvalError::Runtime("Config rule_count is missing.".to_string()))?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Config, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_global_config_rule_count(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "GlobalConfig" => {
            let value = data.fields.get("rule_count").ok_or_else(|| {
                EvalError::Runtime("GlobalConfig rule_count is missing.".to_string())
            })?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected GlobalConfig, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_image_state(value: &VmValue) -> Result<ImageState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Image" => {
            let bytes = data
                .fields
                .get("bytes")
                .ok_or_else(|| EvalError::Runtime("Image value is missing bytes.".to_string()))?;
            let width = data
                .fields
                .get("width")
                .ok_or_else(|| EvalError::Runtime("Image value is missing width.".to_string()))?;
            let height = data
                .fields
                .get("height")
                .ok_or_else(|| EvalError::Runtime("Image value is missing height.".to_string()))?;
            let operations = data.fields.get("operations").ok_or_else(|| {
                EvalError::Runtime("Image value is missing operations.".to_string())
            })?;
            Ok(ImageState {
                bytes: expect_bytes_ref(bytes)?.to_vec(),
                width: option_int_payload(width)?,
                height: option_int_payload(height)?,
                operations: expect_string_list_ref(operations)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Image, got `{}`.",
            other.display()
        ))),
    }
}

fn option_int_payload(value: &VmValue) -> Result<Option<i64>, EvalError> {
    match value {
        VmValue::Variant(data) if data.name.as_ref() == "Some" => data
            .fields
            .get("value")
            .ok_or_else(|| EvalError::Runtime("Some value is missing.".to_string()))
            .and_then(expect_int_ref)
            .map(Some),
        VmValue::Variant(data) if data.name.as_ref() == "None" => Ok(None),
        VmValue::OptionSome(value) => expect_int_ref(value).map(Some),
        VmValue::OptionNone => Ok(None),
        other => Err(EvalError::Runtime(format!(
            "expected Option<Int>, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_http_request_ref(value: &VmValue) -> Result<VmHttpRequest, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "HttpRequest" => {
            let string_field = |name: &str| {
                data.fields
                    .get(name)
                    .ok_or_else(|| EvalError::Runtime(format!("HttpRequest {name} is missing.")))
                    .and_then(expect_string_ref)
                    .map(str::to_string)
            };
            let int_field = |name: &str| {
                data.fields
                    .get(name)
                    .ok_or_else(|| EvalError::Runtime(format!("HttpRequest {name} is missing.")))
                    .and_then(expect_int_ref)
            };
            Ok(VmHttpRequest {
                method: string_field("method")?,
                url: string_field("url")?,
                body: string_field("body")?,
                timeout_ms: int_field("timeout_ms")?,
                attempts: int_field("attempts")?,
                backoff_ms: int_field("backoff_ms")?,
                header_count: int_field("header_count")?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected HttpRequest, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_regex_ref(value: &VmValue) -> Result<regex::Regex, EvalError> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "Regex" => {
            let pattern = data
                .fields
                .get("pattern")
                .ok_or_else(|| EvalError::Runtime("Regex value is missing pattern.".to_string()))?;
            regex::Regex::new(expect_string_ref(pattern)?)
                .map_err(|error| EvalError::Runtime(error.to_string()))
        }
        other => Err(EvalError::Runtime(format!(
            "expected Regex, got `{}`.",
            other.display()
        ))),
    }
}

fn result_variant_payload(value: &VmValue) -> Result<Result<VmValue, VmValue>, EvalError> {
    match value {
        VmValue::Variant(data) if data.name.as_ref() == "Ok" => data
            .fields
            .get("value")
            .cloned()
            .map(Ok)
            .ok_or_else(|| EvalError::Runtime("Ok value is missing.".to_string())),
        VmValue::Variant(data) if data.name.as_ref() == "Err" => data
            .fields
            .get("value")
            .cloned()
            .map(Err)
            .ok_or_else(|| EvalError::Runtime("Err value is missing.".to_string())),
        other => Err(EvalError::Runtime(format!(
            "? expects a Result value, got `{}`.",
            other.display()
        ))),
    }
}

fn value_some(value: VmValue) -> VmValue {
    VmValue::OptionSome(Box::new(value))
}

fn value_none() -> VmValue {
    VmValue::OptionNone
}

fn value_err(value: VmValue) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("value".to_string(), value);
    VmValue::Variant(Rc::new(VmStruct {
        name: Rc::from("Err"),
        fields,
    }))
}

fn value_ok(value: VmValue) -> VmValue {
    let mut fields = HashMap::new();
    fields.insert("value".to_string(), value);
    VmValue::Variant(Rc::new(VmStruct {
        name: Rc::from("Ok"),
        fields,
    }))
}

fn expect_string_ref(value: &VmValue) -> Result<&str, EvalError> {
    match value {
        VmValue::String(value) => Ok(value.as_str()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected String, got `{}`.",
            other.display()
        ))),
    }
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

fn string_slice_range(value: &str, start: i64, len: i64) -> &str {
    let byte_start = clamp_to_char_boundary(value, start.max(0) as usize);
    let requested_end = byte_start.saturating_add(len.max(0) as usize);
    let byte_end = clamp_to_char_boundary(value, requested_end.min(value.len()));
    &value[byte_start..byte_end]
}

fn string_pad(value: &str, width: i64, fill: &str, left: bool) -> String {
    let target = width.max(0) as usize;
    if value.len() >= target || fill.is_empty() {
        return value.to_string();
    }
    let missing = target - value.len();
    let mut padding = String::new();
    while padding.len() < missing {
        padding.push_str(fill);
    }
    while padding.len() > missing {
        padding.pop();
    }
    if left {
        format!("{padding}{value}")
    } else {
        format!("{value}{padding}")
    }
}

fn string_format(template: &str, args: &[String]) -> String {
    let mut output = String::new();
    let mut chars = template.chars().peekable();
    let mut arg_index = 0;
    while let Some(ch) = chars.next() {
        match (ch, chars.peek().copied()) {
            ('{', Some('{')) => {
                chars.next();
                output.push('{');
            }
            ('}', Some('}')) => {
                chars.next();
                output.push('}');
            }
            ('{', Some('}')) => {
                chars.next();
                if let Some(value) = args.get(arg_index) {
                    output.push_str(value);
                    arg_index += 1;
                } else {
                    output.push_str("{}");
                }
            }
            _ => output.push(ch),
        }
    }
    output
}

fn clamp_to_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn type_root_name(name: &str) -> &str {
    name.split_once('<')
        .map(|(root, _)| root.trim())
        .unwrap_or(name)
}

fn type_arg_names(type_name: &str) -> Option<Vec<&str>> {
    let (_, rest) = type_name.split_once('<')?;
    let args = rest.strip_suffix('>')?;
    Some(split_type_args(args))
}

fn split_type_args(args: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                result.push(args[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let last = args[start..].trim();
    if !last.is_empty() {
        result.push(last);
    }
    result
}
