use std::cell::RefCell;
use std::cmp::Ordering;
#[cfg(feature = "native-jit")]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use rsscript_abi_model::{
    FunctionSignature, WireCallTypeTable, WireRecordFieldLayout, WireRecordLayout, WireType,
    WireValue, WireVariantCaseLayout, WireVariantLayout,
};
use rsscript_corelib::{
    collections::{
        dedup as core_list_dedup, deque_to_vec as core_deque_to_vec,
        enumerate as core_list_enumerate, map_difference as core_map_difference,
        map_intersection as core_map_intersection, map_is_subset as core_map_is_subset,
        map_keys as core_map_keys, map_union as core_map_union, map_values as core_map_values,
        reverse as core_list_reverse, skip as core_list_skip, slice as core_list_slice,
        take as core_list_take, zip as core_list_zip,
    },
    compression::gzip_decompress as core_gzip_decompress,
    crypto::{
        hmac_sha256_hex as core_hmac_sha256_hex, sha3_224 as core_sha3_224,
        sha3_256 as core_sha3_256, sha256_hex as core_sha256_hex, shake128 as core_shake128,
    },
    date::{
        add_days as core_date_add_days, add_ms as core_date_add_ms, day as core_date_day,
        days_between as core_date_days_between, days_in_month as core_date_days_in_month,
        format_iso as core_date_format_iso, format_ymd as core_date_format_ymd,
        hour as core_date_hour, is_leap_year as core_date_is_leap_year, minute as core_date_minute,
        month as core_date_month, parse_iso as core_date_parse_iso,
        parse_ymd as core_date_parse_ymd, second as core_date_second,
        start_of_day as core_date_start_of_day, weekday as core_date_weekday,
        year as core_date_year,
    },
    encoding::{
        base64_decode, base64_encode, hex_decode as core_hex_decode, hex_encode as core_hex_encode,
        url_decode_component, url_encode_component,
    },
    regex::CompiledRegex,
    structured_data::yaml_to_json as core_yaml_to_json,
};

use self::calls::PureClosurePlan;
use crate::eval_types::{
    AsyncProviderCallContext, EvalError, EvalExecutionReport, EvalOutput, ExternalFunction,
    ProviderCallContext, ProviderCallMode, ProviderError, ProviderResourceRegistry,
    WireMutationProviderFuture, WireMutationResult, WireProviderFuture,
};
#[cfg(feature = "native-jit")]
use crate::text_util::string_pad_len;
use crate::text_util::{
    string_format, string_pad, string_slice_range, type_arg_names, type_root_name,
};
#[cfg(feature = "native-jit")]
use crate::vm_value::clone_value_map_preserving_capacity;
use crate::vm_value::{
    TypeLayout, TypedVec, ValueMap, VmClosure, VmMapKey, VmNative, VmStruct, VmValue,
};
use rsscript_abi_model::BinaryOp;

mod bytecode;
mod calls;
mod exec;
mod exec_ops;
mod executable;
// jit_host_boundary! must be in textual scope before the child modules that use
// it (jit_native_a/b, native_text_helpers) — a macro_rules is not visible to
// sibling modules otherwise.
#[cfg(feature = "native-jit")]
// Generated code crosses an `extern "C"` boundary to reach VM helpers. Rust
// unwinding must never cross that boundary: a Provider implementation, borrow
// check, or internal assertion that panics is converted into the ordinary JIT
// bail path, where the VM aborts the transaction and resumes in the interpreter.
#[cfg(feature = "native-jit")]
macro_rules! jit_host_boundary {
    (
        extern "C" fn $name:ident(
            $ctx:ident: vm_jit::HostCtx
            $(, $arg:ident: $arg_ty:ty)* $(,)?
        ) -> $ret:ty $body:block
    ) => {
        pub(super) extern "C" fn $name(
            $ctx: vm_jit::HostCtx,
            $($arg: $arg_ty),*
        ) -> $ret {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> $ret { $body })) {
                Ok(value) => value,
                Err(_) => {
                    vm_jit::signal_bail($ctx);
                    <$ret as Default>::default()
                }
            }
        }
    };
}

mod jit_ctx_impl;
mod jit_native_a;
#[cfg(feature = "native-jit")]
use jit_native_a::*;
mod jit_native_b;
#[cfg(feature = "native-jit")]
use jit_native_b::*;
#[cfg(feature = "native-jit")]
mod native_text_helpers;
#[cfg(feature = "native-jit")]
use native_text_helpers::*;
mod native_stats_impl;
#[cfg(feature = "native-jit")]
use native_stats_impl::*;
mod execution_plan;
#[cfg(feature = "native-jit")]
mod intrinsic_metadata;
mod intrinsics;
mod model;
#[cfg(feature = "native-jit")]
mod native;
mod planning;
mod resource_io;
mod resources;
mod runtime_resources;
mod runtime_values;
mod scheduler;
mod state;
mod tier;
mod value_access;
mod value_convert;
mod value_ops;
#[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
use execution_plan::NativeDiagnosticOptions;
#[cfg(feature = "native-jit")]
use execution_plan::NativeExecutionPlan;
use execution_plan::{ExecutionPlan, StdoutMode, TierPlan};
#[cfg(feature = "native-jit")]
pub use execution_plan::{NativeCostModel, NativeJitOptions};
#[cfg(feature = "native-jit")]
use intrinsic_metadata::*;
pub(crate) use model::*;
#[cfg(feature = "native-jit")]
use native::*;
pub use planning::JitPlan;
use planning::*;
use resources::*;
use runtime_resources::*;
use runtime_values::*;
#[cfg(feature = "native-jit")]
use state::DEFAULT_MAX_DEPTH;
pub use state::VmLimits;
use state::{CANCEL_POLL_INTERVAL, Frame, MAP_ENTRY_BYTES, MAX_INTRINSIC_OUTPUT_BYTES};
use tier::JitState;
use value_access::*;
use value_convert::*;
use value_ops::*;

/// Outcome of executing a single "pure" instruction via the shared
/// [`RegVm::try_exec_pure`] dispatcher. Pure instructions push no frames, never
/// suspend, and never call other functions, so both the interpreter (`drive`)
/// and the tier-0 JIT executor (`run_jit`) share one copy of their semantics —
/// gap-freeness is then structural, not just differential-checked.
enum PureStep {
    /// Executed; advance to the next instruction (`ip` already updated for jumps).
    Next,
    /// A `Return` instruction; the caller decides how to unwind (the JIT returns
    /// the value directly, the interpreter pops the frame).
    Return(VmValue),
    /// Not in the pure subset; the caller must handle it (frames, calls, async…).
    NotPure,
}

#[derive(Debug, Clone)]
pub struct RegVmExecutable {
    unit: Rc<RegUnit>,
    artifact: rsscript_bytecode::BytecodeArtifact,
    /// Static facts admitted together with the executable by the bytecode
    /// verifier. Engines consume this wrapper rather than reparsing raw bytes.
    typed_executable_facts: Option<Rc<rsscript_bytecode::BoundTypedExecutableFactsV1>>,
    /// Lazily derived once per verified executable. Interpreter-only execution
    /// pays no facts cost even in a binary that was built with native JIT.
    #[cfg(feature = "native-jit")]
    verified_facts: std::cell::OnceCell<Result<Rc<VerifiedExecutableFacts>, VerifiedFactsError>>,
    /// Immutable loop evidence is derived at most once from the verified code
    /// and reused by every telemetry-enabled execution of this executable.
    #[cfg(feature = "native-jit")]
    loop_optimization_evidence: std::cell::OnceCell<LoopOptimizationEvidence>,
}

impl RegVmExecutable {
    /// Decode a bytecode envelope already accepted by `BytecodeVerifier`.
    /// Callers cannot manufacture `VerifiedBytecode`, so public construction
    /// keeps the generic Artifact verification phase explicit.
    pub fn from_verified_bytecode(
        verified: rsscript_bytecode::VerifiedBytecode,
    ) -> Result<Self, EvalError> {
        let (artifact, unit, typed_executable_facts) = bytecode::decode_verified_bytecode(
            verified,
            rsscript_bytecode::VerificationContext::default(),
        )?
        .into_parts();
        Ok(Self {
            unit: Rc::new(unit),
            artifact,
            typed_executable_facts: typed_executable_facts.map(Rc::new),
            #[cfg(feature = "native-jit")]
            verified_facts: std::cell::OnceCell::new(),
            #[cfg(feature = "native-jit")]
            loop_optimization_evidence: std::cell::OnceCell::new(),
        })
    }

    /// Bind this verified executable to its immutable workspace input.
    pub fn bind_snapshot_digest(&mut self, digest: impl Into<String>) -> Result<(), EvalError> {
        self.artifact
            .bind_snapshot_digest(digest)
            .map_err(|error| EvalError::Runtime(error.to_string()))
    }
}

/// Result of driving a task's call stack one slice at a time.
enum Outcome {
    /// The frame at `floor` returned this value (the task or sync call finished).
    Completed(VmValue),
    /// A blocking op parked the task; details are in `RegVm::suspension`.
    Suspended,
}

type TaskId = usize;

/// What a parked task is waiting for. When the condition is met the scheduler
/// produces the operation's result, writes it into `Suspension::resume_dst`, and
/// re-queues the task (the "completion" model — the parked instruction is not
/// re-executed; the saved `ip` already points past it).
enum Wait {
    /// `Receiver.recv` on an empty-but-open channel: ready when a value is queued
    /// or the channel closes.
    Recv { channel: i64 },
    /// `Sender.send` on a full bounded channel: ready when capacity frees up. The
    /// sender + value are carried so the send can be retried on wake.
    Send { sender: VmSender, value: VmValue },
    /// `await`-ing a spawned task / `async let`: ready when that task finishes.
    Join { task: TaskId },
    /// Structured task-group drain: ready only after every still-live child
    /// finishes. Completed children are reaped when the parent resumes.
    JoinAll { tasks: Vec<TaskId> },
    /// `select { ... }`: ready as soon as any arm task in `handles` finishes. The
    /// winning arm index and its value are written to `winner_dst`/`value_dst`.
    Select {
        handles: Vec<TaskId>,
        winner_dst: usize,
        value_dst: usize,
    },
    /// A descriptor-linked asynchronous Provider call whose callable receives
    /// canonical wire values. The register VM only adapts the completed value
    /// at its legacy register boundary.
    WireProvider {
        future: WireProviderFuture,
        result: Option<Result<WireValue, ProviderError>>,
        key: String,
        mutation_targets: Vec<usize>,
    },
    /// An asynchronous canonical wire Provider call with explicit mutation
    /// write-backs. The scheduler never stores a dynamic mutation envelope.
    WireMutationProvider {
        future: WireMutationProviderFuture,
        result: Option<Result<WireMutationResult, ProviderError>>,
        key: String,
        mutation_targets: Vec<usize>,
    },
}

struct Suspension {
    wait: Wait,
    /// Absolute register that receives the operation's result on wake.
    resume_dst: usize,
}

/// A parked task's full execution state, swapped out of `RegVm` while another
/// task runs and swapped back in on resume.
struct SavedTask {
    frames: Vec<Frame>,
    stack: Vec<VmValue>,
    written: Vec<bool>,
}

struct TaskSlot {
    /// Parked execution state; `None` while the task is the one swapped into
    /// `RegVm` and running.
    saved: Option<SavedTask>,
    /// `Some(value)` once the task has returned (value available to joiners).
    done: Option<VmValue>,
    /// `Some(wait)` while the task is parked on a blocking op.
    wait: Option<Wait>,
    /// Register (in the task's own stack) that receives the op result on wake.
    resume_dst: usize,
}

struct RegVm {
    unit: Rc<RegUnit>,
    /// Evaluation-local experimental JIT state. The decoded verified program
    /// stays immutable; feedback is keyed by its executable digest and function
    /// ordinal inside this side table.
    jit_state: JitState,
    entry_args: Vec<String>,
    external_bindings: HashMap<String, ExternalFunction>,
    stdout: String,
    /// When set, complete lines appended to `stdout` are also written live to the
    /// real process stdout (line-flushed). `stream_flushed` tracks how many bytes
    /// of `stdout` have been streamed so a
    /// partial trailing line is not emitted twice. The captured `stdout` String is
    /// built identically whether or not streaming is on, so every other caller
    /// (and the parity/differential tests) is unaffected.
    stream_stdout: bool,
    stream_flushed: usize,
    stderr: String,
    stack: Vec<VmValue>,
    written: Vec<bool>,
    frames: Vec<Frame>,
    /// Set by a blocking op during `drive`; consumed by the scheduler.
    suspension: Option<Suspension>,
    /// Cooperative single-threaded task table + ready queue.
    tasks: HashMap<TaskId, TaskSlot>,
    ready_queue: VecDeque<TaskId>,
    next_task_id: TaskId,
    current_task: TaskId,
    next_cancellation_id: i64,
    cancellation_flags: HashMap<i64, bool>,
    next_channel_id: i64,
    channels: HashMap<i64, VmChannel>,
    /// Tier-0 JIT: when set, JIT-eligible functions run via the specializing
    /// executor `run_jit` (which reuses the interpreter's value/register
    /// semantics, so it is gap-free by construction).
    jit_enabled: bool,
    /// JIT every supported function, ignoring the has-loop heuristic (used by the
    /// differential tests so the whole covered instruction subset is verified).
    jit_force_all: bool,
    /// Resource limits (recursion depth / step budget / memory ceiling).
    limits: VmLimits,
    /// Instructions executed so far in this run (the step budget's fuel gauge).
    /// Only consulted when `limits.step_budget` is `Some`; the unconditional
    /// increment is the entire overhead when the budget is off.
    steps: u64,
    /// Best-effort cumulative count of VM-owned allocation and capacity growth.
    /// We add estimated growth and do not subtract frees, so this is an
    /// allocation quota rather than a precise live set. It
    /// exists only to trip `limits.allocation_budget`; when that is `None` we skip all
    /// accounting so the overhead is zero. Accounted sites include register-stack
    /// growth, collection construction and capacity growth, and bounded intrinsic
    /// outputs such as SHAKE digests.
    allocated_bytes: usize,
    /// Current and peak reachable RSScript value storage. These are refreshed
    /// at instruction boundaries when the limit is armed and once before an
    /// execution report is built.
    live_memory_bytes: usize,
    peak_live_memory_bytes: usize,
    /// Set by VM-owned allocation/capacity-growth sites. The next instruction
    /// boundary performs one root-set walk; pure scalar instructions pay only
    /// this branch instead of rescanning the heap.
    live_memory_dirty: bool,
    /// Number of stdlib/runtime intrinsic calls dispatched so far (the
    /// `intrinsic_call_budget` fuel gauge). Only consulted when that budget is `Some`;
    /// the unconditional increment is the entire overhead when it is off.
    intrinsic_calls: u64,
    /// Number of calls dispatched through explicitly linked Provider symbols.
    provider_calls: u64,
    /// Structured-concurrency lifecycle counters for execution reports.
    tasks_created: u64,
    tasks_completed: u64,
    tasks_cancelled: u64,
    tasks_live: usize,
    tasks_peak_live: usize,
    /// Lexical resource scopes owned by each scheduler task. Entries retain a
    /// clone of the acquired value so cancellation can finalize a parked task
    /// after its register window has been moved out of the active VM stack.
    resource_scopes: HashMap<TaskId, Vec<TrackedResource>>,
    /// Structured trace of calls crossing the Provider boundary.
    provider_trace: std::sync::Arc<crate::eval_types::ProviderTraceCollector>,
    /// VM-owned, generation-checked Provider resource slots.
    provider_resources: ProviderResourceRegistry,
    /// Native (Cranelift) JIT state, `Some` when the native tier is enabled. The
    /// native tier compiles the integer/control core to machine code and is tried
    /// before the tier-0 executor; anything it can't compile (or bails on) falls
    /// back to tier-0 / the interpreter.
    #[cfg(feature = "native-jit")]
    native: Option<NativeState>,
    /// Cache of canonical non-capturing closures, indexed by function id. A
    /// `MakeClosure` with no captures builds `VmClosure { function, captures: [] }`
    /// — a value that is *identical* for a given function on every execution — so
    /// after the first allocation we hand out clones of the same `Rc` (a refcount
    /// bump) instead of allocating a fresh one each loop iteration.
    ///
    /// SOUNDNESS: sharing one `Rc` makes previously-distinct allocations compare
    /// equal under `Rc::ptr_eq`, which is observable ONLY through `==`/`!=` on a
    /// closure (closures are not `Hashable`, so never `Map`/`Set` keys). The cache
    /// is therefore populated only when `unit.closure_identity_observable` is
    /// `false`, i.e. the whole program provably never compares a closure-bearing
    /// value. When it is `true` the cache stays empty and every `MakeClosure`
    /// allocates fresh, matching the compiled backend bit-for-bit. `VmClosure` is
    /// immutable after construction (its `captures` Vec is never mutated in
    /// place — verified by grep), so a shared `Rc` can never diverge.
    noncapturing_closure_cache: Vec<Option<Rc<VmClosure>>>,
    /// Compiled plans for captureless pure closures, keyed by `(function, arity)`.
    /// Stores negative results too so repeated `List.map/filter/fold` calls do
    /// not re-walk unsupported closure bytecode. Captured closures are excluded
    /// because their behavior depends on per-allocation captures.
    pure_closure_plan_cache: HashMap<(usize, usize), Option<PureClosurePlan>>,
}

#[derive(Clone)]
struct TrackedResource {
    register: usize,
    value: VmValue,
}

/// Outcome of a [`RegVm::try_native`] attempt.
///
/// `Completed` carries the native result (the caller finishes the frame exactly
/// like the `Return` arm). `Resumed` means a native bail was reconstructed into
/// the interpreter at the safepoint's `resume_ip` (precise deopt, only under the
/// `precise_deopt` flag): the live register window has been restored and the
/// frame's `ip` advanced, so the caller just re-enters the interpreter loop.
/// `Fallback` means native did not produce a value (ineligible, arg mismatch, or
/// a bail that precise resume didn't apply): the frame `ip` is still `0`, so the
/// caller re-runs the function from the top on the interpreter — the safe,
/// behavior-preserving default.
#[cfg(feature = "native-jit")]
enum NativeAttempt {
    Completed(VmValue),
    Resumed,
    Fallback,
}

#[cfg(feature = "native-jit")]
type NativeCompiledEntry = (
    vm_jit::CompiledId,
    NativeTy,
    Vec<NativeTy>,
    bool,
    bool,
    Vec<Rc<String>>,
    bool,
);

#[cfg(feature = "native-jit")]
const MAX_NATIVE_SHAPE_VERSIONS: usize = 2;

/// Maximum verifier-proved generic instances admitted for one source function
/// during an evaluation. Static type arguments and runtime representation
/// shapes are separate dimensions: this cap bounds monomorphization/code size,
/// while [`MAX_NATIVE_SHAPE_VERSIONS`] bounds the few genuinely dynamic layouts
/// within each admitted instance.
#[cfg(feature = "native-jit")]
const MAX_JIT_INSTANCES_PER_FUNCTION: usize = 8;
#[cfg(feature = "native-jit")]
const MAX_JIT_TYPE_ARGUMENTS: usize = 16;

/// Concrete substitutions admitted by the typed-facts verifier.
///
/// v1 lowering does not yet retain ordinary direct-call substitutions. Empty
/// input therefore means `Unavailable`, not "proved nongeneric". This explicit
/// state is the fail-closed bridge: native caching can consume real type
/// arguments as soon as lowering supplies them without guessing from function
/// names, register values, or source spellings.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum VerifiedTypeArgsKey {
    Unavailable,
    Known {
        arguments: Box<[WireType]>,
        /// Concrete parameter/result storage verified at the call site. This
        /// prevents reordered substitutions from reusing incompatible code.
        storage_signature: Box<[VerifiedStorageType]>,
    },
}

#[cfg(feature = "native-jit")]
impl VerifiedTypeArgsKey {
    fn from_verified(arguments: &[WireType]) -> Option<Self> {
        if arguments.len() > MAX_JIT_TYPE_ARGUMENTS {
            return None;
        }
        Some(if arguments.is_empty() {
            Self::Unavailable
        } else {
            Self::Known {
                arguments: arguments.to_vec().into_boxed_slice(),
                storage_signature: Box::new([]),
            }
        })
    }

    const fn is_known(&self) -> bool {
        matches!(self, Self::Known { .. })
    }
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct JitInstanceKey {
    function: usize,
    type_arguments: VerifiedTypeArgsKey,
}

#[cfg(feature = "native-jit")]
impl JitInstanceKey {
    fn from_facts(function: usize, facts: &VerifiedFunctionFacts) -> Option<Self> {
        Self::from_type_arguments(function, &facts.generic_substitutions)
    }

    /// Build a bounded cache identity from lowering-attested substitutions.
    /// This key must never authorize a storage class, layout projection, or
    /// unsafe lowering; those remain gated by executable-cross-checked facts.
    fn from_type_arguments(function: usize, type_arguments: &[WireType]) -> Option<Self> {
        Some(Self {
            function,
            type_arguments: VerifiedTypeArgsKey::from_verified(type_arguments)?,
        })
    }

    fn from_call_site(function: usize, call: &VerifiedCallSite) -> Option<Self> {
        if call.type_arguments.len() > MAX_JIT_TYPE_ARGUMENTS {
            return None;
        }
        let mut storage = call.params.to_vec();
        storage.push(call.result);
        Some(Self {
            function,
            type_arguments: if call.type_arguments.is_empty() {
                VerifiedTypeArgsKey::Unavailable
            } else {
                VerifiedTypeArgsKey::Known {
                    arguments: call.type_arguments.clone(),
                    storage_signature: storage.into_boxed_slice(),
                }
            },
        })
    }
}

/// Stable runtime ABI shape used for bounded native multiversioning. Scalar
/// payloads and collection lengths are deliberately absent. Heap identities are
/// included only where the runtime already exposes stable dispatch metadata:
/// closure ABI class and interned struct/variant layouts. Closure function ids
/// stay in the existing mono/PIC feedback so a three-arm PIC does not consume
/// three whole-function shape versions.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum NativeParamShape {
    /// The verified executable already proves the exact scalar storage class.
    /// The function/region key identifies that static fact, so the runtime value
    /// must not create a second specialization dimension.
    StaticScalar,
    Int,
    Bool,
    Float,
    FlatInt,
    FlatFloat,
    Handle,
    Closure,
    Struct(*const TypeLayout),
    Variant(*const TypeLayout),
    Unsupported,
}

#[cfg(feature = "native-jit")]
const INLINE_SHAPE_PARAMS: usize = 8;

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ShapeKey {
    Inline {
        len: u8,
        values: [NativeParamShape; INLINE_SHAPE_PARAMS],
    },
    Heap(Box<[NativeParamShape]>),
}

#[cfg(feature = "native-jit")]
impl Default for ShapeKey {
    fn default() -> Self {
        Self::Inline {
            len: 0,
            values: [NativeParamShape::Unsupported; INLINE_SHAPE_PARAMS],
        }
    }
}

#[cfg(feature = "native-jit")]
impl ShapeKey {
    fn from_shapes(values: impl IntoIterator<Item = NativeParamShape>) -> Self {
        let mut inline = [NativeParamShape::Unsupported; INLINE_SHAPE_PARAMS];
        let mut len = 0usize;
        let mut overflow: Option<Vec<NativeParamShape>> = None;
        for shape in values {
            if let Some(values) = overflow.as_mut() {
                values.push(shape);
            } else if len < INLINE_SHAPE_PARAMS {
                inline[len] = shape;
                len += 1;
            } else {
                let mut values = inline.to_vec();
                values.push(shape);
                overflow = Some(values);
            }
        }
        if let Some(values) = overflow {
            Self::Heap(values.into_boxed_slice())
        } else {
            Self::Inline {
                len: len as u8,
                values: inline,
            }
        }
    }
}

#[cfg(feature = "native-jit")]
fn native_param_shape(value: &VmValue) -> NativeParamShape {
    match value {
        VmValue::Int(_) => NativeParamShape::Int,
        VmValue::Bool(_) => NativeParamShape::Bool,
        VmValue::Float(_) => NativeParamShape::Float,
        VmValue::List(values) => match values.try_borrow().ok().as_deref() {
            Some(TypedVec::Ints(_)) => NativeParamShape::FlatInt,
            Some(TypedVec::Floats(_)) => NativeParamShape::FlatFloat,
            _ => NativeParamShape::Handle,
        },
        VmValue::Closure(_) => NativeParamShape::Closure,
        VmValue::Struct(value) => NativeParamShape::Struct(Rc::as_ptr(&value.layout)),
        VmValue::Variant(value) => NativeParamShape::Variant(Rc::as_ptr(&value.layout)),
        VmValue::Managed(inner) => inner
            .try_borrow()
            .ok()
            .map_or(NativeParamShape::Handle, |value| native_param_shape(&value)),
        VmValue::String(_)
        | VmValue::Bytes(_)
        | VmValue::Json(_)
        | VmValue::Deque(_)
        | VmValue::Map(_)
        | VmValue::Native(_)
        | VmValue::OptionSomeHeap(_) => NativeParamShape::Handle,
        VmValue::Unit | VmValue::Char(_) | VmValue::OptionNone | VmValue::OptionSomeScalar(_) => {
            NativeParamShape::Unsupported
        }
    }
}

#[cfg(feature = "native-jit")]
fn native_param_shape_with_fact(value: &VmValue, fact: VerifiedStorageType) -> NativeParamShape {
    match fact {
        VerifiedStorageType::Int | VerifiedStorageType::Bool | VerifiedStorageType::Float => {
            NativeParamShape::StaticScalar
        }
        // Handles still need representation/layout specialization: a verified
        // v1 handle does not distinguish boxed, flat-list, closure, or nominal
        // aggregate representations.
        VerifiedStorageType::Handle | VerifiedStorageType::Unknown => native_param_shape(value),
        VerifiedStorageType::Unit | VerifiedStorageType::Char => NativeParamShape::Unsupported,
    }
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NativeVersionKey {
    instance: JitInstanceKey,
    shape: ShapeKey,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OsrVersionKey {
    region: RegionKey,
    type_arguments: VerifiedTypeArgsKey,
    shape: ShapeKey,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ContinuationVersionKey {
    instance: JitInstanceKey,
    entry: usize,
    shape: ShapeKey,
    cancel_armed: bool,
}

/// State for the native JIT tier: the Cranelift modules owning compiled code,
/// bounded per-function/per-region shape caches, and tiering/deopt knobs.
#[cfg(feature = "native-jit")]
struct LazyNativeModule {
    helpers: vm_jit::HostHelpers,
    baseline: bool,
    budget: vm_jit::ExecutableMemoryBudget,
    arena_bytes: u64,
    module: Option<vm_jit::NativeModule>,
    initialization_error: Option<String>,
}

#[cfg(feature = "native-jit")]
impl LazyNativeModule {
    fn new(
        helpers: vm_jit::HostHelpers,
        baseline: bool,
        budget: vm_jit::ExecutableMemoryBudget,
        arena_bytes: u64,
    ) -> Self {
        Self {
            helpers,
            baseline,
            budget,
            arena_bytes,
            module: None,
            initialization_error: None,
        }
    }

    /// Allocate executable memory and build the host ISA only when the first
    /// profitable region is actually admitted. A native-enabled execution that
    /// stays on the interpreter therefore reserves no executable arena and pays
    /// no Cranelift initialization cost.
    fn ensure_initialized(&mut self) -> bool {
        if self.module.is_some() {
            return true;
        }
        if self.initialization_error.is_some() {
            return false;
        }
        match vm_jit::NativeModule::new_with_opt_and_memory_budget(
            self.helpers,
            self.baseline,
            self.budget.clone(),
            self.arena_bytes,
        ) {
            Ok(module) => {
                self.module = Some(module);
                true
            }
            Err(error) => {
                self.initialization_error = Some(error.to_string());
                false
            }
        }
    }

    fn compile_phase_timings(&self) -> vm_jit::CompilePhaseTimings {
        self.module
            .as_ref()
            .map(vm_jit::NativeModule::compile_phase_timings)
            .unwrap_or_default()
    }
}

#[cfg(feature = "native-jit")]
impl std::ops::Deref for LazyNativeModule {
    type Target = vm_jit::NativeModule;

    fn deref(&self) -> &Self::Target {
        self.module
            .as_ref()
            .expect("native module must be initialized before use")
    }
}

#[cfg(feature = "native-jit")]
impl std::ops::DerefMut for LazyNativeModule {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.module
            .as_mut()
            .expect("native module must be initialized before use")
    }
}

#[cfg(feature = "native-jit")]
struct NativeState {
    /// Bounded, evaluation-local storage/call/effect facts derived from the
    /// already verified v1 executable. Test-only constructors may leave this
    /// empty; production native execution installs it before dispatch.
    verified_facts: Option<Rc<VerifiedExecutableFacts>>,
    baseline_module: LazyNativeModule,
    /// The speed-optimized tier. `None` is the explicit
    /// baseline-only diagnostic mode.
    optimized_module: Option<LazyNativeModule>,
    /// Shared hard executable-memory boundary for every tier in this run.
    executable_memory_budget: vm_jit::ExecutableMemoryBudget,
    /// Process-local JIT work admission. This bounds code made available for
    /// dispatch across both modules. The provider-level budget above is the hard
    /// allocation boundary; this counter remains useful for telemetry.
    admission: NativeAdmissionBudget,
    // `None` = known not native-eligible; `Some((id, ret, params, has_backedge, scalar_leaf_callable, literals, precise_resume_safe))`
    // = compiled handle, return type (to box the 64-bit result), parameter types
    // (to unbox each argument: `Int`/`Bool` from their VM value, `Float` as bits),
    // and whether the function's body contains an internal back-edge (a loop). The
    // back-edge bit drives the no-amortization profitability gate
    // (`NATIVE_NOAMORTIZE_GIVEUP`): a loop-free body dispatched per loop iteration
    // can never amortize FFI cost, so that version is disabled after `K` dispatches.
    cache: HashMap<NativeVersionKey, Option<NativeCompiledEntry>>,
    /// Module-local optimized handles. IDs from this cache are dispatched only
    /// through `optimized_module`.
    optimized_cache: HashMap<NativeVersionKey, NativeCompiledEntry>,
    /// Recompile sources retained only for baseline whole functions that contain
    /// no native-to-native call edges.
    optimization_sources: HashMap<NativeVersionKey, vm_jit::JitFunction>,
    /// Per-function deterministic interpreted-work counts for baseline tiering.
    /// `0` means "compile on first call" (force-all).
    counts: HashMap<usize, u64>,
    /// Shared hotness/promotion/demotion lifecycle for whole-function versions.
    whole_controllers: HashMap<NativeVersionKey, RegionController>,
    optimize_work_threshold: u64,
    /// Per-version *consecutive* runtime-bail counts, keyed like `cache`.
    /// Incremented on every bail after native was chosen (arg mismatch or runtime
    /// guard), reset to 0 on a successful native completion. At
    /// `NATIVE_BAIL_GIVEUP_THRESHOLD` only that shape is negative-cached, so one
    /// failing shape cannot demote successful versions.
    /// Per-version count of native dispatches of a back-edge-free body. At
    /// `NATIVE_NOAMORTIZE_GIVEUP` only that shape is negative-cached. Loop-bearing
    /// bodies are never inserted, so they are never disabled by this counter.
    noamortize_counts: HashMap<NativeVersionKey, u32>,
    tier_up_threshold: u32,
    /// Deopt stress mode: when set, the native tier always bails, so every
    /// native-eligible function exercises the fallback path. Used to verify
    /// `{interp, tier0, native, force-deopt, compiled}` all agree.
    force_bail: bool,
    /// Deopt stress mode for a real native safepoint. When set, the translator
    /// compiles each native function with that safepoint id forced to bail,
    /// exercising the generated deopt payload and resume map instead of rejecting
    /// native execution before entry.
    forced_safepoint: Option<u32>,
    /// Explicit deopt stress mode: when set, every
    /// generated native safepoint bails unconditionally.
    force_all_safepoints: bool,
    /// Host-selected profitability behavior; never inferred from process state.
    cost_model: NativeCostModel,
    /// Interpreted work required before automatic OSR compilation.
    osr_work_threshold: u32,
    /// Telemetry: where native-tier attempts go (so the next coverage win is
    /// measurable rather than guessed).
    stats: NativeStats,
    /// Whether to collect telemetry. Keep timing and counter updates out of the
    /// native-call hot path unless a caller explicitly asks for them.
    collect_stats: bool,
    /// precise deopt: when set, a native bail at a known safepoint
    /// reconstructs the interpreter register window from the captured live values
    /// and resumes interpretation AT the safepoint's `resume_ip`, instead of
    /// re-running the function from the top. Default `false` ⇒ byte-identical
    /// re-run-from-top (the safe baseline). Selected by the execution plan.
    precise_deopt: bool,
    /// Enables threshold-driven OSR for qualifying native-subset loops.
    auto_osr_enabled: bool,
    /// Forces the first candidate-loop header to attempt OSR. Reserved for
    /// deterministic differential and diagnostic entry points.
    eager_osr: bool,
    /// Deterministically ranked OSR candidates per function. Each fixed-size value
    /// contains at most [`MAX_OSR_REGIONS_PER_FUNCTION`] headers, so a function's
    /// interpreter overhead and native compile exposure stay bounded.
    osr_candidates: HashMap<usize, OsrCandidates>,
    /// Evaluation-local hot-backedge state, independent for each candidate region.
    /// A stable decline at one header cannot disable another header in the function.
    osr_triggers: HashMap<RegionKey, OsrTrigger>,
    /// OSR compile cache keyed by function, original loop header, and runtime
    /// shape. `Some(entry)` is compiled; `None` is a stable per-version decline.
    osr_cache: OsrCache,
    /// Optimized OSR entries and their bounded baseline recompile sources.
    optimized_osr_cache: HashMap<OsrVersionKey, OsrEntry>,
    osr_optimization_sources: HashMap<OsrVersionKey, OsrOptimizationSource>,
    /// Shared hotness/promotion/demotion lifecycle for OSR versions.
    osr_controllers: HashMap<OsrVersionKey, RegionController>,
    /// Straight-line scalar continuation regions, keyed by exact VM entry IP and
    /// runtime register shape. `None` is a stable decline for that version.
    continuation_cache: HashMap<ContinuationVersionKey, Option<Rc<ContinuationEntry>>>,
    continuation_controllers: HashMap<ContinuationVersionKey, RegionController>,
    /// Structural CFG plans (including negative results) are shape-independent and
    /// cached separately so probing each interpreter IP stays an O(1) lookup.
    continuation_plans: HashMap<(usize, usize), Option<Rc<ContinuationRegion>>>,
    continuation_entry_sets: HashMap<usize, Rc<ContinuationEntrySet>>,
    continuation_functions: HashMap<usize, bool>,
    /// Reusable per-call marshalling scratch buffers (TV2 arg/len words and the
    /// flat-list `Rc` keep-alive set). Held here and `mem::take`n into the call
    /// frame so a hot per-iteration native dispatch (e.g. a tiny leaf/closure
    /// called once per loop iteration) does not heap-allocate three `Vec`s on
    /// every call — that per-call allocation churn, not the native body, is what
    /// made marginal closure/leaf kernels slower than the interpreter.
    scratch_args: Vec<i64>,
    scratch_lens: Vec<i64>,
    scratch_flat_owned: Vec<Rc<RefCell<TypedVec>>>,
    scratch_flat_mut_owned: Vec<Rc<RefCell<TypedVec>>>,
    scratch_heap_input_slots: Vec<(usize, usize)>,
    scratch_osr_window: Vec<i64>,
    scratch_osr_lens: Vec<i64>,
    scratch_osr_flat_owned: Vec<Rc<RefCell<TypedVec>>>,
    scratch_osr_flat_mut_owned: Vec<Rc<RefCell<TypedVec>>>,
    scratch_osr_flat_slots: Vec<(usize, NativeTy)>,
    scratch_osr_flat_mut_slots: Vec<(usize, usize)>,
    scratch_osr_heap_input_slots: Vec<(usize, usize)>,
    /// Per-evaluation call scratch. Machine code and immutable deopt metadata live
    /// in `NativeModule`; activation payloads are reused here and never stored on
    /// the executable-code owner.
    call_session: vm_jit::NativeCallSession,
    /// Missed-optimization report armed by an explicit diagnostic plan. Read once
    /// at construction (mirrors `collect_stats`), so the hot path pays only
    /// a single hoisted bool read. When `false` the report machinery does nothing —
    /// no allocation, no recording, no print. Purely observational: it never gates
    /// any compile decision (the differential proves byte-identical behavior on/off).
    report: bool,
    /// Per-function set of `native_key`s that actually ran natively to completion at
    /// least once this run. Populated ONLY when `report` is on (gated like the
    /// stats counters). Lets the report print an accurate `native: ok` positive that
    /// matches the real runtime outcome (vs the static eligibility re-derivation).
    report_native_ok: std::collections::HashSet<usize>,
    /// Per-function set of `native_key`s that actually OSR-entered at least once.
    /// Populated ONLY when `report` is on. Accurate positive for `osr: entered`.
    report_osr_ok: std::collections::HashSet<usize>,
    /// True when the most recent failed OSR attempt entered native code and hit a
    /// dynamic uncommon trap. The auto-trigger uses this to back off and retry later
    /// instead of permanently marking the loop `GaveUp`.
    osr_dynamic_bail: bool,
}

#[cfg(feature = "native-jit")]
type OsrCache = HashMap<OsrVersionKey, Option<OsrEntry>>;

#[cfg(feature = "native-jit")]
impl NativeState {
    fn measure_translation<T>(&mut self, translate: impl FnOnce() -> T) -> T {
        if !self.collect_stats {
            return translate();
        }
        let started = std::time::Instant::now();
        let result = translate();
        self.stats.translation_nanos = self
            .stats
            .translation_nanos
            .saturating_add(started.elapsed().as_nanos());
        result
    }

    fn compile_phase_timings(&self) -> vm_jit::CompilePhaseTimings {
        let baseline = self.baseline_module.compile_phase_timings();
        let optimized = self
            .optimized_module
            .as_ref()
            .map(LazyNativeModule::compile_phase_timings)
            .unwrap_or_default();
        vm_jit::CompilePhaseTimings {
            validation_nanos: baseline
                .validation_nanos
                .saturating_add(optimized.validation_nanos),
            codegen_nanos: baseline
                .codegen_nanos
                .saturating_add(optimized.codegen_nanos),
            finalize_nanos: baseline
                .finalize_nanos
                .saturating_add(optimized.finalize_nanos),
        }
    }
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeCodeTier {
    Baseline,
    Optimized,
}

#[cfg(feature = "native-jit")]
struct OsrOptimizationSource {
    jit_fn: vm_jit::JitFunction,
    header: u32,
    exit: usize,
}

#[cfg(feature = "native-jit")]
#[derive(Debug)]
struct NativeAdmissionBudget {
    max_code_bytes: u64,
    max_compile_nanos: u128,
    admitted_code_bytes: u64,
    compile_nanos: u128,
    code_exhausted: bool,
}

/// Native-JIT telemetry. The VM is single-threaded, so plain counters suffice.
#[cfg(feature = "native-jit")]
#[derive(Debug, Default, Clone)]
pub struct NativeStats {
    /// Register storage classes proved once from the verified executable.
    pub verified_known_reg_types: u64,
    /// Registers whose nominal/storage type was erased by v1 and remains unknown.
    pub verified_unknown_reg_types: u64,
    /// Call sites whose target/signature projection remains present in v1.
    pub verified_known_call_sites: u64,
    /// Instruction effect records derived under the facts work budget.
    pub verified_instruction_effects: u64,
    /// Continuation regions admitted through the bounded typed block IR.
    pub typed_region_compiles: u64,
    /// Typed basic blocks compiled across continuation regions.
    pub typed_region_blocks: u64,
    /// Dense typed values represented across continuation regions.
    pub typed_region_values: u64,
    /// Bounded construction work consumed by typed region IRs.
    pub typed_region_work_units: u64,
    /// Aggregate candidates observed by the shared virtual-object analysis.
    pub virtual_objects_observed: u64,
    /// Virtual objects proven not to escape their native region.
    pub virtual_objects_no_escape: u64,
    /// Virtual objects requiring normal-exit materialization.
    pub virtual_objects_exit_only: u64,
    /// Virtual objects conservatively declined due to escape/unknown state.
    pub virtual_objects_declined: u64,
    /// Hot functions that reached the native tier (passed tiering, not force-bail).
    pub considered: u64,
    /// Calls deferred below the tier-up threshold (still on the interpreter).
    pub tier_deferred: u64,
    /// Functions that translated into the native IR.
    pub translated: u64,
    /// Functions rejected by translation (outside the native subset).
    pub not_eligible: u64,
    /// Weighted instructions that were native-lowerable but still ran through the
    /// interpreter. This is the primary "missed hot work" signal for deciding
    /// whether continuation regions would remove meaningful interpreter work.
    pub interpreted_native_work: u64,
    /// Dynamic normal-boundary counts grouped by stable barrier reason. These are
    /// observations only; they do not change eligibility or execution behavior.
    pub native_barrier_counts: BTreeMap<String, u64>,
    /// Stable native translation decline reasons, grouped by the same explanation
    /// used by the structured missed-optimization report.
    pub native_decline_reasons: BTreeMap<String, u64>,
    /// Functions Cranelift compiled to machine code.
    pub compiled: u64,
    /// Regions compiled by the `opt_level=none` baseline module.
    pub baseline_compiles: u64,
    /// Regions compiled by the `opt_level=speed` optimized module.
    pub optimized_compiles: u64,
    /// Successful baseline native dispatches, including OSR.
    pub baseline_calls: u64,
    /// Successful optimized native dispatches, including OSR.
    pub optimized_calls: u64,
    /// Baseline entries successfully promoted and admitted.
    pub promotions: u64,
    /// Total native IR instructions accepted by Cranelift across compiled regions.
    pub compiled_ir_instrs: u64,
    /// Total machine-code bytes emitted by Cranelift across compiled regions.
    pub compiled_code_bytes: u64,
    /// Compiled regions admitted to a dispatch cache by the hard JIT budgets.
    pub admission_admitted: u64,
    /// Machine-code bytes admitted to dispatch caches.
    pub admission_admitted_bytes: u64,
    /// Regions denied before compilation or rejected atomically after compilation.
    pub admission_rejected: u64,
    /// Machine-code bytes emitted for post-compile budget rejections. Pre-compile
    /// rejections contribute zero because no machine code exists.
    pub admission_rejected_bytes: u64,
    /// Total deopt/guard sites emitted across compiled regions.
    pub deopt_sites: u64,
    /// Bounds checks retained on direct flat-list loads and stores.
    pub direct_list_bounds_check_sites: u64,
    /// Direct flat-list bounds checks removed by the backend's existing range
    /// and provenance proof. Kept separate from retained sites so benchmarks
    /// can demonstrate an actual elimination rather than infer one.
    pub direct_list_bounds_checks_elided: u64,
    /// Canonical natural loops recognized from the shared OSR/helper-hoist facts.
    pub canonical_loops: u64,
    /// Canonical loops with one explicit predecessor outside the loop.
    pub canonical_loop_preheaders: u64,
    /// Canonical loops with a conservative affine induction-variable proof.
    pub canonical_induction_variables: u64,
    /// Number of source/edge work units consumed by the bounded immutable loop
    /// analysis cached on the verified executable.
    pub loop_analysis_work_units: u64,
    /// One or more functions hit the linear loop-analysis work ceiling. Other
    /// loop counters are conservative lower bounds when this is non-zero.
    pub loop_analysis_limit_reached: u64,
    /// Loop-invariant scalar host calls emitted with lazy memoization.
    pub memoized_runtime_helper_call_sites: u64,
    /// Ordinary, non-memoized host calls emitted across compiled regions.
    pub runtime_helper_call_sites: u64,
    /// Map/sorted-map match sites whose payload and found flag use one host call.
    pub fused_map_match_helper_sites: u64,
    /// Direct flat-list stores followed by the matching `Move` shape produced when
    /// an adjacent load is forwarded from the stored value.
    pub direct_list_store_load_forwarded_moves: u64,
    /// Native-to-native call sites emitted across compiled regions.
    pub native_call_edges: u64,
    /// Native call sites using the frame-free, infallible scalar internal ABI.
    pub direct_scalar_call_edges: u64,
    /// Small acyclic pure-scalar known calls routed through the existing static
    /// leaf inliner instead of compiling a separate callee entry.
    pub static_inline_candidates: u64,
    /// Deepest native-to-native call chain emitted across compiled regions.
    pub native_call_depth_max: u64,
    /// Profile-guided monomorphic closure guards emitted across compiled regions.
    pub profile_closure_guard_sites: u64,
    /// Profile-guided polymorphic closure dispatch id reads emitted across compiled regions.
    pub profile_closure_id_reads: u64,
    /// Profile-guided polymorphic inline-cache dispatch sites emitted.
    pub profile_closure_pic_sites: u64,
    /// Profile-guided polymorphic inline-cache arms emitted across all PIC sites.
    pub profile_closure_pic_arms: u64,
    /// Conditional branch sites with collected profile feedback.
    pub profile_branch_sites: u64,
    /// Total conditional branch samples collected across profiled branch sites.
    pub profile_branch_samples: u64,
    /// Samples where a profiled conditional branch jumped to its explicit target.
    pub profile_branch_taken: u64,
    /// Samples where a profiled conditional branch fell through to the next ip.
    pub profile_branch_fallthrough: u64,
    /// Backend blocks marked cold from strong profile-guided branch bias.
    pub profile_branch_cold_blocks: u64,
    /// Conditional branch edges compiled as profile-guided side exits.
    pub profile_branch_side_exits: u64,
    /// Functions that translated but failed to compile.
    pub compile_failed: u64,
    /// Native calls whose runtime args didn't match the inferred parameter types.
    pub arg_mismatch: u64,
    /// Baseline shape versions admitted to whole-function or OSR caches.
    pub shape_versions: u64,
    /// Native region instances keyed by verifier-proved concrete type
    /// arguments. Empty/erased v1 substitutions are intentionally excluded.
    pub static_type_instances: u64,
    /// Static instances declined by the bounded type-argument or per-function
    /// monomorphization limits.
    pub static_instance_limit_fallbacks: u64,
    /// Dispatches served by an existing shape-specific native version.
    pub shape_cache_hits: u64,
    /// New shapes declined after a tier/site reached its two-version cap.
    pub shape_limit_fallbacks: u64,
    /// Runtime bails attributed to the selected shape version.
    pub shape_bails: u64,
    /// Native calls that ran to completion.
    pub native_calls: u64,
    /// Native calls that bailed at a guard (overflow/div-by-zero/…) → interpreter.
    pub native_bails: u64,
    /// Native bails that originated in a nested native callee frame.
    pub native_child_bails: u64,
    /// Nested native callee bails reconstructed into an interpreter frame chain.
    pub native_child_resumes: u64,
    /// Total nanoseconds spent translating verified register bytecode into JIT IR.
    pub translation_nanos: u128,
    /// Total nanoseconds spent in the sealed JIT validator.
    pub validation_nanos: u128,
    /// Total nanoseconds spent lowering and defining Cranelift functions.
    pub codegen_nanos: u128,
    /// Total nanoseconds spent finalizing published machine code.
    pub finalize_nanos: u128,
    /// Total wall nanoseconds spent in admitted native compilation, including VM
    /// orchestration around the separately reported phases.
    pub compile_nanos: u128,
    /// Total nanoseconds spent executing native code.
    pub run_nanos: u128,
    /// OSR: OSR-entries that ran a loop natively mid-function and resumed at the
    /// post-loop ip (the forced-trigger success count).
    pub osr_entries: u64,
    /// Successful entries into a continuation region.
    pub continuation_entries: u64,
    /// Interpreter instruction positions checked against the hoisted candidate
    /// bitset. This is collected only in diagnostic mode.
    pub continuation_candidate_checks: u64,
    /// Candidate positions that crossed into full continuation preparation.
    pub continuation_full_probes: u64,
    /// Static instance keys built after a candidate probe was admitted.
    pub continuation_instance_key_builds: u64,
    /// Direct source instructions represented by admitted continuation regions.
    /// This is compile-time coverage evidence, not a dynamic execution count.
    pub continuation_compiled_source_instructions: u64,
    /// Normal commit-capable exits back to the VM trampoline.
    pub continuation_yields: u64,
    /// Step 1 cost model: regions that translated (were eligible) but the
    /// profitability gate kept on the interpreter. In `report` mode this counts
    /// regions that *would* decline without changing execution; in `enforce` mode
    /// it counts regions actually held back. The per-region reason is recorded in
    /// `unprofitable_decline_reasons`.
    pub unprofitable_declines: u64,
    /// Per-reason counts for cost-model profitability declines. Kept SEPARATE from
    /// `native_decline_reasons` because that map is rebuilt wholesale from the
    /// unit's native-ELIGIBILITY declines at run end (`add_native_decline_reasons`);
    /// profitability is a distinct, post-eligibility judgement and must not be
    /// clobbered by it.
    pub unprofitable_decline_reasons: BTreeMap<String, u64>,
    /// Runtime attribution: for each function the cost model declined this run, the
    /// (first) decline reason — ground truth for the report's per-function "declined
    /// by cost model" verdict, so it need not re-derive (which loses profile-guided
    /// PICs). Keyed by function name.
    pub unprofitable_declined_fns: BTreeMap<String, String>,
}

#[cfg(feature = "native-jit")]
struct JitCallCtxState {
    active_depth: usize,
    active_token: vm_jit::HostCtx,
    next_token: vm_jit::HostCtx,
    heap_args: Vec<VmValue>,
    heap_results: Vec<VmValue>,
    heap_result_roots: Vec<Option<usize>>,
    heap_writebacks: Vec<(usize, i64)>,
    deadline: Option<rsscript_operation::MonotonicDeadline>,
}

#[cfg(feature = "native-jit")]
impl JitCallCtxState {
    const fn new() -> Self {
        Self {
            active_depth: 0,
            active_token: 0,
            next_token: 1,
            heap_args: Vec::new(),
            heap_results: Vec::new(),
            heap_result_roots: Vec::new(),
            heap_writebacks: Vec::new(),
            deadline: None,
        }
    }

    fn reset_inputs(&mut self) {
        self.heap_args.clear();
    }

    fn clear_results(&mut self) {
        self.heap_results.clear();
        self.heap_result_roots.clear();
    }

    fn clear_writebacks(&mut self) {
        self.heap_writebacks.clear();
    }

    fn allocate_token(&mut self) -> vm_jit::HostCtx {
        let token = self.next_token.max(1);
        self.next_token = self.next_token.wrapping_add(1).max(1);
        token
    }
}

#[cfg(feature = "native-jit")]
thread_local! {
    /// Native call ABI state: heap input handles, speculative heap result handles,
    /// pending heap writebacks for the in-flight native call.
    ///
    /// Heap results and writebacks remain speculative until a clean native completion.
    /// On every bail/drop path the transaction/frame clears this context before the
    /// interpreter re-runs, so no helper result becomes observable accidentally.
    static JIT_CALL_CTX: RefCell<JitCallCtxState> =
        const { RefCell::new(JitCallCtxState::new()) };
    static JIT_STRING_LITERALS: RefCell<Vec<Rc<String>>> = const { RefCell::new(Vec::new()) };
    static JIT_HEAP_WRITE_UNDO: RefCell<Vec<JitHeapWriteUndo>> = const { RefCell::new(Vec::new()) };
    static JIT_HEAP_WRITE_SNAPSHOT_KEYS: RefCell<Vec<JitHeapSnapshotKey>> =
        const { RefCell::new(Vec::new()) };
    static JIT_HEAP_VALUE_CACHE: RefCell<Vec<JitHeapValueCache>> = const { RefCell::new(Vec::new()) };
    static JIT_SORTED_MAP_SCAN_CACHE: RefCell<Option<JitSortedMapScanCache>> =
        const { RefCell::new(None) };
    static JIT_LIST_HANDLE_CACHE: RefCell<Option<JitListHandleCache>> =
        const { RefCell::new(None) };
    static JIT_MAP_HANDLE_CACHE: RefCell<Option<JitMapHandleCache>> =
        const { RefCell::new(None) };
    static JIT_DEQUE_HANDLE_CACHE: RefCell<Option<JitDequeHandleCache>> =
        const { RefCell::new(None) };
    /// Native allocation meter. Unlike the step cell this is charged by
    /// the `ListPush*` HOST HELPER (the only native-subset op the interpreter bills to
    /// `allocation_budget`), not by generated code. `allocation_budget` is `None` when
    /// unarmed (the helper then charges nothing). The host seeds it before a native call and, on a CLEAN OSR
    /// exit, reads `allocated_bytes` back to commit the charges; on a bail the OSR rolls back
    /// the list writes and reruns on the interpreter, which recharges authoritatively, so
    /// the native charges are simply discarded (exact `account_bytes` parity).
    static JIT_MEM_CELL: std::cell::Cell<JitMemoryCell> = const {
        std::cell::Cell::new(JitMemoryCell { allocated_bytes: 0, allocation_budget: None })
    };
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy)]
struct JitMemoryCell {
    allocated_bytes: usize,
    allocation_budget: Option<usize>,
}

/// Seed the native limit accounting mem cell before a native call. `allocated_bytes` is the interpreter's current
/// accounted cumulative allocation total; `allocation_budget` is `None` when
/// disarmed. Every native call seeds the full-width `usize` state so a stale armed
/// budget cannot leak into a later `ListPush*` helper and large host budgets cannot
/// truncate through the machine-value ABI.
#[cfg(feature = "native-jit")]
fn jit_set_mem_cell(allocated_bytes: usize, allocation_budget: Option<usize>) {
    JIT_MEM_CELL.with(|cell| {
        cell.set(JitMemoryCell {
            allocated_bytes,
            allocation_budget,
        })
    });
}

/// Read the accumulated live-byte count back out of the mem cell after a CLEAN OSR exit
/// (to commit the native `ListPush*` charges into the interpreter's `allocated_bytes`).
#[cfg(feature = "native-jit")]
fn jit_allocation_cell_allocated_bytes() -> usize {
    JIT_MEM_CELL.with(|cell| cell.get().allocated_bytes)
}

/// Charge `grew` bytes (a `ListPush*` flat-capacity growth) against the armed mem cell,
/// mirroring the interpreter's `account_bytes`. Returns `false` if the budget is now
/// exceeded — the caller signals a bail, the OSR rolls back + reruns on the interpreter,
/// which recharges and errors at the exact push. An absent budget is a no-op.
#[cfg(feature = "native-jit")]
fn jit_mem_charge(grew: usize) -> bool {
    JIT_MEM_CELL.with(|cell| {
        let mut state = cell.get();
        let Some(budget) = state.allocation_budget else {
            return true;
        };
        state.allocated_bytes = state.allocated_bytes.saturating_add(grew);
        cell.set(state);
        state.allocated_bytes <= budget
    })
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy)]
struct JitSortedMapScanCache {
    handle: i64,
    next_index: usize,
}

#[cfg(feature = "native-jit")]
struct JitListHandleCache {
    handle: i64,
    list: Rc<RefCell<TypedVec>>,
}

#[cfg(feature = "native-jit")]
struct JitMapHandleCache {
    handle: i64,
    map: Rc<RefCell<ValueMap>>,
}

#[cfg(feature = "native-jit")]
struct JitDequeHandleCache {
    handle: i64,
    deque: Rc<RefCell<VecDeque<VmValue>>>,
}

#[cfg(feature = "native-jit")]
struct JitHeapValueCache {
    handle: i64,
    value: VmValue,
}

#[cfg(feature = "native-jit")]
enum JitHeapWriteUndo {
    List(Rc<RefCell<TypedVec>>, TypedVec),
    Map(Rc<RefCell<ValueMap>>, ValueMap),
    Deque(Rc<RefCell<VecDeque<VmValue>>>, VecDeque<VmValue>),
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum JitHeapSnapshotKey {
    List(*const RefCell<TypedVec>),
    Map(*const RefCell<ValueMap>),
    Deque(*const RefCell<VecDeque<VmValue>>),
}

/// Clears the per-call heap-arg table on drop, so a native attempt never retains
/// its cloned struct/list arguments past the call (on success, bail, or error).
#[cfg(feature = "native-jit")]
struct JitCallCtx;

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy)]
struct JitHostCallCtx {
    call_context: vm_jit::HostCtx,
}

#[cfg(feature = "native-jit")]
struct JitCallCtxGuard;

#[cfg(feature = "native-jit")]
impl JitCallCtxGuard {
    fn enter(deadline: Option<rsscript_operation::MonotonicDeadline>) -> Self {
        JitCallCtx::enter_frame(deadline);
        Self
    }
}

#[cfg(feature = "native-jit")]
impl Drop for JitCallCtxGuard {
    fn drop(&mut self) {
        if JitCallCtx::exit_frame() {
            jit_debug_assert_call_ctx_clean();
        }
    }
}

#[cfg(feature = "native-jit")]
struct JitNativeCallFrame {
    heap_tx: JitHeapTransactionGuard,
    _ctx: JitCallCtxGuard,
}

#[cfg(feature = "native-jit")]
impl JitNativeCallFrame {
    fn begin(deadline: Option<rsscript_operation::MonotonicDeadline>) -> Self {
        let ctx = JitCallCtxGuard::enter(deadline);
        let heap_tx = JitHeapTransactionGuard::begin_after_context_clear();
        Self { heap_tx, _ctx: ctx }
    }

    fn push_heap_arg(&self, value: VmValue) -> usize {
        JitCallCtx::push_heap_arg(value)
    }

    fn host_ctx(&self) -> vm_jit::HostCtx {
        JitCallCtx::active_token()
    }

    fn commit_scalar_with_writebacks(
        &mut self,
        input_slots: &[(usize, usize)],
    ) -> Option<Vec<(usize, VmValue)>> {
        self.heap_tx.commit_scalar_with_writebacks(input_slots)
    }

    fn commit_handle_with_writebacks(
        &mut self,
        handle: i64,
        input_slots: &[(usize, usize)],
    ) -> Option<(VmValue, Vec<(usize, VmValue)>)> {
        self.heap_tx
            .commit_handle_with_writebacks(handle, input_slots)
    }

    fn abort(&mut self) {
        self.heap_tx.abort();
    }

    fn can_precise_deopt_resume(&self) -> bool {
        self.heap_tx.can_precise_deopt_resume()
    }
}

#[cfg(feature = "native-jit")]
fn jit_debug_assert_call_ctx_clean() {
    debug_assert!(
        JIT_CALL_CTX.with(|ctx| ctx.borrow().active_depth == 0),
        "native call context leaked an active frame",
    );
    debug_assert!(
        JIT_CALL_CTX.with(|ctx| ctx.borrow().active_token == 0),
        "native call context leaked an active token",
    );
    debug_assert!(
        JIT_CALL_CTX.with(|ctx| ctx.borrow().heap_args.is_empty()),
        "native call context leaked heap arguments",
    );
    debug_assert!(
        JIT_CALL_CTX.with(|ctx| ctx.borrow().heap_results.is_empty()),
        "native call context leaked heap results",
    );
    debug_assert!(
        JIT_CALL_CTX.with(|ctx| ctx.borrow().heap_result_roots.is_empty()),
        "native call context leaked heap result roots",
    );
    debug_assert!(
        JIT_CALL_CTX.with(|ctx| ctx.borrow().heap_writebacks.is_empty()),
        "native call context leaked heap writebacks",
    );
    debug_assert!(
        JIT_HEAP_WRITE_UNDO.with(|undo| undo.borrow().is_empty()),
        "native call context leaked heap write undo entries",
    );
    debug_assert!(
        JIT_HEAP_WRITE_SNAPSHOT_KEYS.with(|keys| keys.borrow().is_empty()),
        "native call context leaked heap write snapshot keys",
    );
    debug_assert!(
        JIT_HEAP_VALUE_CACHE.with(|cache| cache.borrow().is_empty()),
        "native call context leaked heap value cache entries",
    );
    debug_assert!(
        JIT_LIST_HANDLE_CACHE.with(|cache| cache.borrow().is_none()),
        "native call context leaked list handle cache",
    );
    debug_assert!(
        JIT_MAP_HANDLE_CACHE.with(|cache| cache.borrow().is_none()),
        "native call context leaked map handle cache",
    );
    debug_assert!(
        JIT_DEQUE_HANDLE_CACHE.with(|cache| cache.borrow().is_none()),
        "native call context leaked deque handle cache",
    );
}

/// Transaction guard for heap values allocated by native host helpers. Helpers
/// publish into the call context's heap-result table, but those values stay speculative until the
/// native call completes without a bail. Dropping an uncommitted transaction aborts
/// it, so every early return/fallback path preserves the interpreter's visible
/// heap state.
#[cfg(feature = "native-jit")]
struct JitHeapTransactionGuard {
    finished: bool,
    owns_ctx_frame: bool,
}

#[cfg(feature = "native-jit")]
impl JitHeapTransactionGuard {
    fn begin_after_context_clear() -> Self {
        debug_assert!(
            JitCallCtx::is_active(),
            "native heap transaction must run inside an active native call context",
        );
        JitCallCtx::clear_heap_results();
        JitCallCtx::clear_heap_writebacks();
        jit_clear_heap_write_undo();
        Self {
            finished: false,
            owns_ctx_frame: false,
        }
    }

    fn commit_scalar_with_writebacks(
        &mut self,
        input_slots: &[(usize, usize)],
    ) -> Option<Vec<(usize, VmValue)>> {
        let writebacks = jit_materialize_heap_writebacks(input_slots)?;
        JitCallCtx::clear_heap_results();
        JitCallCtx::clear_heap_writebacks();
        jit_clear_heap_write_undo();
        jit_clear_heap_handle_caches();
        self.finished = true;
        Some(writebacks)
    }

    fn commit_handle_with_writebacks(
        &mut self,
        handle: i64,
        input_slots: &[(usize, usize)],
    ) -> Option<(VmValue, Vec<(usize, VmValue)>)> {
        let value = jit_materialize_heap_result(handle)?;
        let writebacks = jit_materialize_heap_writebacks(input_slots)?;
        JitCallCtx::clear_heap_results();
        JitCallCtx::clear_heap_writebacks();
        jit_clear_heap_write_undo();
        jit_clear_heap_handle_caches();
        self.finished = true;
        Some((value, writebacks))
    }

    fn abort(&mut self) {
        jit_restore_heap_writes();
        JitCallCtx::clear_heap_results();
        JitCallCtx::clear_heap_writebacks();
        jit_clear_heap_write_undo();
        jit_clear_heap_handle_caches();
        self.finished = true;
    }

    fn can_precise_deopt_resume(&self) -> bool {
        let no_heap_results = JitCallCtx::heap_results_empty();
        let no_heap_writebacks = JitCallCtx::heap_writebacks_empty();
        let no_heap_writes = JIT_HEAP_WRITE_UNDO.with(|undo| undo.borrow().is_empty())
            && JIT_HEAP_WRITE_SNAPSHOT_KEYS.with(|keys| keys.borrow().is_empty());
        no_heap_results && no_heap_writebacks && no_heap_writes
    }
}

#[cfg(feature = "native-jit")]
impl Drop for JitHeapTransactionGuard {
    fn drop(&mut self) {
        if !self.finished {
            jit_restore_heap_writes();
            JitCallCtx::clear_heap_results();
            JitCallCtx::clear_heap_writebacks();
            jit_clear_heap_write_undo();
            jit_clear_heap_handle_caches();
        }
        if self.owns_ctx_frame && JitCallCtx::exit_frame() {
            jit_debug_assert_call_ctx_clean();
        }
    }
}

#[cfg(feature = "native-jit")]
struct JitStringLiteralsGuard;

#[cfg(feature = "native-jit")]
impl Drop for JitStringLiteralsGuard {
    fn drop(&mut self) {
        JIT_STRING_LITERALS.with(|table| table.borrow_mut().clear());
    }
}

#[cfg(feature = "native-jit")]
fn jit_install_string_literals(literals: &[Rc<String>]) -> JitStringLiteralsGuard {
    JIT_STRING_LITERALS.with(|table| {
        *table.borrow_mut() = literals.to_vec();
    });
    JitStringLiteralsGuard
}

#[cfg(feature = "native-jit")]
fn jit_clear_heap_handle_caches() {
    JIT_HEAP_VALUE_CACHE.with(|cache| {
        cache.borrow_mut().clear();
    });
    JIT_LIST_HANDLE_CACHE.with(|cache| {
        *cache.borrow_mut() = None;
    });
    JIT_MAP_HANDLE_CACHE.with(|cache| {
        *cache.borrow_mut() = None;
    });
    JIT_DEQUE_HANDLE_CACHE.with(|cache| {
        *cache.borrow_mut() = None;
    });
}

#[cfg(feature = "native-jit")]
fn jit_clear_heap_write_undo() {
    JIT_HEAP_WRITE_UNDO.with(|undo| undo.borrow_mut().clear());
    JIT_HEAP_WRITE_SNAPSHOT_KEYS.with(|keys| keys.borrow_mut().clear());
}

#[cfg(feature = "native-jit")]
fn jit_mark_heap_snapshot(key: JitHeapSnapshotKey) -> bool {
    JIT_HEAP_WRITE_SNAPSHOT_KEYS.with(|keys| {
        let mut keys = keys.borrow_mut();
        if keys.contains(&key) {
            return false;
        }
        keys.push(key);
        true
    })
}

#[cfg(feature = "native-jit")]
fn jit_host_helpers() -> vm_jit::HostHelpers {
    // Typed `extern "C"` functions: `vm-jit` owns the raw-pointer conversion, so
    // `rsscript` never hands it an untyped address. Keeps this crate's
    // `#![forbid(unsafe_code)]` honest without an unsound safe API on the boundary.
    vm_jit::HostHelpers {
        deadline_expired: rss_jit_deadline_expired,
        field_int: rss_jit_field_int,
        field_set_int: rss_jit_field_set_int,
        field_set_handle: rss_jit_field_set_handle,
        field_set_float: rss_jit_field_set_float,
        list_len: rss_jit_list_len,
        list_is_empty: rss_jit_list_is_empty,
        list_get_int: rss_jit_list_get_int,
        list_set_int: rss_jit_list_set_int,
        list_set_float: rss_jit_list_set_float,
        list_push_int: rss_jit_list_push_int,
        list_push_handle: rss_jit_list_push_handle,
        list_push_float: rss_jit_list_push_float,
        list_sort_int: rss_jit_list_sort_int,
        list_new_int: rss_jit_list_new_int,
        field_float: rss_jit_field_float,
        list_get_float: rss_jit_list_get_float,
        closure_id: rss_jit_closure_id,
        closure_capture: rss_jit_closure_capture,
        field_closure_id: rss_jit_field_closure_id,
        field_closure_capture: rss_jit_field_closure_capture,
        field_handle: rss_jit_field_handle,
        list_get_handle: rss_jit_list_get_handle,
        string_from_int: rss_jit_string_from_int,
        string_len: rss_jit_string_len,
        string_concat: rss_jit_string_concat,
        string_slice: rss_jit_string_slice,
        string_pad_left: rss_jit_string_pad_left,
        string_pad_left_len: rss_jit_string_pad_left_len,
        string_split: rss_jit_string_split,
        string_starts_with: rss_jit_string_starts_with,
        string_split_count: rss_jit_string_split_count,
        string_literal: rss_jit_string_literal,
        json_parse: rss_jit_json_parse,
        json_field: rss_jit_json_field,
        json_field_int: rss_jit_json_field_int,
        bytes_len: rss_jit_bytes_len,
        bytes_slice: rss_jit_bytes_slice,
        map_insert_int: rss_jit_map_insert_int,
        map_insert_handle_key_int: rss_jit_map_insert_handle_key_int,
        map_insert_float: rss_jit_map_insert_float,
        map_get_int: rss_jit_map_get_int,
        map_get_match_int: rss_jit_map_get_match_int,
        map_get_match_float: rss_jit_map_get_match_float,
        map_contains_int: rss_jit_map_contains_int,
        map_len: rss_jit_map_len,
        map_is_empty: rss_jit_map_is_empty,
        set_insert_int: rss_jit_set_insert_int,
        set_insert_handle: rss_jit_set_insert_handle,
        set_len: rss_jit_set_len,
        set_is_empty: rss_jit_set_is_empty,
        sorted_set_insert_int: rss_jit_sorted_set_insert_int,
        sorted_set_insert_handle: rss_jit_sorted_set_insert_handle,
        sorted_set_contains_int: rss_jit_sorted_set_contains_int,
        sorted_set_is_empty: rss_jit_sorted_set_is_empty,
        sorted_map_insert_int: rss_jit_sorted_map_insert_int,
        sorted_map_insert_handle_key_int: rss_jit_sorted_map_insert_handle_key_int,
        sorted_map_get_int: rss_jit_sorted_map_get_int,
        sorted_map_get_float: rss_jit_sorted_map_get_float,
        sorted_map_contains_key_int: rss_jit_sorted_map_contains_key_int,
        sorted_map_is_empty: rss_jit_sorted_map_is_empty,
        sorted_map_len: rss_jit_sorted_map_len,
        deque_len: rss_jit_deque_len,
        deque_is_empty: rss_jit_deque_is_empty,
        deque_push_back_int: rss_jit_deque_push_back_int,
        deque_push_back_handle: rss_jit_deque_push_back_handle,
        deque_push_back_float: rss_jit_deque_push_back_float,
        deque_push_front_int: rss_jit_deque_push_front_int,
        deque_push_front_handle: rss_jit_deque_push_front_handle,
        deque_push_front_float: rss_jit_deque_push_front_float,
        deque_pop_front_int: rss_jit_deque_pop_front_int,
        deque_pop_back_int: rss_jit_deque_pop_back_int,
        deque_pop_front_float: rss_jit_deque_pop_front_float,
        deque_pop_back_float: rss_jit_deque_pop_back_float,
    }
}

#[cfg(feature = "native-jit")]
fn jit_verify_deopt_map(
    module: &vm_jit::NativeModule,
    id: vm_jit::CompiledId,
    jit_fn: &vm_jit::JitFunction,
    forced_safepoint: Option<u32>,
    required_resume_ip: Option<usize>,
) -> Result<(), String> {
    let map = module
        .deopt_map(id)
        .ok_or_else(|| "compiled function has no deopt map".to_string())?;
    let n_regs = usize::try_from(jit_fn.n_regs).map_err(|_| "n_regs overflow".to_string())?;
    if n_regs != jit_fn.reg_types.len() {
        return Err(format!(
            "n_regs/reg_types mismatch: n_regs={} reg_types={}",
            n_regs,
            jit_fn.reg_types.len()
        ));
    }
    if let Some(compiled_n_regs) = module.n_regs(id)
        && compiled_n_regs != n_regs
    {
        return Err(format!(
            "compiled n_regs mismatch: module={} jit_fn={}",
            compiled_n_regs, n_regs
        ));
    }
    if let Some(site) = forced_safepoint
        && site > 0
        && (site as usize) <= map.sites.len()
        && map.sites[(site - 1) as usize].resume_ip as usize >= jit_fn.source_instruction_count()
    {
        return Err(format!(
            "forced safepoint {site} resumes outside translated code"
        ));
    }

    let mut saw_required_resume = required_resume_ip.is_none();
    for (site_index, site) in map.sites.iter().enumerate() {
        let source_ip = site.source_ip as usize;
        let resume_ip = site.resume_ip as usize;
        if source_ip >= jit_fn.source_instruction_count() {
            return Err(format!(
                "deopt site {} has source ip {}, outside source code len {}",
                site_index + 1,
                source_ip,
                jit_fn.source_instruction_count()
            ));
        }
        if resume_ip >= jit_fn.source_instruction_count() {
            return Err(format!(
                "deopt site {} resumes at {}, outside source code len {}",
                site_index + 1,
                resume_ip,
                jit_fn.source_instruction_count()
            ));
        }
        if required_resume_ip == Some(resume_ip) {
            saw_required_resume = true;
        }
        for (reg, ty) in &site.live {
            let reg = *reg as usize;
            let Some(actual_ty) = jit_fn.reg_types.get(reg) else {
                return Err(format!(
                    "deopt site {} has out-of-range live reg {}",
                    site_index + 1,
                    reg
                ));
            };
            if actual_ty != ty {
                return Err(format!(
                    "deopt site {} live reg {} type mismatch: map={:?} reg_types={:?}",
                    site_index + 1,
                    reg,
                    ty,
                    actual_ty
                ));
            }
        }
    }
    if !saw_required_resume {
        return Err(format!(
            "compiled OSR function has no deopt site for required resume ip {}",
            required_resume_ip.expect("checked above")
        ));
    }
    Ok(())
}

#[cfg(feature = "native-jit")]
fn jit_verify_compiled_native(
    module: &vm_jit::NativeModule,
    id: vm_jit::CompiledId,
    jit_fn: &vm_jit::JitFunction,
    forced_safepoint: Option<u32>,
) -> Result<(), String> {
    jit_verify_deopt_map(module, id, jit_fn, forced_safepoint, None)
}

#[cfg(feature = "native-jit")]
fn jit_verify_compiled_osr(
    module: &vm_jit::NativeModule,
    id: vm_jit::CompiledId,
    jit_fn: &vm_jit::JitFunction,
    source_exit: usize,
) -> Result<(), String> {
    jit_verify_deopt_map(module, id, jit_fn, None, Some(source_exit))
}

#[cfg(feature = "native-jit")]
fn jit_native_verify_is_strict() -> bool {
    cfg!(debug_assertions)
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JitHeapHandle {
    Input(usize),
    Output(usize),
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

fn eval_int_compare(op: RegIntCompare, lhs: i64, rhs: i64) -> bool {
    match op {
        RegIntCompare::Less => lhs < rhs,
        RegIntCompare::LessEqual => lhs <= rhs,
        RegIntCompare::Greater => lhs > rhs,
        RegIntCompare::GreaterEqual => lhs >= rhs,
    }
}

fn int_overflow_error(operation: &str, lhs: i64, rhs: i64) -> EvalError {
    EvalError::Runtime(format!(
        "integer {operation} overflow: {lhs} and {rhs} exceed the Int range"
    ))
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

fn expect_sorted_map_entries(value: &VmValue) -> Result<Vec<(VmValue, VmValue)>, EvalError> {
    let entries = expect_list_ref(value)?;
    entries
        .borrow()
        .iter()
        .map(|entry| {
            let pair = expect_list_ref(&entry)?;
            let pair = pair.borrow();
            if pair.len() != 2 {
                return Err(EvalError::Runtime(format!(
                    "reg VM expected SortedMap entry, got `{}`.",
                    entry.display()
                )));
            }
            Ok((pair.get(0).unwrap(), pair.get(1).unwrap()))
        })
        .collect()
}

fn join_string_values(values: &TypedVec, separator: &str) -> Result<String, EvalError> {
    Ok(values
        .iter()
        .map(|value| expect_string_ref(&value).map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?
        .join(separator))
}

fn list_item_at(
    list: &Rc<RefCell<TypedVec>>,
    index: usize,
    operation: &str,
) -> Result<VmValue, EvalError> {
    let values = list.borrow();
    values.get(index).ok_or_else(|| {
        EvalError::Runtime(format!(
            "reg VM {operation} observed list length change at index {index}."
        ))
    })
}

fn expect_closure_rc(value: &VmValue) -> Result<Rc<VmClosure>, EvalError> {
    match value {
        VmValue::Closure(value) => Ok(Rc::clone(value)),
        VmValue::Managed(inner) => expect_closure_rc(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Closure, got `{}`.",
            other.display()
        ))),
    }
}

fn ensure_option_value(value: VmValue) -> Result<VmValue, EvalError> {
    match value {
        VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) | VmValue::OptionNone => {
            Ok(value)
        }
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Option, got `{}`.",
            other.display()
        ))),
    }
}

fn vm_value_from_map_key(key: &VmMapKey) -> VmValue {
    key.detached_value()
}

fn as_task_handle(value: &VmValue) -> Option<TaskId> {
    match value {
        VmValue::Native(native) if native.type_name.as_ref() == "Task" => Some(native.id as TaskId),
        _ => None,
    }
}
