#![allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::derivable_impls,
    clippy::doc_lazy_continuation,
    clippy::if_same_then_else,
    clippy::items_after_test_module,
    clippy::let_and_return,
    clippy::manual_contains,
    clippy::manual_slice_fill,
    clippy::mutable_key_type,
    clippy::needless_borrow,
    clippy::needless_lifetimes,
    clippy::needless_range_loop,
    clippy::nonminimal_bool,
    clippy::op_ref,
    clippy::ptr_arg,
    clippy::question_mark,
    clippy::redundant_closure,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_lazy_evaluations,
    clippy::useless_conversion
)]
// VM/JIT compatibility code keeps its lint debt local to this implementation tree.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
mod execution_plan;
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
mod tier;
mod value_access;
mod value_convert;
mod value_ops;
#[cfg(feature = "native-jit")]
pub use execution_plan::NativeJitOptions;
use execution_plan::{ExecutionPlan, StdoutMode, TierPlan};
#[cfg(feature = "native-jit")]
use execution_plan::{NativeAdmissionPolicy, NativeExecutionPlan};
pub(crate) use model::*;
#[cfg(feature = "native-jit")]
use native::*;
pub use planning::JitPlan;
use planning::*;
use resources::*;
use runtime_resources::*;
use runtime_values::*;
use tier::JitState;
use value_access::*;
use value_convert::*;
use value_ops::*;

/// Run `f` with the native-tier profitability cost model DISABLED on the current
/// thread, regardless of `RSS_JIT_COST_MODEL` (whose default is now `enforce`).
/// Race-free across parallel tests because the override is thread-local and native
/// compilation runs synchronously on the calling thread.
///
/// NOT part of the public API — this is internal JIT test machinery, exposed only so
/// the out-of-crate native-mechanism integration tests can observe a region compile
/// (e.g. a polymorphic closure inline cache) that the cost model would otherwise
/// decline. Hidden from docs; do not depend on it from user code.
#[doc(hidden)]
#[cfg(feature = "native-jit")]
pub fn with_native_cost_model_disabled<R>(f: impl FnOnce() -> R) -> R {
    let _guard = CostModeGuard::new(CostMode::Off);
    f()
}

// ============================================================================
// Central intrinsic/effect registry (JIT descriptor table)
// ============================================================================
//
// One `IntrinsicDescriptor` per `RegIntrinsic`, re-encoding the per-intrinsic
// facts the JIT's three hand-coded classification sites need. The table is the
// single source of truth for *which* intrinsics each site admits/expands/folds;
// the sites keep their exact lowering/fold/expansion *mechanism*.
//
// Conservative DEFAULT: the vast majority of the ~637 `RegIntrinsic` variants
// are opaque to the JIT — they allocate / write / suspend / are not foldable and
// not native-lowerable, so they BAIL out of the native subset. The `Default` impl
// encodes exactly that (`effect: Allocate`, every external_binding `false`). Only the
// intrinsics the three sites historically special-cased carry an explicit
// descriptor; populating richer facts for the rest is incremental future work and
// changes no behavior until a site is taught to read the new field.

/// The observable effect class of an intrinsic, as the JIT cares about it. Today's
/// sites only need to distinguish "pure/read" (safe to fold / re-run after a native
/// bail) from "allocate/write/suspend" (opaque to the native path). The richer
/// split is recorded for the future missed-optimization report (lever 2).
// The registry's consumers (the three JIT classification sites) are all
// `native-jit`-gated, so in a plain library build the table and its fields look
// dead. They are exercised under `--features native-jit` and by the table unit
// test; keep them compiled unconditionally as the lever-2 substrate.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntrinsicEffect {
    /// No observable effect; result depends only on the (read-only) operands.
    Pure,
    /// Reads heap/host state but mutates nothing (e.g. a length query).
    Read,
    /// Allocates a fresh heap value from its operands; observes/mutates nothing
    /// else. This is the conservative DEFAULT.
    Allocate,
    /// Mutates heap/host/collection state.
    Write,
    /// May suspend (async/stream/await).
    Suspend,
}

/// The role a foldable-string intrinsic plays in the string-length-fold pass: it is
/// either a *producer* of a string whose byte length the pass can compute from its
/// operands, or the length *query* itself. `None` ⇒ not part of that pass.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringFoldRole {
    /// `String.from_int` — produces an (always-ASCII) decimal string from an Int.
    ProducerFromInt,
    /// `String.slice` — produces a (length-law, ASCII-gated) substring.
    ProducerSlice,
    /// `String.len` — the byte-length query the pass dissolves into arithmetic.
    LengthQuery,
}

/// The role a foldable-Bytes intrinsic plays in the (Bytes sibling of the) query-fold
/// pass. Bytes are RAW bytes — there is no char/grapheme boundary, so the Bytes slice
/// length law is exact integer arithmetic with NO ASCII gate (unlike `String.slice`).
/// A producer's byte length is computed from its operands; the query is the length read.
/// `None` ⇒ not part of the Bytes fold.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BytesFoldRole {
    /// `Bytes.from_string` — produces raw bytes from a String; its byte length equals
    /// the source String's byte length (`value.as_bytes().len()`).
    ProducerFromString,
    /// `Bytes.slice` — produces a byte-index substring; the length law is the exact
    /// clamp arithmetic of `bytes_slice` (no char-boundary subtlety, no ASCII gate).
    ProducerSlice,
    /// `Bytes.len` — the byte-length query the pass dissolves into arithmetic.
    LengthQuery,
}

/// Per-intrinsic JIT facts, keyed by `RegIntrinsic` via [`intrinsic_descriptor`].
/// Every field defaults to the most conservative value so an unlisted intrinsic is
/// automatically opaque to all three sites (see the module note above).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
struct IntrinsicDescriptor {
    /// Observable effect class (pure/read vs allocate/write/suspend).
    effect: IntrinsicEffect,
    /// Whether a value this intrinsic produces can be folded away when used only by
    /// a read-only query (e.g. `String.from_int`/`String.slice` feeding `String.len`).
    /// Also marks the pure heap value-builders a deopt cold arm may re-run.
    can_fold: bool,
    /// Whether the intrinsic can be emitted directly in the native subset. The shape
    /// check stays at the call site.
    native_lowerable: bool,
    /// RESERVED for the future view work (zero-copy slice/borrow lowering). Set
    /// `false` for every intrinsic today; it exists only so the table shape is right
    /// for lever-2 / view consumers. No site reads it yet.
    view_capable: bool,
    /// If `Some`, this intrinsic is one of the six expandable Option/Result
    /// combinators, with its concrete lowering kind. The combinator-expansion pass
    /// uses this for *recognition*; it keeps the per-kind match/construct emission.
    combinator_kind: Option<CombinatorKind>,
    /// If `Some`, this intrinsic participates in the string-length-fold pass in the
    /// given role. The pass uses this for *classification*; it keeps the exact length
    /// laws and the ASCII-only-slice bail.
    string_fold_role: Option<StringFoldRole>,
    /// If `Some`, this intrinsic participates in the Bytes-length-fold pass in the
    /// given role (the Bytes sibling of `string_fold_role`). The pass uses this for
    /// *classification*; the exact byte-length laws stay in the pass. Bytes carry no
    /// char-boundary subtlety, so the slice law needs no ASCII gate.
    bytes_fold_role: Option<BytesFoldRole>,
    /// Whether this intrinsic is a pure, re-runnable heap String *builder* that the
    /// deopt-before-heap cold-arm classifier permits inside a bailable cold arm (it
    /// allocates a fresh String from read-only operands and observes/mutates nothing
    /// else). A tight whitelist; impure intrinsics (I/O, env, collections, time, RNG)
    /// are excluded. Distinct from `can_fold` (which also covers queries/combinators).
    cold_arm_pure_builder: bool,
    /// Whether this intrinsic is a pure, first-order, side-effect-free *reader* that
    /// returns a SCALAR (Int/Bool) and that the deopt cold-arm classifier permits inside
    /// a bailable cold arm (e.g. `String.count`/`String.index_of`/`String.contains`):
    /// it reads its operands, allocates nothing, and is faithfully re-runnable on the
    /// interpreter after a native `Bail` (native never executes the arm). Distinct from
    /// `cold_arm_pure_builder` (which allocates a fresh heap value). MUST be first-order:
    /// a higher-order/closure-taking intrinsic (the `Pure` combinators) is NOT eligible
    /// because the closure can have arbitrary effects — those are excluded by leaving
    /// this `false`. A tight whitelist; when unsure, leave `false`.
    cold_arm_pure_reader: bool,
    /// Short human-readable reason for the conservative classification, for the
    /// future missed-optimization report (e.g. "allocates", "suspends",
    /// "non-ASCII-dependent slice"). Empty for the trivial/expected cases.
    notes: &'static str,
}

impl Default for IntrinsicDescriptor {
    /// The conservative default for the ~637 intrinsics that no site special-cases:
    /// treat as an opaque allocator that the JIT cannot fold or lower.
    fn default() -> Self {
        IntrinsicDescriptor {
            effect: IntrinsicEffect::Allocate,
            can_fold: false,
            native_lowerable: false,
            view_capable: false,
            combinator_kind: None,
            string_fold_role: None,
            bytes_fold_role: None,
            cold_arm_pure_builder: false,
            cold_arm_pure_reader: false,
            notes: "default: opaque to JIT (allocate/not-foldable/not-native-lowerable)",
        }
    }
}

/// The central JIT descriptor for `intrinsic`. Returns the conservative
/// [`IntrinsicDescriptor::default`] for every intrinsic not explicitly listed (the
/// vast majority) and an explicit descriptor for the ones the three classification
/// sites historically special-cased.
#[allow(dead_code)]
fn intrinsic_descriptor(intrinsic: RegIntrinsic) -> IntrinsicDescriptor {
    use IntrinsicEffect::*;
    let d = IntrinsicDescriptor::default;
    match intrinsic {
        // --- native_subset_instruction: native-lowerable intrinsics ---
        // `Int.to_float` lowers to a native signed-int→f64 conversion (the single-Int
        // -arg shape check stays at the call site).
        RegIntrinsic::IntToFloat => IntrinsicDescriptor {
            effect: Pure,
            native_lowerable: true,
            notes: "native i64→f64 conversion (single Int arg)",
            ..d()
        },

        // --- Option/Result combinator expansion: the six expandable combinators ---
        RegIntrinsic::OptionMap => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::OptionMap),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::OptionAndThen => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::OptionAndThen),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::OptionUnwrapOr => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::OptionUnwrapOr),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::ResultMap => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::ResultMap),
            notes: "expandable pure Result combinator",
            ..d()
        },
        RegIntrinsic::ResultAndThen => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::ResultAndThen),
            notes: "expandable pure Result combinator",
            ..d()
        },
        RegIntrinsic::ResultUnwrapOr => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::ResultUnwrapOr),
            notes: "expandable pure Result combinator",
            ..d()
        },

        // --- string-length fold: the foldable string producers + the length query ---
        // `String.len` is a pure byte-length READ; the pass dissolves it to arithmetic.
        RegIntrinsic::StringLen => IntrinsicDescriptor {
            effect: Read,
            can_fold: true,
            native_lowerable: true,
            string_fold_role: Some(StringFoldRole::LengthQuery),
            notes: "byte-length query (foldable to arithmetic)",
            ..d()
        },
        // `String.from_int` allocates a fresh (always-ASCII) decimal string, but its
        // byte length is computable, so the length-fold pass can dissolve it; it is
        // also a whitelisted pure heap builder for deopt cold arms.
        RegIntrinsic::StringFromInt => IntrinsicDescriptor {
            effect: Allocate,
            can_fold: true,
            native_lowerable: true,
            string_fold_role: Some(StringFoldRole::ProducerFromInt),
            cold_arm_pure_builder: true,
            notes: "allocates ASCII decimal string; native-lowerable for final return",
            ..d()
        },
        // `String.slice` allocates a substring; foldable only when the source is
        // provably ASCII (the ASCII-gate stays in the pass).
        RegIntrinsic::StringSlice => IntrinsicDescriptor {
            effect: Allocate,
            can_fold: true,
            native_lowerable: true,
            string_fold_role: Some(StringFoldRole::ProducerSlice),
            cold_arm_pure_builder: true,
            notes: "allocates substring; native-lowerable and byte length foldable only when source is ASCII; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },
        RegIntrinsic::StringPadLeft => IntrinsicDescriptor {
            effect: Allocate,
            native_lowerable: true,
            cold_arm_pure_builder: true,
            notes: "allocates padded string; native-lowerable as a typed host helper; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },
        RegIntrinsic::StringSplit => IntrinsicDescriptor {
            effect: Allocate,
            native_lowerable: true,
            notes: "allocates List<String>; native-lowerable and split+len elidable",
            ..d()
        },
        RegIntrinsic::StringStartsWith => IntrinsicDescriptor {
            effect: Read,
            native_lowerable: true,
            cold_arm_pure_reader: true,
            notes: "string prefix query (Bool); native-lowerable; pure scalar reader (re-runnable after a cold-arm bail)",
            ..d()
        },
        // Pure first-order scalar string queries: read the operands, allocate nothing,
        // return Int/Bool. Eligible as cold-arm pure readers — faithfully re-runnable on
        // the interpreter after a native `Bail` (e.g. a cold arm `return String.count(s, n)`
        // whose heap source `s` is dead at the arm boundary; the scalar result is live-out).
        RegIntrinsic::StringCount | RegIntrinsic::StringContains | RegIntrinsic::StringIndexOf => {
            IntrinsicDescriptor {
                effect: Read,
                cold_arm_pure_reader: true,
                notes: "pure scalar string query (re-runnable after a cold-arm bail)",
                ..d()
            }
        }
        // `Map.len` is a pure scalar size query (Int); eligible as a cold-arm reader for
        // the arm-local `let m = Map.new(); m.insert(k, v); return Map.len(m)` shape.
        RegIntrinsic::MapLen => IntrinsicDescriptor {
            effect: Read,
            cold_arm_pure_reader: true,
            notes: "pure scalar map-size query (re-runnable after a cold-arm bail)",
            ..d()
        },
        // `Map.new` allocates a fresh empty map from no operands — a pure heap builder,
        // re-runnable after a cold-arm bail (the arm-local `Map.new()` of the shape above).
        RegIntrinsic::MapNew => IntrinsicDescriptor {
            effect: Allocate,
            cold_arm_pure_builder: true,
            notes: "allocates a fresh empty map; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },
        // `Set.new` / `Deque.new` — fresh empty collections (pure builders); their `.len`
        // is a pure scalar size query (reader). Same arm-local cold-arm shape as Map.
        RegIntrinsic::SetNew | RegIntrinsic::DequeNew => IntrinsicDescriptor {
            effect: Allocate,
            cold_arm_pure_builder: true,
            notes: "allocates a fresh empty collection; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },
        RegIntrinsic::SetLen | RegIntrinsic::DequeLen => IntrinsicDescriptor {
            effect: Read,
            cold_arm_pure_reader: true,
            notes: "pure scalar collection-size query (re-runnable after a cold-arm bail)",
            ..d()
        },

        // --- Bytes-length fold: the foldable Bytes producers + the length query ---
        // `Bytes.len` is a pure raw-byte-length READ (`value.len()`); the Bytes fold
        // dissolves it to arithmetic. No char/grapheme subtlety — raw bytes.
        RegIntrinsic::BytesLen => IntrinsicDescriptor {
            effect: Read,
            can_fold: true,
            native_lowerable: true,
            bytes_fold_role: Some(BytesFoldRole::LengthQuery),
            notes: "raw byte-length query (foldable to arithmetic; native-lowerable as a typed host helper)",
            ..d()
        },
        // `Bytes.from_string` allocates raw bytes from a String; its byte length is
        // exactly the source String's byte length (`as_bytes().len()`), so the Bytes
        // fold can dissolve it when the source length is known.
        RegIntrinsic::BytesFromString => IntrinsicDescriptor {
            effect: Allocate,
            can_fold: true,
            bytes_fold_role: Some(BytesFoldRole::ProducerFromString),
            cold_arm_pure_builder: true,
            notes: "allocates raw bytes from String; byte length = source String byte length; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },
        // `Bytes.slice` allocates a byte-index substring; its length is the exact clamp
        // arithmetic of `bytes_slice` — NO ASCII gate (raw bytes have no char boundary).
        RegIntrinsic::BytesSlice => IntrinsicDescriptor {
            effect: Allocate,
            can_fold: true,
            native_lowerable: true,
            bytes_fold_role: Some(BytesFoldRole::ProducerSlice),
            cold_arm_pure_builder: true,
            notes: "allocates byte-index substring; native-lowerable and byte length foldable; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },

        // --- deopt cold-arm pure heap builders (cold_arm_pure_intrinsic) ---
        // These allocate a fresh String from read-only operands and observe/mutate
        // nothing else, so a native Bail can discard the arm and the interpreter
        // re-runs it faithfully. (`StringFromInt` above already carries can_fold.)
        // The slice/pad/bytes producers above (`StringSlice`/`StringPadLeft`/
        // `BytesFromString`/`BytesSlice`) are the same shape — pure Allocate from
        // read-only operands — and also carry `cold_arm_pure_builder`; any
        // operand-domain error (e.g. a bad `String.slice` boundary) is raised
        // identically by the interpreter on re-run, so parity holds.
        RegIntrinsic::StringCopy | RegIntrinsic::StringFromBool | RegIntrinsic::StringFromFloat => {
            IntrinsicDescriptor {
                effect: Allocate,
                cold_arm_pure_builder: true,
                notes: "pure String builder (re-runnable after a native cold-arm bail)",
                ..d()
            }
        }

        // Everything else: conservative default (opaque allocator). Intentionally the
        // common case for the ~637 intrinsics; see the module note.
        _ => d(),
    }
}

// Not `native-jit`-gated: the intrinsic descriptor table (always compiled, for the
// table unit test and lever-2) embeds `Option<CombinatorKind>`. Read by the
// `native-jit` combinator-expansion pass and the table unit test.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CombinatorKind {
    OptionMap,
    OptionAndThen,
    OptionUnwrapOr,
    ResultMap,
    ResultAndThen,
    ResultUnwrapOr,
}

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
}

impl RegVmExecutable {
    /// Decode a bytecode envelope already accepted by `BytecodeVerifier`.
    /// Callers cannot manufacture `VerifiedBytecode`, so public construction
    /// keeps the generic Artifact verification phase explicit.
    pub fn from_verified_bytecode(
        verified: rsscript_bytecode::VerifiedBytecode,
    ) -> Result<Self, EvalError> {
        let (artifact, unit) = bytecode::decode_verified_bytecode(
            verified,
            rsscript_bytecode::VerificationContext::default(),
        )?
        .into_parts();
        Ok(Self {
            unit: Rc::new(unit),
            artifact,
        })
    }

    /// Bind this verified executable to its immutable workspace input.
    pub fn bind_snapshot_digest(&mut self, digest: impl Into<String>) -> Result<(), EvalError> {
        self.artifact
            .bind_snapshot_digest(digest)
            .map_err(|error| EvalError::Runtime(error.to_string()))
    }
}

impl RegVmExecutable {
    /// Serialize this already-verified executable as `rsscript.bytecode.v1`.
    pub fn to_bytecode(&self) -> Result<Vec<u8>, EvalError> {
        self.artifact
            .to_bytes()
            .map_err(|error| EvalError::Runtime(error.to_string()))
    }

    pub fn bytecode_artifact(&self) -> &rsscript_bytecode::BytecodeArtifact {
        &self.artifact
    }

    /// Return the canonical result value for `main` when its v1 declaration
    /// contains enough structural type information to do so. Unsupported
    /// values return `None`; the report never fabricates dynamic identities.
    fn main_result_wire_value(&self, value: VmValue) -> Option<WireValue> {
        let result = self
            .unit
            .native_signatures
            .get("main")?
            .return_type
            .as_deref()
            .map(WireType::parse)?;
        let record_layouts = self
            .unit
            .types
            .values()
            .map(|layout| WireRecordLayout {
                ty: WireType::parse(&layout.name),
                fields: layout
                    .fields
                    .iter()
                    .map(|field| WireRecordFieldLayout {
                        name: field.name.clone(),
                        ty: WireType::parse(&field.type_name),
                    })
                    .collect(),
            })
            .collect();
        let variant_layouts = self
            .unit
            .variant_layouts
            .values()
            .map(|layout| WireVariantLayout {
                ty: WireType::parse(&layout.name),
                variants: layout
                    .variants
                    .iter()
                    .map(|variant| WireVariantCaseLayout {
                        name: variant.name.clone(),
                        fields: variant
                            .fields
                            .iter()
                            .map(|field| WireRecordFieldLayout {
                                name: field.name.clone(),
                                ty: WireType::parse(&field.type_name),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        let signature = FunctionSignature {
            parameters: Vec::new(),
            result: result.clone(),
            asynchronous: false,
        };
        let types = WireCallTypeTable::for_signature(&signature)
            .and_then(|types| types.with_record_layouts(record_layouts))
            .and_then(|types| types.with_variant_layouts(variant_layouts))
            .ok()?;
        wire_value_from_vm_value(value, &result, &types).ok()
    }

    fn prepare_vm(
        &self,
        args: Vec<String>,
        external_bindings: HashMap<String, ExternalFunction>,
        plan: &ExecutionPlan,
    ) -> Result<RegVm, EvalError> {
        let mut vm = RegVm::new(
            Rc::clone(&self.unit),
            self.artifact.header.executable_hash.clone(),
            args,
            external_bindings,
        );
        vm.set_limits(plan.limits.clone());
        vm.stream_stdout = plan.stdout == StdoutMode::Streaming;
        match &plan.tier {
            TierPlan::Interpreter => {}
            TierPlan::Tier0 { force_all } => {
                vm.jit_enabled = true;
                vm.jit_force_all = *force_all;
            }
            #[cfg(feature = "native-jit")]
            TierPlan::Native(native) => {
                vm.native = Some(NativeState::new_with_plan(native)?);
                vm.jit_enabled = true;
                vm.jit_force_all = true;
            }
        }
        Ok(vm)
    }

    /// Per-function JIT eligibility analysis (the tier-0 "compile" step). A
    /// function is eligible when it is non-suspending and non-recursive — every
    /// instruction in the JIT-supported subset or a `CallKnown` to another
    /// eligible function (see [`compute_jit_eligibility`]); otherwise it falls
    /// back to the interpreter.
    pub fn jit_plan(&self) -> JitPlan {
        let mut plan = JitPlan::default();
        for function in &self.unit.functions {
            plan.total_functions += 1;
            let eligible = function.code.iter().all(jit_supported_instruction);
            if eligible {
                plan.eligible_functions += 1;
            } else {
                plan.fallback_functions += 1;
            }
        }
        plan
    }

    pub fn eval_main_with_args(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings(
            args,
            std::iter::empty::<(String, ExternalFunction)>(),
        )
    }

    /// Run `main` with the tier-0 JIT enabled: JIT-eligible functions execute via
    /// the specializing executor, the rest via the interpreter. Output is
    /// identical to `eval_main_with_args` (verified by the N-way differential).
    pub fn eval_main_with_args_jit(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        // The differential/parity callers want every supported function JIT'd so
        // the whole covered subset is verified, not just loop functions.
        self.eval_main_with_args_and_external_bindings_jit_inner(
            args,
            std::iter::empty::<(String, ExternalFunction)>(),
            true,
            VmLimits::default(),
        )
    }

    /// Run the tier-0 executor with explicit limits. Trusted hosts that require
    /// unrestricted native-style recursion must opt in with
    /// [`VmLimits::unbounded_for_trusted_host`].
    pub fn eval_main_with_args_jit_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings_jit_inner(
            args,
            std::iter::empty::<(String, ExternalFunction)>(),
            true,
            limits,
        )
    }

    /// Run `main` with the native (Cranelift) JIT enabled: the integer/control
    /// core executes as machine code, tier-0 covers the rest of the supported
    /// subset, and the interpreter the remainder. Output is identical to
    /// `eval_main_with_args` (verified by the N-way differential). Compiles
    /// eligible functions on first call (threshold 0) so the differential
    /// exercises them.
    ///
    /// The default tier-up threshold is 0 (compile on first call), which keeps
    /// the differential's full coverage. The `RSS_JIT_TIER_THRESHOLD` env var
    /// overrides it with any valid `u32`: a function then only compiles to
    /// baseline native after accumulating more than that many deterministic
    /// interpreted-work units. This is the runtime knob for tuning/measuring
    /// tier-up (see plan §3.4) and for production
    /// deployments that want to defer native compilation for cold functions.
    /// The differential never sets it, so its behavior is unchanged.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        let tier_up_threshold = std::env::var("RSS_JIT_TIER_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        self.eval_main_with_args_native_inner(
            args,
            tier_up_threshold,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            // J0.1: precise resume is the production DEFAULT. A native guard bail
            // reconstructs the live interpreter window (heap-aware: scalars restored,
            // heap/flat regs left to the frame) and resumes at the safepoint. It is
            // byte-identical to re-run-from-top (validated corpus-wide), which remains
            // the fallback when a heap write disables precise resume and is kept under
            // differential coverage by the force-deopt backend.
            true,
            false,
            None,
            false,
        )
        .map(|(output, _stats)| output)
    }

    /// Like [`Self::eval_main_with_args_native`] but also returns the native-tier
    /// [`NativeStats`] from the run (for benchmark/telemetry reporting).
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_with_stats(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        let tier_up_threshold = std::env::var("RSS_JIT_TIER_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        self.eval_main_with_args_native_inner(
            args,
            tier_up_threshold,
            false,
            true,
            // J0.1: precise resume is the production default (see
            // `eval_main_with_args_native`).
            true,
            false,
            None,
            false,
        )
    }

    /// Benchmark/test entry point that pins tier-up without mutating process
    /// environment. Keeping this explicit prevents parallel test cases from
    /// changing one another's native compilation policy.
    #[doc(hidden)]
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_at_threshold(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            tier_up_threshold,
            false,
            false,
            true,
            false,
            None,
            false,
        )
        .map(|(output, _stats)| output)
    }

    /// Stats variant of [`Self::eval_main_with_args_native_at_threshold`].
    #[doc(hidden)]
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_with_stats_at_threshold(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            tier_up_threshold,
            false,
            true,
            true,
            false,
            None,
            false,
        )
    }

    /// Like [`Self::eval_main_with_args_native_osr`] but also returns the
    /// native-tier [`NativeStats`] (notably `osr_entries`) for bench telemetry.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_osr_with_stats(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner(args, 0, false, true, true, true, None, false)
    }

    /// Run `main` with the native tier AND J5.2 OSR forced on (deterministically,
    /// independent of `RSS_JIT_OSR`): a function with a qualifying native-subset
    /// hot loop runs that loop natively mid-function (OSR-entry at the header,
    /// OSR-exit/precise-resume at the post-loop ip). Must equal every other backend
    /// byte-for-byte. Test/validation + bench entry point.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_osr(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            // OSR-exit resumes via the precise-deopt path, so OSR implies precise.
            true,
            true,
            None,
            false,
        )
        .map(|(output, _stats)| output)
    }

    /// Run `main` with the native tier AND J0.2 precise resume forced on,
    /// regardless of `RSS_JIT_PRECISE_DEOPT`. Native code runs for real; when it
    /// bails at a real guard safepoint, the live interpreter register window is
    /// reconstructed and interpretation resumes AT the safepoint (instead of re-
    /// running the function from the top). The observable result must equal every
    /// non-precise backend. Test/validation entry point only — lets the test set
    /// `precise_deopt` deterministically without a (racy) process env var.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_precise(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            true,
            false,
            None,
            false,
        )
        .map(|(output, _stats)| output)
    }

    /// Run `main` with the native tier in **deopt stress mode**: the native code
    /// always bails at its first guard, so every native-eligible function falls
    /// back to the interpreter. Its output must equal every other backend — this
    /// is how the deopt/fallback path is verified end-to-end.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_force_deopt(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            true,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            false,
            false,
            None,
            false,
        )
        .map(|(output, _stats)| output)
    }

    /// Run `main` while forcing the selected native safepoint to deopt. Unlike
    /// [`Self::eval_main_with_args_native_force_deopt`], this still enters native
    /// code and captures the safepoint's live register payload before falling back
    /// or precise-resuming, so it exercises the real deopt machinery.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_force_safepoint(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        safepoint: u32,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            true,
            false,
            Some(safepoint),
            false,
        )
        .map(|(output, _stats)| output)
    }

    /// Run `main` while forcing every generated native safepoint to deopt.
    /// Unlike process-env `RSS_JIT_DEOPT_EVERY`, this is deterministic and safe
    /// for in-process differential tests and fuzzers.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_force_all_safepoints(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            true,
            false,
            None,
            true,
        )
        .map(|(output, _stats)| output)
    }

    /// Test/validation entry point for the lever-2 missed-optimization report. Runs
    /// `main` with the native tier + OSR forced on AND the report armed deterministically
    /// (independent of the `RSS_JIT_REPORT` env var), returning the report block lines
    /// alongside the stats. The report is observational, so the `EvalOutput` is byte-
    /// identical to [`Self::eval_main_with_args_native_osr`]; this just also hands the
    /// caller the report so a test can assert the per-region reasons.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_osr_report(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats, Vec<String>), EvalError> {
        // Cranelift does not yet account the interpreter's execution budgets.
        // This explicitly native, experimental entry point is therefore trusted-only;
        // bounded callers must use the corresponding `_with_limits` entry point,
        // which keeps execution on the interpreter while limits are armed.
        self.eval_main_with_args_native_inner_reported(
            args,
            0,
            false,
            true,
            true,
            true,
            true,
            None,
            false,
            VmLimits::unbounded_for_trusted_host(),
        )
    }

    #[cfg(feature = "native-jit")]
    fn eval_main_with_args_native_inner(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        precise_deopt_override: bool,
        osr_override: bool,
        forced_safepoint: Option<u32>,
        force_all_safepoints_override: bool,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        // Native code does not yet poll or account interpreter budgets. Keep this
        // opt-in experimental API explicit about its trusted-host execution model.
        self.eval_main_with_args_native_inner_reported(
            args,
            tier_up_threshold,
            force_bail,
            collect_stats,
            precise_deopt_override,
            osr_override,
            false,
            forced_safepoint,
            force_all_safepoints_override,
            VmLimits::unbounded_for_trusted_host(),
        )
        .map(|(output, stats, _lines)| (output, stats))
    }

    /// Like [`Self::eval_main_with_args_native_with_stats`] but runs under explicit
    /// [`VmLimits`]. With native enabled, an armed `step_budget`/`cancel`/`allocation_budget`
    /// must prevent native dispatch (Cranelift polls/accounts none of them) — used to
    /// regression-test the recursive native fast-path limit gate.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner_reported(
            args, 0, false, true, true, false, false, None, false, limits,
        )
        .map(|(output, stats, _lines)| (output, stats))
    }

    /// Like [`Self::eval_main_with_args_native_osr_with_stats`] but under explicit
    /// [`VmLimits`]. Any armed step, cancellation, memory, or host-call limit keeps
    /// OSR on the interpreter until transformed regions carry exact source resource
    /// costs. Tests use this entry point to verify both the result and that
    /// `osr_entries == 0` under those modes.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_osr_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner_reported(
            args, 0, false, true, true, true, false, None, false, limits,
        )
        .map(|(output, stats, _lines)| (output, stats))
    }

    #[cfg(feature = "native-jit")]
    #[allow(clippy::too_many_arguments)]
    fn eval_main_with_args_native_inner_reported(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        precise_deopt_override: bool,
        osr_override: bool,
        report_override: bool,
        forced_safepoint: Option<u32>,
        force_all_safepoints_override: bool,
        limits: VmLimits,
    ) -> Result<(EvalOutput, NativeStats, Vec<String>), EvalError> {
        let native_plan = NativeExecutionPlan::from_environment(
            tier_up_threshold,
            force_bail,
            collect_stats,
            precise_deopt_override,
            osr_override,
            report_override,
            forced_safepoint,
            force_all_safepoints_override,
        );
        let plan = ExecutionPlan::native(limits, native_plan);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            HashMap::new(),
            &plan,
        )?;
        let value = vm.run_program("main")?;
        let jit_state = &vm.jit_state;
        if let Some(native) = &mut vm.native
            && native.collect_stats
        {
            native.stats.add_profile_feedback(&self.unit, jit_state);
            native
                .stats
                .add_native_decline_reasons(&self.unit, jit_state);
        }
        // Telemetry: `RSS_JIT_STATS=1` prints where native-tier attempts went, so
        // the next coverage win is measurable.
        if std::env::var_os("RSS_JIT_STATS").is_some()
            && let Some(native) = &vm.native
        {
            eprintln!("{}", native.stats.summary());
        }
        // Lever 2: `RSS_JIT_REPORT=1` (or `report_override`) prints the per-hot-region
        // missed-optimization report (why each function/loop did or didn't go
        // native/OSR/…). Purely observational; emitted once after the run, deduped per
        // function. When armed via the env var we print to stderr; the structured lines
        // are always returned so a test/caller can assert them.
        let report_lines = if let Some(native) = &vm.native
            && native.report
        {
            let lines = jit_missed_opt_report(&self.unit, &vm.jit_state, native);
            if std::env::var_os("RSS_JIT_REPORT").is_some() {
                for line in &lines {
                    eprintln!("{line}");
                }
            }
            lines
        } else {
            Vec::new()
        };
        let stats = vm
            .native
            .as_ref()
            .map(|native| native.stats.clone())
            .unwrap_or_default();
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok((
            EvalOutput {
                usage: vm.usage(),
                value: display_value.clone(),
                display_value,
                stdout: vm.stdout,
                stderr: vm.stderr,
                provider_call_traces: vm.provider_trace.snapshot(),
            },
            stats,
            report_lines,
        ))
    }

    /// Like [`eval_main_with_args_jit`] but with native host bindings, using the
    /// production has-loop heuristic (only loop functions are JIT'd).
    pub fn eval_main_with_args_and_external_bindings_jit(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings_jit_inner(
            args,
            external_bindings,
            false,
            VmLimits::default(),
        )
    }

    fn eval_main_with_args_and_external_bindings_jit_inner(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        force_all: bool,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let plan = ExecutionPlan::tier0(limits, force_all);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            external_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
            &plan,
        )?;
        let value = vm.run_program("main")?;
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok(EvalOutput {
            usage: vm.usage(),
            value: display_value.clone(),
            display_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
            provider_call_traces: vm.provider_trace.snapshot(),
        })
    }

    /// Like [`Self::eval_main_with_args_and_external_bindings`] but streams program
    /// stdout (`Output.write` output) live to the real process stdout, line-flushed,
    /// as the program runs. This lets a library caller show output immediately
    /// instead of buffering until exit. The returned
    /// `EvalOutput.stdout` is still the full captured buffer (identical to the
    /// non-streaming call), so the program output has already been written to the
    /// terminal — the caller must NOT print it a second time.
    pub fn eval_main_with_args_and_external_bindings_streaming_stdout(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings_streaming_stdout_with_limits(
            args,
            external_bindings,
            VmLimits::default(),
        )
    }

    /// Streaming variant with an explicit output/resource budget.
    pub fn eval_main_with_args_and_external_bindings_streaming_stdout_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let plan = ExecutionPlan::streaming(limits);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            external_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
            &plan,
        )?;
        let result = vm.run_program("main");
        // Flush any final line that lacks a trailing newline so no output is lost.
        let flush_result = if vm.stream_flushed < vm.stdout.len() {
            let mut out = std::io::stdout();
            out.write_all(&vm.stdout.as_bytes()[vm.stream_flushed..])
                .and_then(|()| out.flush())
                .map_err(|error| EvalError::Runtime(format!("failed to stream stdout: {error}")))
        } else {
            Ok(())
        };
        let value = result?;
        flush_result?;
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok(EvalOutput {
            usage: vm.usage(),
            value: display_value.clone(),
            display_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
            provider_call_traces: vm.provider_trace.snapshot(),
        })
    }

    /// Run `main` under explicit resource limits ([`VmLimits`]). Limits convert
    /// selected runaway behavior into `EvalError::Runtime`; they are not an
    /// isolation boundary. Output is otherwise identical to
    /// [`Self::eval_main_with_args`].
    pub fn eval_main_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let plan = ExecutionPlan::interpreter(limits);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            HashMap::new(),
            &plan,
        )?;
        let value = vm.run_program("main")?;
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok(EvalOutput {
            usage: vm.usage(),
            value: display_value.clone(),
            display_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
            provider_call_traces: vm.provider_trace.snapshot(),
        })
    }

    pub fn eval_main_with_args_and_external_bindings(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_external_bindings_and_limits(
            args,
            external_bindings,
            VmLimits::default(),
        )
    }

    /// Execute under explicit limits while retaining partial usage, output,
    /// Provider traces, and cleanup counters on failure.
    pub fn execute_main_with_args_and_external_bindings_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        limits: VmLimits,
    ) -> Result<EvalExecutionReport, EvalError> {
        let plan = ExecutionPlan::interpreter(limits);
        self.execute_main_with_args_and_external_bindings_plan(args, external_bindings, plan)
    }

    /// Execute through the adaptive native tier while preserving the same
    /// verified bytecode, Provider linking, limits, cleanup, and report path as
    /// the reference interpreter. Unsupported or unprofitable regions fall back
    /// to the interpreter. Armed limits remain authoritative: whole-function
    /// native dispatch is refused where exact accounting is unavailable, while
    /// verified OSR regions use the native limits cells.
    #[cfg(feature = "native-jit")]
    pub fn execute_main_with_args_and_external_bindings_native_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        limits: VmLimits,
    ) -> Result<EvalExecutionReport, EvalError> {
        self.execute_main_with_args_and_external_bindings_native_options_and_limits(
            args,
            external_bindings,
            NativeJitOptions::default(),
            limits,
        )
    }

    /// Native execution with a deterministic host-owned policy. This is the
    /// production embedding entry point; it never reads process environment.
    #[cfg(feature = "native-jit")]
    pub fn execute_main_with_args_and_external_bindings_native_options_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        options: NativeJitOptions,
        limits: VmLimits,
    ) -> Result<EvalExecutionReport, EvalError> {
        let native = NativeExecutionPlan::from_options(options);
        let plan = ExecutionPlan::native(limits, native);
        self.execute_main_with_args_and_external_bindings_plan(args, external_bindings, plan)
    }

    fn execute_main_with_args_and_external_bindings_plan(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        plan: ExecutionPlan,
    ) -> Result<EvalExecutionReport, EvalError> {
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            external_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
            &plan,
        )?;
        let execution = vm.run_program("main");
        #[cfg(feature = "native-jit")]
        if let Some(native) = &mut vm.native
            && native.collect_stats
        {
            native.stats.add_profile_feedback(&self.unit, &vm.jit_state);
            native
                .stats
                .add_native_decline_reasons(&self.unit, &vm.jit_state);
            if std::env::var_os("RSS_JIT_STATS").is_some() {
                eprintln!("{}", native.stats.summary());
            }
        }
        #[cfg(feature = "native-jit")]
        let engine =
            vm.native
                .as_ref()
                .map_or(crate::ExecutionEngineTelemetry::Interpreter, |native| {
                    crate::ExecutionEngineTelemetry::Native {
                        considered: native.stats.considered,
                        compiled: native.stats.compiled,
                        native_calls: native.stats.native_calls,
                        native_bails: native.stats.native_bails,
                        osr_entries: native.stats.osr_entries,
                        compile_nanos: native.stats.compile_nanos,
                        run_nanos: native.stats.run_nanos,
                    }
                });
        #[cfg(not(feature = "native-jit"))]
        let engine = crate::ExecutionEngineTelemetry::Interpreter;
        let cleanup = vm.cleanup_provider_resources();
        let result = match (execution, cleanup) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        };
        let usage = vm.usage();
        let provider_call_traces = vm.provider_trace.snapshot();
        let stdout = vm.stdout;
        let stderr = vm.stderr;
        match result {
            Ok(value) => {
                let display_value = value.display();
                let wire_value = self.main_result_wire_value(value.clone());
                Ok(EvalExecutionReport {
                    usage,
                    value: Some(display_value.clone()),
                    display_value: Some(display_value),
                    wire_value,
                    stdout,
                    stderr,
                    provider_call_traces,
                    engine: engine.clone(),
                    failure: None,
                })
            }
            Err(error) => Ok(EvalExecutionReport {
                usage,
                value: None,
                display_value: None,
                wire_value: None,
                stdout,
                stderr,
                provider_call_traces,
                engine,
                failure: Some(error),
            }),
        }
    }

    pub fn eval_main_with_args_and_external_bindings_and_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let plan = ExecutionPlan::interpreter(limits);
        let mut vm = self.prepare_vm(
            args.into_iter().map(Into::into).collect(),
            external_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
            &plan,
        )?;
        let value = vm.run_program("main")?;
        vm.cleanup_provider_resources()?;
        let display_value = value.display();
        Ok(EvalOutput {
            usage: vm.usage(),
            value: display_value.clone(),
            display_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
            provider_call_traces: vm.provider_trace.snapshot(),
        })
    }
}

/// One activation record on the explicit call stack. The interpreter is
/// stackless: instead of `run_frame` recursing into itself for each RSScript
/// call, it pushes a `Frame` and keeps looping, so a task's whole call chain
/// lives in `RegVm::frames` and can be suspended/resumed (the foundation for
/// the cooperative async scheduler). Synchronous closure callbacks
/// (`List.map`/`sort_with`/…) still nest via `run_frame`, which is fine because
/// they can never `await`.
struct Frame {
    func: Rc<RegFunction>,
    ip: usize,
    base: usize,
    /// Absolute register in the caller that receives this frame's return value.
    /// `usize::MAX` marks a driver root (its value is returned out of `run_frame`).
    ret_dst: usize,
    /// `mut`-argument write-backs to perform when this frame completes:
    /// `(caller_abs_reg, this_frame_abs_reg)` pairs. The caller register receives
    /// the parameter's final (possibly mutated) value, so `mut` params propagate.
    /// Empty for the overwhelmingly common no-`mut`-arg call (then a no-op).
    mut_writeback: Vec<(usize, usize)>,
    /// Number of self-tail calls elided by TCO in this activation. Combined with
    /// the physical frame depth to preserve `VmLimits::max_depth` observability.
    tail_calls: usize,
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

/// Resource limits for one reg-VM execution.
///
/// These limits are resilience controls, not an isolation boundary. Public
/// defaults are deliberately finite so an accidentally non-terminating or
/// excessively allocating script fails with a recoverable error. Trusted hosts
/// that intentionally need an unlimited run must opt in through
/// [`VmLimits::unbounded_for_trusted_host`].
/// Note: not `Copy` — it carries an optional shared cancellation token. All
/// fields are public and `Clone`/struct-update (`..VmLimits::default()`) keep
/// callers ergonomic; the scalar budget fields are read by value as before.
#[derive(Debug, Clone)]
pub struct VmLimits {
    /// Maximum simultaneous call frames (recursion depth). Default-on and
    /// generous; checked before every frame push. `usize::MAX` effectively
    /// disables the cap.
    pub max_depth: usize,
    /// Maximum number of executed instructions over the whole run. `None` means
    /// unlimited. When `Some(limit)`, a run that executes more than
    /// `limit` instructions fails with a "step budget exceeded" error — this is
    /// what stops `while true {}`.
    pub step_budget: Option<u64>,
    /// Best-effort cumulative quota for VM-owned allocation and container
    /// capacity growth. `None` disables accounting (near-zero overhead).
    /// This is not a live-memory measurement or an operating-system sandbox.
    pub allocation_budget: Option<usize>,
    /// Maximum reachable RSScript value storage at an instruction boundary.
    /// Unlike `allocation_budget`, this counter subtracts unreachable values and
    /// deduplicates shared `Rc` nodes. The metric is a deterministic VM storage
    /// model, not allocator RSS and not memory owned inside Provider futures.
    pub live_memory_limit: Option<usize>,
    /// Host-level preemption hook. `None` means no polling (the off path is
    /// near-free: `tick()` never touches the atomic). When `Some`, the host can
    /// set the flag to `true` from anywhere (e.g. a watchdog thread on timeout or
    /// an abort signal) and the running evaluation is preempted at the next
    /// throttled step check — even inside a tight `while true {}` loop that never
    /// awaits or checks the cooperative RSS `CancellationToken`. The eval then
    /// returns a structured `ExecutionFailureKind::Cancelled`. This stops the *whole*
    /// eval; see the note in `tick()` on per-task preemption.
    pub cancel: Option<rsscript_operation::CancellationToken>,
    /// Monotonic execution deadline shared with Provider calls.
    pub deadline: Option<rsscript_operation::MonotonicDeadline>,
    /// Maximum output bytes exposed to an external Provider call.
    pub stdout_budget: Option<usize>,
    /// Maximum number of deterministic stdlib/runtime intrinsic calls — every
    /// `Type.method` dispatch out of VM bytecode into the built-in runtime
    /// implementation. External host effects are counted separately by
    /// `provider_call_budget`.
    /// `None` means uncounted. When `Some(limit)`, the call that would exceed
    /// `limit` fails with an "intrinsic call budget exceeded" error. This caps the volume
    /// of runtime-library calls independently of raw instruction count (a single
    /// intrinsic can do unbounded I/O), so an agent program can be limited to N
    /// operation even if it is individually cheap in `step_budget`
    /// terms.
    pub intrinsic_call_budget: Option<u64>,
    /// Maximum number of calls through an explicitly linked external Provider
    /// symbol. This counter does not include VM/runtime intrinsics.
    pub provider_call_budget: Option<u64>,
    /// Maximum simultaneously live Provider-owned resources.
    pub resource_limit: Option<usize>,
    /// Whether the synchronous interpreter may invoke a Provider function that
    /// declares `MayBlock`. Disabled by default so embedding hosts must choose
    /// an execution lane deliberately before admitting blocking host work.
    pub allow_blocking_provider_calls: bool,
}

/// Default recursion-depth cap: generous enough never to trip real code (deep
/// but finite recursion, the ML framework's call chains) yet finite, so an
/// unbounded self-recursive program is caught long before it can overflow the
/// native stack.
const DEFAULT_MAX_DEPTH: usize = 16_384;

/// How often `tick()` polls the ambient cancel flag (once every this many
/// instructions). A power of two so the modulo lowers to a mask. Small enough
/// that a watchdog preempts a tight loop within microseconds, large enough that
/// the relaxed atomic load is negligible amortized over real work.
const CANCEL_POLL_INTERVAL: u64 = 1024;

/// Estimated bytes charged per map entry: a key plus a `VmValue`, with hashmap
/// bookkeeping folded into the key term as a rough fudge factor.
const MAP_ENTRY_BYTES: usize = std::mem::size_of::<VmValue>() * 2;
const MAX_INTRINSIC_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_depth: 4_096,
            step_budget: Some(50_000_000),
            allocation_budget: Some(256 * 1024 * 1024),
            live_memory_limit: Some(128 * 1024 * 1024),
            cancel: None,
            deadline: None,
            stdout_budget: Some(4 * 1024 * 1024),
            intrinsic_call_budget: Some(1_000_000),
            provider_call_budget: Some(100_000),
            resource_limit: Some(4_096),
            allow_blocking_provider_calls: false,
        }
    }
}

impl VmLimits {
    /// Disable accounting limits for a host-controlled, trusted workload.
    ///
    /// This is intentionally verbose: process isolation and provider authority
    /// remain the host's responsibility, and ordinary callers should use
    /// [`Default`] instead.
    pub fn unbounded_for_trusted_host() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            step_budget: None,
            allocation_budget: None,
            live_memory_limit: None,
            cancel: None,
            deadline: None,
            stdout_budget: None,
            intrinsic_call_budget: None,
            provider_call_budget: None,
            resource_limit: None,
            allow_blocking_provider_calls: true,
        }
    }
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
/// the interpreter at the safepoint's `resume_ip` (J0.2, only under the
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

/// Stable runtime ABI shape used for bounded native multiversioning. Scalar
/// payloads and collection lengths are deliberately absent. Heap identities are
/// included only where the runtime already exposes stable dispatch metadata:
/// closure ABI class and interned struct/variant layouts. Closure function ids
/// stay in the existing mono/PIC feedback so a three-arm PIC does not consume
/// three whole-function shape versions.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum NativeParamShape {
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
    fn from_values<'a>(values: impl IntoIterator<Item = &'a VmValue>) -> Self {
        Self::from_shapes(values.into_iter().map(native_param_shape))
    }

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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NativeVersionKey {
    function: usize,
    shape: ShapeKey,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OsrVersionKey {
    region: RegionKey,
    shape: ShapeKey,
}

/// State for the native JIT tier: the Cranelift modules owning compiled code,
/// bounded per-function/per-region shape caches, and tiering/deopt knobs.
#[cfg(feature = "native-jit")]
struct NativeState {
    baseline_module: vm_jit::NativeModule,
    /// The speed-optimized tier. `None` is the explicit
    /// `RSS_JIT_BASELINE` baseline-only mode.
    optimized_module: Option<vm_jit::NativeModule>,
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
    /// Deterministic interpreted-work accumulated after baseline admission.
    promotion_work: HashMap<NativeVersionKey, u64>,
    optimize_work_threshold: u64,
    /// Per-version *consecutive* runtime-bail counts, keyed like `cache`.
    /// Incremented on every bail after native was chosen (arg mismatch or runtime
    /// guard), reset to 0 on a successful native completion. At
    /// `NATIVE_BAIL_GIVEUP_THRESHOLD` only that shape is negative-cached, so one
    /// failing shape cannot demote successful versions.
    bail_counts: HashMap<NativeVersionKey, u32>,
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
    /// Env-gated deopt stress mode (`RSS_JIT_DEOPT_EVERY`): when set, every
    /// generated native safepoint bails unconditionally.
    force_all_safepoints: bool,
    /// Explicit host opt-in for non-tail recursion in generated machine code.
    /// Disabled by default because the backend's static frame estimate is not a
    /// proof of the live host stack available at the call site.
    allow_recursive_calls: bool,
    /// Telemetry: where native-tier attempts go (so the next coverage win is
    /// measurable rather than guessed).
    stats: NativeStats,
    /// Whether to collect telemetry. Keep timing and counter updates out of the
    /// native-call hot path unless a caller explicitly asks for them.
    collect_stats: bool,
    /// J0.2 precise deopt: when set, a native bail at a known safepoint
    /// reconstructs the interpreter register window from the captured live values
    /// and resumes interpretation AT the safepoint's `resume_ip`, instead of
    /// re-running the function from the top. Default `false` ⇒ byte-identical
    /// re-run-from-top (the safe baseline). Wired from `RSS_JIT_PRECISE_DEOPT`.
    precise_deopt: bool,
    /// J5.2 OSR (on-stack replacement): when set, a function with a qualifying
    /// native-subset hot loop (see [`detect_single_natural_loop`]) runs that loop
    /// natively *mid-function* — the interpreter reaches the loop header, hands the
    /// register window to an OSR-compiled loop body, then resumes at the post-loop
    /// ip with the live-out window (OSR-exit / precise-deopt resume). Default
    /// execution uses the hot-backedge auto-trigger; this flag selects the eager
    /// trigger used by `RSS_JIT_OSR` and deterministic test/bench entry points.
    osr_enabled: bool,
    /// Deterministically ranked OSR candidates per function. Each fixed-size value
    /// contains at most [`MAX_OSR_REGIONS_PER_FUNCTION`] headers, so a function's
    /// interpreter overhead and native compile exposure stay bounded.
    osr_candidates: HashMap<usize, OsrCandidates>,
    /// Evaluation-local hot-backedge state, independent for each candidate region.
    /// A stable decline at one header cannot disable another header in the function.
    osr_triggers: HashMap<RegionKey, OsrTrigger>,
    /// OSR compile cache keyed by function, original loop header, and runtime
    /// shape. `Some(entry)` is compiled; `None` is a stable per-version decline.
    #[allow(clippy::type_complexity)]
    osr_cache: HashMap<OsrVersionKey, Option<OsrEntry>>,
    /// Optimized OSR entries and their bounded baseline recompile sources.
    optimized_osr_cache: HashMap<OsrVersionKey, OsrEntry>,
    osr_optimization_sources: HashMap<OsrVersionKey, OsrOptimizationSource>,
    osr_promotion_work: HashMap<OsrVersionKey, u64>,
    /// Abnormal OSR exits attributed to the selected shape version. Normal
    /// `OsrExit` deopts are successful entries and reset this counter.
    osr_bail_counts: HashMap<OsrVersionKey, u32>,
    /// Native self-recursion cache (native-call-ABI slice 3; generalized in Phase 2):
    /// per-function stable ordinal key compiled `CallSelf` entry, with the
    /// compiled parameter `NativeTy`s and return `NativeTy` so the dispatcher
    /// marshals scalar args (Int/Bool/Float) and wraps the result. `None` = known
    /// not natively self-recursion-compilable (fall back to the tier-0 i64 executor
    /// for i64-only bodies, or the full interpreter for non-i64 bodies).
    self_recursive_native: HashMap<usize, Option<(vm_jit::CompiledId, Vec<NativeTy>, NativeTy)>>,
    /// Native mutual-recursion cache (native-call-ABI slice 4; generalized to scalar
    /// Float in the Phase 2 follow-up): per-function stable ordinal key
    /// compiled group-member `(CompiledId, param_tys, ret)`. The dispatcher marshals
    /// each scalar arg (Int/Bool/Float) and wraps the `i64` result per `ret`, exactly
    /// like the self-recursion cache. Compiling any member of a recursive cycle
    /// compiles+caches the whole group. `None` = known not a natively-compilable
    /// mutual-recursion member (interpreter).
    mutual_recursive_native: HashMap<usize, Option<(vm_jit::CompiledId, Vec<NativeTy>, NativeTy)>>,
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
    /// Lever 2: `RSS_JIT_REPORT` missed-optimization report armed. Read ONCE from
    /// the env at construction (mirrors `collect_stats`), so the hot path pays only
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
    /// Hot functions that reached the native tier (passed tiering, not force-bail).
    pub considered: u64,
    /// Calls deferred below the tier-up threshold (still on the interpreter).
    pub tier_deferred: u64,
    /// Functions that translated into the native IR.
    pub translated: u64,
    /// Functions rejected by translation (outside the native subset).
    pub not_eligible: u64,
    /// Stable native translation decline reasons, grouped by the same explanation
    /// used by the human `RSS_JIT_REPORT` missed-optimization report.
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
    /// Total nanoseconds spent in Cranelift compilation.
    pub compile_nanos: u128,
    /// Total nanoseconds spent executing native code.
    pub run_nanos: u128,
    /// J5.2: OSR-entries that ran a loop natively mid-function and resumed at the
    /// post-loop ip (the forced-trigger success count).
    pub osr_entries: u64,
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
impl NativeStats {
    fn summary(&self) -> String {
        format!(
            "native-jit: considered={} translated={} compiled={} baseline_compiles={} optimized_compiles={} baseline_calls={} optimized_calls={} promotions={} ir_instrs={} code_bytes={} admission_admitted={} admission_admitted_bytes={} admission_rejected={} admission_rejected_bytes={} deopt_sites={} direct_list_bounds_check_sites={} memoized_runtime_helper_call_sites={} runtime_helper_call_sites={} fused_map_match_helper_sites={} direct_list_store_load_forwarded_moves={} native_call_edges={} native_call_depth_max={} profile_closure_guards={} profile_closure_id_reads={} profile_closure_pic_sites={} profile_closure_pic_arms={} profile_branch_sites={} profile_branch_samples={} profile_branch_taken={} profile_branch_fallthrough={} profile_branch_cold_blocks={} profile_branch_side_exits={} not_eligible={} top_decline={} \
compile_failed={} calls={} bails={} child_bails={} child_resumes={} arg_mismatch={} shape_versions={} shape_cache_hits={} shape_limit_fallbacks={} shape_bails={} tier_deferred={} \
compile_ms={:.3} run_ms={:.3} osr_entries={} unprofitable_declines={}",
            self.considered,
            self.translated,
            self.compiled,
            self.baseline_compiles,
            self.optimized_compiles,
            self.baseline_calls,
            self.optimized_calls,
            self.promotions,
            self.compiled_ir_instrs,
            self.compiled_code_bytes,
            self.admission_admitted,
            self.admission_admitted_bytes,
            self.admission_rejected,
            self.admission_rejected_bytes,
            self.deopt_sites,
            self.direct_list_bounds_check_sites,
            self.memoized_runtime_helper_call_sites,
            self.runtime_helper_call_sites,
            self.fused_map_match_helper_sites,
            self.direct_list_store_load_forwarded_moves,
            self.native_call_edges,
            self.native_call_depth_max,
            self.profile_closure_guard_sites,
            self.profile_closure_id_reads,
            self.profile_closure_pic_sites,
            self.profile_closure_pic_arms,
            self.profile_branch_sites,
            self.profile_branch_samples,
            self.profile_branch_taken,
            self.profile_branch_fallthrough,
            self.profile_branch_cold_blocks,
            self.profile_branch_side_exits,
            self.not_eligible,
            self.top_native_decline_reason(),
            self.compile_failed,
            self.native_calls,
            self.native_bails,
            self.native_child_bails,
            self.native_child_resumes,
            self.arg_mismatch,
            self.shape_versions,
            self.shape_cache_hits,
            self.shape_limit_fallbacks,
            self.shape_bails,
            self.tier_deferred,
            self.compile_nanos as f64 / 1.0e6,
            self.run_nanos as f64 / 1.0e6,
            self.osr_entries,
            self.unprofitable_declines,
        )
    }

    fn top_native_decline_reason(&self) -> String {
        self.native_decline_reasons
            .iter()
            .max_by(|(lhs_reason, lhs_count), (rhs_reason, rhs_count)| {
                lhs_count
                    .cmp(rhs_count)
                    .then_with(|| rhs_reason.cmp(lhs_reason))
            })
            .map(|(reason, count)| format!("{count}x {reason}"))
            .unwrap_or_else(|| "none".to_string())
    }

    fn add_native_decline_reasons(&mut self, unit: &RegUnit, jit_state: &JitState) {
        self.native_decline_reasons = native_decline_reason_counts(unit, jit_state);
    }

    fn add_profile_feedback(&mut self, unit: &RegUnit, jit_state: &JitState) {
        let mut sites = 0u64;
        let mut taken = 0u64;
        let mut fallthrough = 0u64;
        for func in &unit.functions {
            let Some(profile) = jit_state.profile(func) else {
                continue;
            };
            for (_, feedback) in profile.branch_feedback_sites() {
                sites += 1;
                taken += u64::from(feedback.taken);
                fallthrough += u64::from(feedback.fallthrough);
            }
        }
        self.profile_branch_sites = sites;
        self.profile_branch_taken = taken;
        self.profile_branch_fallthrough = fallthrough;
        self.profile_branch_samples = taken.saturating_add(fallthrough);
    }

    /// Telemetry as JSON for VM/JIT benchmark and reporting harnesses.
    pub fn to_json(&self) -> crate::serde_json::Value {
        let mut value = crate::serde_json::json!({
            "considered": self.considered,
            "translated": self.translated,
            "compiled": self.compiled,
            "compiled_ir_instrs": self.compiled_ir_instrs,
            "compiled_code_bytes": self.compiled_code_bytes,
            "admission_admitted": self.admission_admitted,
            "admission_admitted_bytes": self.admission_admitted_bytes,
            "admission_rejected": self.admission_rejected,
            "admission_rejected_bytes": self.admission_rejected_bytes,
            "deopt_sites": self.deopt_sites,
            "direct_list_bounds_check_sites": self.direct_list_bounds_check_sites,
            "memoized_runtime_helper_call_sites": self.memoized_runtime_helper_call_sites,
            "runtime_helper_call_sites": self.runtime_helper_call_sites,
            "direct_list_store_load_forwarded_moves": self.direct_list_store_load_forwarded_moves,
            "native_call_edges": self.native_call_edges,
            "native_call_depth_max": self.native_call_depth_max,
            "profile_closure_guard_sites": self.profile_closure_guard_sites,
            "profile_closure_id_reads": self.profile_closure_id_reads,
            "profile_closure_pic_sites": self.profile_closure_pic_sites,
            "profile_closure_pic_arms": self.profile_closure_pic_arms,
            "profile_branch_sites": self.profile_branch_sites,
            "profile_branch_samples": self.profile_branch_samples,
            "profile_branch_taken": self.profile_branch_taken,
            "profile_branch_fallthrough": self.profile_branch_fallthrough,
            "profile_branch_cold_blocks": self.profile_branch_cold_blocks,
            "profile_branch_side_exits": self.profile_branch_side_exits,
            "not_eligible": self.not_eligible,
            "native_decline_reasons": &self.native_decline_reasons,
            "compile_failed": self.compile_failed,
            "native_calls": self.native_calls,
            "bails": self.native_bails,
            "child_bails": self.native_child_bails,
            "child_resumes": self.native_child_resumes,
            "arg_mismatch": self.arg_mismatch,
            "tier_deferred": self.tier_deferred,
            "compile_ms": self.compile_nanos as f64 / 1.0e6,
            "run_ms": self.run_nanos as f64 / 1.0e6,
            "osr_entries": self.osr_entries,
            "unprofitable_declines": self.unprofitable_declines,
            "unprofitable_decline_reasons": &self.unprofitable_decline_reasons,
            "unprofitable_declined_fns": &self.unprofitable_declined_fns,
        });
        let object = value.as_object_mut().expect("stats JSON is an object");
        object.insert("baseline_compiles".into(), self.baseline_compiles.into());
        object.insert("optimized_compiles".into(), self.optimized_compiles.into());
        object.insert("baseline_calls".into(), self.baseline_calls.into());
        object.insert("optimized_calls".into(), self.optimized_calls.into());
        object.insert("promotions".into(), self.promotions.into());
        object.insert(
            "fused_map_match_helper_sites".into(),
            self.fused_map_match_helper_sites.into(),
        );
        object.insert("shape_versions".into(), self.shape_versions.into());
        object.insert("shape_cache_hits".into(), self.shape_cache_hits.into());
        object.insert(
            "shape_limit_fallbacks".into(),
            self.shape_limit_fallbacks.into(),
        );
        object.insert("shape_bails".into(), self.shape_bails.into());
        value
    }
}

/// Lever 2: the developer-facing missed-optimization report (`RSS_JIT_REPORT`).
///
/// Walks every function in `unit` and re-derives — **observationally, read-only** —
/// why each did or didn't go native / OSR / scalar-replace / inline / fold, with the
/// intrinsic-level reasons sourced from the central [`intrinsic_descriptor`] registry
/// (effect + notes). This RE-RUNS the same cheap predicates the real passes use
/// (`translate_to_native_jit`, `detect_single_natural_loop`, `native_subset_instruction`,
/// `native_inline_leaf_calls`) WITHOUT touching the passes themselves, so it cannot
/// change any compile decision — the proof is the byte-identical differential with the
/// report on or off. Positive verdicts (`native: ok`, `osr: entered`) are cross-checked
/// against the actual runtime outcome recorded in `report_native_ok` / `report_osr_ok`,
/// so a line the report prints as "ok"/"entered" really happened, and a "not …" line
/// really did not (the report-correctness tests assert this).
///
/// One block per function (deduped by construction — each function is visited once).
#[cfg(feature = "native-jit")]
fn jit_missed_opt_report(
    unit: &RegUnit,
    jit_state: &JitState,
    native: &NativeState,
) -> Vec<String> {
    let mut out = vec![format!("jit-report: summary\n  {}", native.stats.summary())];
    let native_decline_counts = native_decline_reason_counts(unit, jit_state);
    for func in &unit.functions {
        let profile_lines = jit_profile_report_lines(unit, jit_state, func);
        // Skip the synthetic/placeholder/trivial bodies: a body that is only the
        // lowerer's defensive `LoadUnit; Return` (≤ 2 instructions, no real work) is
        // not a "hot region" worth a block unless it accumulated profile feedback.
        // Tiny higher-order dispatchers often contain only `CallClosure; Return`,
        // and their profile is exactly the data J2 speculation consumes.
        if func.code.len() <= 2 && profile_lines.is_empty() {
            continue;
        }
        let key = jit_state.function_ordinal(func);
        let mut block = vec![format!("jit-report: fn `{}`", func.name)];

        // --- Native-tier verdict --------------------------------------------------
        match translate_to_native_jit(
            unit,
            func,
            jit_state.profile(func),
            jit_state.call_count(func),
        ) {
            Some(_) => {
                if native.report_native_ok.contains(&key) {
                    block.push("  native: ok".to_string());
                } else if let Some(reason) = native.stats.unprofitable_declined_fns.get(&func.name)
                {
                    // Runtime attribution (ground truth from this run's cost-model
                    // consult) — the common "why no JIT" case now the model enforces
                    // by default. Reliable even for profile-guided PICs, which a
                    // re-derivation here would miss.
                    block.push(format!("  not native: declined by cost model — {reason}"));
                } else {
                    // Eligible but never observed running natively this run
                    // (tier-deferred, not called hot, or demoted by another gate).
                    block.push("  native: eligible (not run natively this execution)".to_string());
                }
            }
            None => {
                let reason = native_decline_reason(unit, jit_state, func);
                block.push(format!("  not native: {reason}"));
            }
        }

        // --- OSR verdict ----------------------------------------------------------
        // ACCURACY FIRST: if the function actually OSR-entered this run, the verdict is
        // `osr: entered` regardless of any static re-derivation — the recorded runtime
        // outcome is ground truth. (The OSR pipeline applies several region transforms —
        // combinator expansion, leaf inlining, string-length folding, Option/Result/
        // variant/struct scalar replacement — before the subset check, so a body with a
        // *raw* allocating string/Option op can still OSR once those passes dissolve it;
        // re-deriving that whole pipeline here would be fragile, so we trust the outcome
        // for the positive and use the cheap static re-derivation only to EXPLAIN a
        // genuine non-entry.)
        if native.report_osr_ok.contains(&key) {
            block.push("  osr: entered".to_string());
        } else {
            match detect_single_natural_loop(&func.code) {
                None => {
                    if jit_function_has_loop(&func.code) {
                        block.push(
                            "  not osr: loop shape not a single reducible natural loop".to_string(),
                        );
                    } else {
                        block.push("  not osr: no loop".to_string());
                    }
                }
                Some(lp) => {
                    let checked = native_lower_checked_payload_intrinsics_in_region(
                        &func.code, func.regs, lp.header, lp.exit,
                    );
                    let (code, header, exit) = checked
                        .as_ref()
                        .map(|(code, _, _)| (code.as_slice(), lp.header, lp.exit))
                        .unwrap_or((func.code.as_slice(), lp.header, lp.exit));
                    // A candidate loop exists but it did not OSR. Surface the first
                    // disqualifier after cheap native-only checked-payload rewrites
                    // (registry-sourced for intrinsics) as the likely cause; if the
                    // body is already in the native subset, the decline was a
                    // downstream type/marshalling reason.
                    match first_non_subset_reason(&code[header..exit]) {
                        Some(reason) => block.push(format!("  not osr: loop body {reason}")),
                        None if native.report_native_ok.contains(&key) => block.push(
                            "  osr: n/a (whole function ran native; no mid-function OSR needed)"
                                .to_string(),
                        ),
                        None => block.push(
                            "  not osr: loop not lowered (type/marshalling decline)".to_string(),
                        ),
                    }
                }
            }
        }

        block.extend(profile_lines);
        out.push(block.join("\n"));
    }
    out.insert(1, jit_native_decline_summary_block(native_decline_counts));
    out.insert(2, jit_cost_model_decline_summary_block(&native.stats));
    out
}

/// "Why did the cost model keep functions on the interpreter?" — the per-reason
/// counts of profitability declines (each reason carries its score breakdown). Empty
/// (`none`) when the model is off or nothing was declined. Distinct from the native
/// ELIGIBILITY decline summary: these regions ARE valid native code, just judged
/// not worth it (native ≈ interpreter).
#[cfg(feature = "native-jit")]
fn jit_cost_model_decline_summary_block(stats: &NativeStats) -> String {
    let mut lines = vec!["jit-report: cost-model decline summary".to_string()];
    if stats.unprofitable_decline_reasons.is_empty() {
        lines.push("  none".to_string());
        return lines.join("\n");
    }
    let mut counts: Vec<(&String, &u64)> = stats.unprofitable_decline_reasons.iter().collect();
    counts.sort_by(|(lhs_reason, lhs_count), (rhs_reason, rhs_count)| {
        rhs_count
            .cmp(lhs_count)
            .then_with(|| lhs_reason.cmp(rhs_reason))
    });
    for (reason, count) in counts {
        lines.push(format!("  {count}× {reason}"));
    }
    lines.join("\n")
}

#[cfg(feature = "native-jit")]
fn native_decline_reason_counts(unit: &RegUnit, jit_state: &JitState) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::<String, u64>::new();
    for func in &unit.functions {
        let profile_lines = jit_profile_report_lines(unit, jit_state, func);
        if func.code.len() <= 2 && profile_lines.is_empty() {
            continue;
        }
        if translate_to_native_jit(
            unit,
            func,
            jit_state.profile(func),
            jit_state.call_count(func),
        )
        .is_none()
        {
            let reason = native_decline_reason(unit, jit_state, func);
            *counts.entry(reason).or_default() += 1;
        }
    }
    counts
}

#[cfg(feature = "native-jit")]
fn jit_native_decline_summary_block(counts: BTreeMap<String, u64>) -> String {
    let mut lines = vec!["jit-report: native decline summary".to_string()];
    if counts.is_empty() {
        lines.push("  none".to_string());
        return lines.join("\n");
    }

    let mut counts: Vec<(String, u64)> = counts.into_iter().collect();
    counts.sort_by(|(lhs_reason, lhs_count), (rhs_reason, rhs_count)| {
        rhs_count
            .cmp(lhs_count)
            .then_with(|| lhs_reason.cmp(rhs_reason))
    });
    for (reason, count) in counts {
        lines.push(format!("  {count}x {reason}"));
    }
    lines.join("\n")
}

#[cfg(feature = "native-jit")]
fn jit_profile_report_lines(
    unit: &RegUnit,
    jit_state: &JitState,
    func: &RegFunction,
) -> Vec<String> {
    let Some(profile) = jit_state.profile(func) else {
        return Vec::new();
    };
    let function_name = |id: usize| {
        unit.functions
            .get(id)
            .map(|func| func.name.as_str())
            .unwrap_or("<unknown>")
            .to_string()
    };
    let mut lines = Vec::new();
    for (ip, instr) in func.code.iter().enumerate() {
        if !matches!(instr, RegInstr::CallClosure { .. }) {
            continue;
        }
        let Some(feedback) = profile.call_sites.get(&ip) else {
            continue;
        };
        let state = match feedback.state() {
            MonoState::Monomorphic => "monomorphic",
            MonoState::Polymorphic => "polymorphic",
            MonoState::Megamorphic => "megamorphic",
        };
        let observed = feedback
            .observed
            .iter()
            .map(|(key, count)| {
                let name = usize::try_from(*key)
                    .ok()
                    .map(&function_name)
                    .unwrap_or_else(|| "<invalid>".to_string());
                format!("{name}:{count}")
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut line = format!("  profile: closure@{ip} {state} observed=[{observed}]");
        if !feedback.captures_all_scalar {
            line.push_str(" scalar-captures=false");
        }
        if let Some(target) = monomorphic_closure_inline_target(
            unit,
            func,
            Some(profile),
            jit_state.call_count(func),
            ip,
        ) {
            line.push_str(&format!(" guard={}", function_name(target)));
        } else if let Some(targets) = polymorphic_closure_inline_targets(
            unit,
            func,
            Some(profile),
            jit_state.call_count(func),
            ip,
        ) {
            let arm_count = targets.len();
            let order = targets
                .into_iter()
                .map(&function_name)
                .collect::<Vec<_>>()
                .join(",");
            line.push_str(&format!(" pic=hottest-first[{order}] pic_arms={arm_count}"));
        }
        lines.push(line);
    }
    for (ip, instr) in func.code.iter().enumerate() {
        if !matches!(
            instr,
            RegInstr::JumpIfBool { .. } | RegInstr::JumpIfIntCompare { .. }
        ) {
            continue;
        }
        let Some(feedback) = profile.branch_feedback(ip) else {
            continue;
        };
        let mut line = format!(
            "  profile: branch@{ip} taken={} fallthrough={} taken_pct={:.1} bias={}",
            feedback.taken,
            feedback.fallthrough,
            feedback.taken_percent(),
            profile.branch_bias(ip).as_str(),
        );
        if let Some(hot_target) = feedback.hot_edge() {
            let (hot_edge, cold_edge) = if hot_target {
                ("target", "fallthrough")
            } else {
                ("fallthrough", "target")
            };
            line.push_str(&format!(
                " hot_edge={hot_edge} side_exit_candidate={cold_edge}"
            ));
        }
        lines.push(line);
    }
    lines
}

/// Re-derive, observationally, the first reason whole-function native translation
/// declines `func`. Mirrors the early bails in [`translate_to_native_jit`] and then
/// scans the (leaf-inlined) reachable body for the first non-subset instruction —
/// reporting the intrinsic-level cause from the registry. Read-only.
#[cfg(feature = "native-jit")]
fn native_decline_reason(unit: &RegUnit, jit_state: &JitState, func: &RegFunction) -> String {
    if func.captures != 0 {
        return "function has captures (closure body, not a native leaf)".to_string();
    }
    // Re-run leaf inlining + aggregate scalar-replacement exactly as translation does,
    // so the reason reflects the FINAL body the native subset check sees. If any pass
    // bails, report that — these are the structural reasons the real pass declines on.
    let Some((code, _n_regs, _ip_map)) = native_inline_leaf_calls(
        unit,
        func,
        jit_state.profile(func),
        jit_state.call_count(func),
        false,
        None,
    ) else {
        return "contains a non-inlinable call (callee not native-inlinable)".to_string();
    };
    let region_exit = native_whole_function_region_exit(&code);
    let Some((code, _n_regs, _ip_map, _recipes)) =
        native_scalar_replace_results_in_region(&code, _n_regs, 0, region_exit)
    else {
        return "not scalar-replaced: Result escapes the region".to_string();
    };
    let Some((code, _n_regs, _payload, _ip_map)) = native_scalar_replace_options(&code, _n_regs)
    else {
        return "not scalar-replaced: Option escapes the region".to_string();
    };
    let region_exit = native_whole_function_region_exit(&code);
    let Some((code, _n_regs, _ip_map, _recipes)) =
        native_scalar_replace_variants_in_region(&code, _n_regs, 0, region_exit)
    else {
        return "not scalar-replaced: variant escapes the region".to_string();
    };
    let region_exit = native_whole_function_region_exit(&code);
    let Some((code, _n_regs, _ip_map, _recipes)) =
        native_scalar_replace_structs_in_region(&code, _n_regs, 0, region_exit)
    else {
        return "not scalar-replaced: struct escapes the region".to_string();
    };
    let reachable = native_reachable_instructions(&code);
    for (i, instr) in code.iter().enumerate() {
        if reachable[i]
            && !native_subset_instruction(instr)
            && let Some(reason) = instr_decline_reason(instr)
        {
            return reason;
        }
    }
    // Translation declined for a shape reason the above re-derivation doesn't pinpoint
    // (e.g. type unification conflict, param/reg count). Generic but honest.
    "outside the native subset (shape/type not lowerable)".to_string()
}

/// A report reason for why `body` is outside the native subset. Prefers the most
/// *substantive* cause — a non-pure (allocate/write/suspend/read) `CallIntrinsic`,
/// whose registry effect/notes are the real missed-opt explanation — over an
/// incidental non-subset instruction (e.g. a `LoadString` constant load that the
/// subset also rejects). Falls back to the first non-subset instruction otherwise.
/// `None` ⇒ the whole body is in the native subset.
#[cfg(feature = "native-jit")]
fn first_non_subset_reason(body: &[RegInstr]) -> Option<String> {
    // First: a non-subset effectful intrinsic (the headline reason).
    if let Some(instr) = body.iter().find(|instr| {
        !native_subset_instruction(instr)
            && matches!(
                instr,
                RegInstr::CallIntrinsic { .. } | RegInstr::CallTypedIntrinsic { .. }
            )
            && match instr {
                RegInstr::CallIntrinsic { intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, .. } => {
                    intrinsic_descriptor(*intrinsic).effect != IntrinsicEffect::Pure
                }
                _ => false,
            }
    }) {
        return instr_decline_reason(instr);
    }
    // Otherwise: the first non-subset instruction, whatever it is.
    body.iter()
        .find(|instr| !native_subset_instruction(instr))
        .map(|instr| instr_decline_reason(instr).unwrap_or_else(|| "outside native subset".into()))
}

/// Human-readable reason a single instruction is outside the native subset, with
/// the intrinsic-level effect/notes pulled from the central [`intrinsic_descriptor`]
/// registry for `CallIntrinsic`/`CallTypedIntrinsic`. `None` for a subset instruction.
#[cfg(feature = "native-jit")]
fn instr_decline_reason(instr: &RegInstr) -> Option<String> {
    if native_subset_instruction(instr) {
        return None;
    }
    Some(match instr {
        RegInstr::CallIntrinsic { intrinsic, .. }
        | RegInstr::CallTypedIntrinsic { intrinsic, .. } => {
            let d = intrinsic_descriptor(*intrinsic);
            let effect = match d.effect {
                IntrinsicEffect::Pure => "pure",
                IntrinsicEffect::Read => "read",
                IntrinsicEffect::Allocate => "allocate",
                IntrinsicEffect::Write => "write",
                IntrinsicEffect::Suspend => "suspend",
            };
            format!(
                "contains CallIntrinsic {:?} (effect={}; {})",
                intrinsic, effect, d.notes
            )
        }
        RegInstr::CallClosure { .. } => {
            "contains a closure call (megamorphic / not native-inlinable)".to_string()
        }
        RegInstr::CallKnown { .. } | RegInstr::CallDynamic { .. } => {
            "contains a non-inlined call".to_string()
        }
        other => {
            // A non-call, non-subset instruction (heap construct, async, float-only
            // op the subset rejects, …). Name the opcode for the developer.
            let dbg = format!("{other:?}");
            let opcode = dbg.split([' ', '{']).next().unwrap_or("?");
            format!("contains {opcode} (outside native scalar/control subset)")
        }
    })
}

// --- Native-JIT host helpers ------------------------------------------------
//
// Heap values (structs/lists) can't live in the native tier's scalar registers,
// so the compiled code reads them by calling back into these helpers, passing an
// opaque handle (an index into a per-call table the VM fills in `try_native`).
// A read that can't be satisfied (wrong type / out of bounds) sets a bail flag;
// `try_native` checks it and re-runs the function on the interpreter, preserving
// the gap-free model. `rsscript` stays `#![forbid(unsafe_code)]`: defining these
// `extern "C"` functions and taking their addresses needs no `unsafe` — the only
// `unsafe` (the indirect call) lives in `vm-jit`.

#[cfg(feature = "native-jit")]
struct JitCallCtxState {
    active_depth: usize,
    active_token: vm_jit::HostCtx,
    next_token: vm_jit::HostCtx,
    heap_args: Vec<VmValue>,
    heap_results: Vec<VmValue>,
    heap_result_roots: Vec<Option<usize>>,
    heap_writebacks: Vec<(usize, i64)>,
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
    /// J0.5 limits cell: `[steps, step_budget, cancel_addr]`, read/written in place by
    /// an armed OSR native variant through a raw pointer (Exec-Spec §6.2). `step_budget`
    /// is `-1` when unarmed; `cancel_addr` is `0` when unarmed or the address of the
    /// host `AtomicBool` otherwise. The host seeds it before the call and reads `steps`
    /// back after, so one tick stream spans native and interpreter.
    static JIT_LIMITS_CELL: std::cell::Cell<[i64; 3]> = const { std::cell::Cell::new([0, -1, 0]) };
    /// J0.5 mem cell: `[allocated_bytes, allocation_budget]`. Unlike the step cell this is charged by
    /// the `ListPush*` HOST HELPER (the only native-subset op the interpreter bills to
    /// `allocation_budget`), not by generated code. `allocation_budget` is `-1` when unarmed (the helper
    /// then charges nothing). The host seeds it before a native call and, on a CLEAN OSR
    /// exit, reads `allocated_bytes` back to commit the charges; on a bail the OSR rolls back
    /// the list writes and reruns on the interpreter, which recharges authoritatively, so
    /// the native charges are simply discarded (exact `account_bytes` parity).
    static JIT_MEM_CELL: std::cell::Cell<[i64; 2]> = const { std::cell::Cell::new([0, -1]) };
}

/// Seed the J0.5 limits cell before an armed OSR native call. `steps` is the current
/// interpreter step count, `step_budget` is the budget (or `-1`), `cancel_addr` is the
/// `AtomicBool` address (or `0`).
#[cfg(feature = "native-jit")]
fn jit_set_limits_cell(steps: i64, step_budget: i64, cancel_addr: i64) {
    JIT_LIMITS_CELL.with(|cell| cell.set([steps, step_budget, cancel_addr]));
}

/// Read the accumulated step count back out of the J0.5 limits cell after an armed OSR
/// native call (clean completion or deopt both write it back).
#[cfg(feature = "native-jit")]
fn jit_limits_cell_steps() -> i64 {
    JIT_LIMITS_CELL.with(|cell| cell.get()[0])
}

/// Seed the J0.5 mem cell before a native call. `allocated_bytes` is the interpreter's current
/// accounted cumulative allocation total; `allocation_budget` is the budget (or `-1` to disarm — every non-mem-armed
/// native call MUST seed `-1` so a stale armed budget never leaks into the `ListPush*`
/// helper).
#[cfg(feature = "native-jit")]
fn jit_set_mem_cell(allocated_bytes: i64, allocation_budget: i64) {
    JIT_MEM_CELL.with(|cell| cell.set([allocated_bytes, allocation_budget]));
}

/// Read the accumulated live-byte count back out of the mem cell after a CLEAN OSR exit
/// (to commit the native `ListPush*` charges into the interpreter's `allocated_bytes`).
#[cfg(feature = "native-jit")]
fn jit_allocation_cell_allocated_bytes() -> i64 {
    JIT_MEM_CELL.with(|cell| cell.get()[0])
}

/// Charge `grew` bytes (a `ListPush*` flat-capacity growth) against the armed mem cell,
/// mirroring the interpreter's `account_bytes`. Returns `false` if the budget is now
/// exceeded — the caller signals a bail, the OSR rolls back + reruns on the interpreter,
/// which recharges and errors at the exact push. Unarmed (`allocation_budget < 0`) ⇒ no charge.
#[cfg(feature = "native-jit")]
fn jit_mem_charge(grew: i64) -> bool {
    JIT_MEM_CELL.with(|cell| {
        let [live, budget] = cell.get();
        if budget < 0 {
            return true;
        }
        let live = live.saturating_add(grew);
        cell.set([live, budget]);
        live <= budget
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
impl JitCallCtx {
    fn enter_frame() {
        JIT_CALL_CTX.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            if ctx.active_depth == 0 {
                ctx.reset_inputs();
                ctx.clear_results();
                ctx.clear_writebacks();
                ctx.active_token = ctx.allocate_token();
            }
            ctx.active_depth = ctx.active_depth.saturating_add(1);
        });
        jit_clear_heap_handle_caches();
    }

    fn exit_frame() -> bool {
        let became_inactive = JIT_CALL_CTX.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            debug_assert!(
                ctx.active_depth > 0,
                "native call context exited without an active frame"
            );
            if ctx.active_depth > 0 {
                ctx.active_depth -= 1;
            }
            if ctx.active_depth == 0 {
                ctx.reset_inputs();
                ctx.clear_results();
                ctx.clear_writebacks();
                ctx.active_token = 0;
                true
            } else {
                false
            }
        });
        if became_inactive {
            jit_clear_heap_write_undo();
            jit_clear_heap_handle_caches();
        }
        became_inactive
    }

    fn is_active() -> bool {
        JIT_CALL_CTX.with(|ctx| ctx.borrow().active_depth > 0)
    }

    fn active_token() -> vm_jit::HostCtx {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth > 0 {
                ctx.active_token
            } else {
                0
            }
        })
    }

    fn token_is_active(token: vm_jit::HostCtx) -> bool {
        token != 0
            && JIT_CALL_CTX.with(|ctx| {
                let ctx = ctx.borrow();
                ctx.active_depth > 0 && ctx.active_token == token
            })
    }

    fn push_heap_arg(value: VmValue) -> usize {
        JIT_CALL_CTX.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            assert!(
                ctx.active_depth > 0,
                "native heap arg registered outside an active native call context",
            );
            ctx.heap_args.push(value);
            ctx.heap_args.len() - 1
        })
    }

    fn with_heap_arg<R>(index: usize, read: impl FnOnce(&VmValue) -> Option<R>) -> Option<R> {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_args.get(index).and_then(read)
        })
    }

    fn clone_heap_arg(index: usize) -> Option<VmValue> {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_args.get(index).cloned()
        })
    }

    fn clear_heap_results() {
        JIT_CALL_CTX.with(|ctx| ctx.borrow_mut().clear_results());
    }

    fn push_heap_result(value: VmValue, root: Option<usize>) -> Option<i64> {
        JIT_CALL_CTX.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_results.push(value);
            match JitHeapHandle::encode_output(ctx.heap_results.len() - 1) {
                Some(index) => {
                    ctx.heap_result_roots.push(root);
                    Some(index)
                }
                None => {
                    ctx.heap_results.pop();
                    None
                }
            }
        })
    }

    fn clone_heap_result(index: usize) -> Option<VmValue> {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_results.get(index).cloned()
        })
    }

    fn heap_result_root(index: usize) -> Option<usize> {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_result_roots.get(index).copied().flatten()
        })
    }

    fn heap_results_empty() -> bool {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            ctx.heap_results.is_empty() && ctx.heap_result_roots.is_empty()
        })
    }

    fn clear_heap_writebacks() {
        JIT_CALL_CTX.with(|ctx| ctx.borrow_mut().clear_writebacks());
    }

    fn push_heap_writeback(root: usize, handle: i64) {
        JIT_CALL_CTX.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            if ctx.active_depth > 0 {
                ctx.heap_writebacks.push((root, handle));
            }
        });
    }

    fn heap_writebacks_empty() -> bool {
        JIT_CALL_CTX.with(|ctx| ctx.borrow().heap_writebacks.is_empty())
    }

    fn with_heap_writebacks<R>(read: impl FnOnce(&[(usize, i64)]) -> R) -> R {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return read(&[]);
            }
            read(ctx.heap_writebacks.as_slice())
        })
    }
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy)]
struct JitHostCallCtx;

#[cfg(feature = "native-jit")]
impl JitHostCallCtx {
    fn active() -> Option<Self> {
        JitCallCtx::is_active().then_some(Self)
    }

    fn from_token(token: vm_jit::HostCtx) -> Option<Self> {
        JitCallCtx::token_is_active(token).then_some(Self)
    }

    fn push_heap_arg(self, value: VmValue) -> usize {
        JitCallCtx::push_heap_arg(value)
    }

    fn with_heap_arg<R>(self, index: usize, read: impl FnOnce(&VmValue) -> Option<R>) -> Option<R> {
        JitCallCtx::with_heap_arg(index, read)
    }

    fn clone_heap_arg(self, index: usize) -> Option<VmValue> {
        JitCallCtx::clone_heap_arg(index)
    }

    fn push_heap_result(self, value: VmValue, root: Option<usize>) -> Option<i64> {
        JitCallCtx::push_heap_result(value, root)
    }

    fn publish_heap_result(self, value: VmValue) -> i64 {
        jit_push_heap_result_with_root_with_ctx(self, value, None)
    }

    fn publish_heap_handle(self, value: Option<VmValue>) -> i64 {
        match value {
            Some(value) => self.push_heap_arg(value) as i64,
            None => {
                vm_jit::signal_bail();
                0
            }
        }
    }

    fn clone_heap_result(self, index: usize) -> Option<VmValue> {
        JitCallCtx::clone_heap_result(index)
    }

    fn heap_result_root(self, index: usize) -> Option<usize> {
        JitCallCtx::heap_result_root(index)
    }

    fn push_heap_writeback(self, root: usize, handle: i64) {
        JitCallCtx::push_heap_writeback(root, handle);
    }

    fn with_heap_writebacks<R>(self, read: impl FnOnce(&[(usize, i64)]) -> R) -> R {
        JitCallCtx::with_heap_writebacks(read)
    }

    fn heap_read<R>(self, handle: i64, read: impl FnOnce(&VmValue) -> Option<R>) -> Option<R> {
        let index = usize::try_from(handle).ok()?;
        self.with_heap_arg(index, read)
    }

    fn heap_read_handle<R>(
        self,
        handle: i64,
        read: impl FnOnce(&VmValue) -> Option<R>,
    ) -> Option<R> {
        let value = jit_cached_heap_value_with_ctx(self, handle)?;
        read(&value)
    }

    fn heap_list_handle(self, handle: i64) -> Option<Rc<RefCell<TypedVec>>> {
        jit_heap_list_handle_with_ctx(self, handle)
    }

    fn heap_map_handle(self, handle: i64) -> Option<Rc<RefCell<ValueMap>>> {
        jit_heap_map_handle_with_ctx(self, handle)
    }

    fn heap_deque_handle(self, handle: i64) -> Option<Rc<RefCell<VecDeque<VmValue>>>> {
        jit_heap_deque_handle_with_ctx(self, handle)
    }

    fn with_journaled_list_write<R>(
        self,
        handle: i64,
        write: impl FnOnce(&mut TypedVec) -> Option<R>,
    ) -> Option<R> {
        jit_with_journaled_list_write_with_ctx(self, handle, write)
    }

    fn with_journaled_map_write<R>(
        self,
        handle: i64,
        write: impl FnOnce(&mut ValueMap) -> Option<R>,
    ) -> Option<R> {
        jit_with_journaled_map_write_with_ctx(self, handle, write)
    }

    fn with_journaled_deque_write<R>(
        self,
        handle: i64,
        write: impl FnOnce(&mut VecDeque<VmValue>) -> Option<R>,
    ) -> Option<R> {
        jit_with_journaled_deque_write_with_ctx(self, handle, write)
    }
}

#[cfg(feature = "native-jit")]
struct JitCallCtxGuard;

#[cfg(feature = "native-jit")]
impl JitCallCtxGuard {
    fn enter() -> Self {
        JitCallCtx::enter_frame();
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
    fn begin() -> Self {
        let ctx = JitCallCtxGuard::enter();
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
    #[allow(dead_code)]
    fn begin() -> Self {
        let owns_ctx_frame = !JitCallCtx::is_active();
        if owns_ctx_frame {
            JitCallCtx::enter_frame();
        }
        JitCallCtx::clear_heap_results();
        JitCallCtx::clear_heap_writebacks();
        jit_clear_heap_write_undo();
        jit_clear_heap_handle_caches();
        Self {
            finished: false,
            owns_ctx_frame,
        }
    }

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
        && map.sites[(site - 1) as usize].resume_ip as usize >= jit_fn.code.len()
    {
        return Err(format!(
            "forced safepoint {site} resumes outside translated code"
        ));
    }

    let mut saw_required_resume = required_resume_ip.is_none();
    for (site_index, site) in map.sites.iter().enumerate() {
        let resume_ip = site.resume_ip as usize;
        if resume_ip >= jit_fn.code.len() {
            return Err(format!(
                "deopt site {} resumes at {}, outside translated code len {}",
                site_index + 1,
                resume_ip,
                jit_fn.code.len()
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
    trans_exit: usize,
) -> Result<(), String> {
    jit_verify_deopt_map(module, id, jit_fn, None, Some(trans_exit))
}

#[cfg(feature = "native-jit")]
fn jit_native_verify_is_strict() -> bool {
    std::env::var_os("RSS_JIT_VERIFY").is_some()
}

#[cfg(feature = "native-jit")]
fn jit_native_deopt_every_from_env_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

#[cfg(feature = "native-jit")]
fn jit_native_deopt_every_from_env() -> bool {
    jit_native_deopt_every_from_env_value(std::env::var("RSS_JIT_DEOPT_EVERY").ok().as_deref())
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JitHeapHandle {
    Input(usize),
    Output(usize),
}

#[cfg(feature = "native-jit")]
impl JitHeapHandle {
    fn encode_output(index: usize) -> Option<i64> {
        let index = i64::try_from(index).ok()?;
        index.checked_add(1)?.checked_neg()
    }

    fn decode(bits: i64) -> Option<Self> {
        if bits >= 0 {
            return usize::try_from(bits).ok().map(JitHeapHandle::Input);
        }
        let index = bits.checked_add(1)?.checked_neg()?;
        usize::try_from(index).ok().map(JitHeapHandle::Output)
    }
}

#[cfg(feature = "native-jit")]
fn jit_cached_heap_value_with_ctx(ctx: JitHostCallCtx, handle: i64) -> Option<VmValue> {
    if let Some(value) = JIT_HEAP_VALUE_CACHE.with(|cache| {
        cache
            .borrow()
            .iter()
            .find(|entry| entry.handle == handle)
            .map(|entry| entry.value.clone())
    }) {
        return Some(value);
    }

    let value = match JitHeapHandle::decode(handle)? {
        JitHeapHandle::Input(index) => ctx.clone_heap_arg(index),
        JitHeapHandle::Output(index) => ctx.clone_heap_result(index),
    }?;

    JIT_HEAP_VALUE_CACHE.with(|cache| {
        const CACHE_LIMIT: usize = 4;
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_LIMIT {
            cache.remove(0);
        }
        cache.push(JitHeapValueCache {
            handle,
            value: value.clone(),
        });
    });
    Some(value)
}

#[cfg(feature = "native-jit")]
fn jit_materialize_heap_result(handle: i64) -> Option<VmValue> {
    match JitHeapHandle::decode(handle)? {
        JitHeapHandle::Input(index) => JitHostCallCtx::active()?.clone_heap_arg(index),
        JitHeapHandle::Output(index) => JitHostCallCtx::active()?.clone_heap_result(index),
    }
}

#[cfg(feature = "native-jit")]
fn jit_heap_result_root_with_ctx(ctx: JitHostCallCtx, handle: i64) -> Option<usize> {
    match JitHeapHandle::decode(handle)? {
        JitHeapHandle::Input(index) => Some(index),
        JitHeapHandle::Output(index) => ctx.heap_result_root(index),
    }
}

#[cfg(feature = "native-jit")]
fn jit_heap_handle_needs_write_undo(handle: i64) -> bool {
    matches!(JitHeapHandle::decode(handle), Some(JitHeapHandle::Input(_)))
}

#[cfg(feature = "native-jit")]
fn jit_materialize_heap_writebacks(
    input_slots: &[(usize, usize)],
) -> Option<Vec<(usize, VmValue)>> {
    JitHostCallCtx::active()?.with_heap_writebacks(|writebacks| {
        let mut materialized = Vec::new();
        for (input, slot) in input_slots {
            if let Some((_, handle)) = writebacks
                .iter()
                .rev()
                .find(|(updated_input, _)| updated_input == input)
            {
                materialized.push((*slot, jit_materialize_heap_result(*handle)?));
            }
        }
        Some(materialized)
    })
}

#[cfg(feature = "native-jit")]
fn jit_heap_list_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
) -> Option<Rc<RefCell<TypedVec>>> {
    if let Some(cached) = JIT_LIST_HANDLE_CACHE.with(|cache| {
        let cache = cache.borrow();
        cache
            .as_ref()
            .and_then(|cached| (cached.handle == handle).then(|| Rc::clone(&cached.list)))
    }) {
        return Some(cached);
    }

    ctx.heap_read_handle(handle, |value| match value {
        VmValue::List(list) => Some(Rc::clone(list)),
        VmValue::Managed(inner) => match &*inner.borrow() {
            VmValue::List(list) => Some(Rc::clone(list)),
            _ => None,
        },
        _ => None,
    })
    .inspect(|list| {
        JIT_LIST_HANDLE_CACHE.with(|cache| {
            *cache.borrow_mut() = Some(JitListHandleCache {
                handle,
                list: Rc::clone(list),
            });
        });
    })
}

#[cfg(feature = "native-jit")]
fn jit_heap_map_handle_with_ctx(ctx: JitHostCallCtx, handle: i64) -> Option<Rc<RefCell<ValueMap>>> {
    if let Some(cached) = JIT_MAP_HANDLE_CACHE.with(|cache| {
        let cache = cache.borrow();
        cache
            .as_ref()
            .and_then(|cached| (cached.handle == handle).then(|| Rc::clone(&cached.map)))
    }) {
        return Some(cached);
    }

    ctx.heap_read_handle(handle, |value| match value {
        VmValue::Map(map) => Some(Rc::clone(map)),
        VmValue::Managed(inner) => match &*inner.borrow() {
            VmValue::Map(map) => Some(Rc::clone(map)),
            _ => None,
        },
        _ => None,
    })
    .inspect(|map| {
        JIT_MAP_HANDLE_CACHE.with(|cache| {
            *cache.borrow_mut() = Some(JitMapHandleCache {
                handle,
                map: Rc::clone(map),
            });
        });
    })
}

#[cfg(feature = "native-jit")]
fn jit_heap_deque_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
) -> Option<Rc<RefCell<VecDeque<VmValue>>>> {
    if handle >= 0 {
        if let Some(cached) = JIT_DEQUE_HANDLE_CACHE.with(|cache| {
            let cache = cache.borrow();
            cache
                .as_ref()
                .and_then(|cached| (cached.handle == handle).then(|| Rc::clone(&cached.deque)))
        }) {
            return Some(cached);
        }
    }
    ctx.heap_read_handle(handle, |value| match value {
        VmValue::Deque(deque) => Some(Rc::clone(deque)),
        VmValue::Managed(inner) => match &*inner.borrow() {
            VmValue::Deque(deque) => Some(Rc::clone(deque)),
            _ => None,
        },
        _ => None,
    })
    .inspect(|deque| {
        if handle >= 0 {
            JIT_DEQUE_HANDLE_CACHE.with(|cache| {
                *cache.borrow_mut() = Some(JitDequeHandleCache {
                    handle,
                    deque: Rc::clone(deque),
                });
            });
        }
    })
}

#[cfg(feature = "native-jit")]
fn jit_snapshot_list_before_write(handle: i64, list: &Rc<RefCell<TypedVec>>) -> bool {
    if !JitCallCtx::is_active() {
        return false;
    }
    if !jit_heap_handle_needs_write_undo(handle) {
        return true;
    }
    jit_snapshot_input_list_before_write(list)
}

#[cfg(feature = "native-jit")]
fn jit_snapshot_input_list_before_write(list: &Rc<RefCell<TypedVec>>) -> bool {
    if !JitCallCtx::is_active() {
        return false;
    }
    if !jit_mark_heap_snapshot(JitHeapSnapshotKey::List(Rc::as_ptr(list))) {
        return true;
    }
    JIT_HEAP_WRITE_UNDO.with(|undo| {
        undo.borrow_mut().push(JitHeapWriteUndo::List(
            Rc::clone(list),
            list.borrow().clone_preserving_capacity(),
        ));
    });
    true
}

#[cfg(feature = "native-jit")]
fn jit_struct_field_list(value: &VmValue, slot: usize) -> Option<Rc<RefCell<TypedVec>>> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => match data.fields.get(slot)? {
            VmValue::List(list) => Some(Rc::clone(list)),
            VmValue::Managed(inner) => match &*inner.borrow() {
                VmValue::List(list) => Some(Rc::clone(list)),
                _ => None,
            },
            _ => None,
        },
        VmValue::Managed(inner) => jit_struct_field_list(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_value_may_contain_list(value: &VmValue) -> bool {
    matches!(
        value,
        VmValue::List(_)
            | VmValue::Deque(_)
            | VmValue::Map(_)
            | VmValue::OptionSomeHeap(_)
            | VmValue::Struct(_)
            | VmValue::Variant(_)
            | VmValue::Managed(_)
            | VmValue::Closure(_)
    )
}

#[cfg(feature = "native-jit")]
fn jit_value_contains_list_rc(value: &VmValue, needle: &Rc<RefCell<TypedVec>>) -> bool {
    // `seen` tracks the pointer identity of EVERY interior-mutable container already
    // entered (`List`/`Deque`/`Map`/`Managed`), not just `Managed`. A heap graph can be
    // cyclic (e.g. a `List` that, through a `RefCell`, contains itself), so without
    // recording every container identity the recursion would loop forever / stack-overflow.
    // A revisit returns `false`: the needle is matched by `Rc::ptr_eq` on FIRST visit, so a
    // back-edge to an already-seen node cannot be (a fresh path to) the needle.
    fn contains(value: &VmValue, needle: &Rc<RefCell<TypedVec>>, seen: &mut Vec<usize>) -> bool {
        // Returns false if `ptr` was already visited; otherwise records it and returns true.
        fn first_visit(seen: &mut Vec<usize>, ptr: usize) -> bool {
            if seen.contains(&ptr) {
                return false;
            }
            seen.push(ptr);
            true
        }
        match value {
            VmValue::List(list) => {
                Rc::ptr_eq(list, needle)
                    || (first_visit(seen, Rc::as_ptr(list) as usize) && {
                        let borrowed = list.borrow();
                        borrowed.iter().any(|item| contains(&item, needle, seen))
                    })
            }
            VmValue::Deque(deque) => {
                first_visit(seen, Rc::as_ptr(deque) as usize)
                    && deque
                        .borrow()
                        .iter()
                        .any(|item| contains(item, needle, seen))
            }
            VmValue::Map(map) => {
                first_visit(seen, Rc::as_ptr(map) as usize)
                    && map.borrow().iter().any(|(key, value)| {
                        contains(key.value(), needle, seen) || contains(value, needle, seen)
                    })
            }
            VmValue::OptionSomeHeap(value) => contains(value, needle, seen),
            VmValue::Struct(data) | VmValue::Variant(data) => data
                .fields
                .iter()
                .any(|field| contains(field, needle, seen)),
            VmValue::Managed(inner) => {
                first_visit(seen, Rc::as_ptr(inner) as usize)
                    && contains(&inner.borrow(), needle, seen)
            }
            VmValue::Closure(closure) => closure
                .captures
                .iter()
                .any(|capture| contains(capture, needle, seen)),
            _ => false,
        }
    }

    if !jit_value_may_contain_list(value) {
        return false;
    }
    contains(value, needle, &mut Vec::new())
}

#[cfg(feature = "native-jit")]
fn jit_heap_inputs_alias_flat_mut(
    input_slots: &[(usize, usize)],
    flat_mut_owned: &[Rc<RefCell<TypedVec>>],
) -> bool {
    if flat_mut_owned.is_empty() || input_slots.is_empty() {
        return false;
    }
    let Some(ctx) = JitHostCallCtx::active() else {
        return false;
    };
    input_slots.iter().any(|(input, _)| {
        ctx.with_heap_arg(*input, |value| {
            if !jit_value_may_contain_list(value) {
                return Some(false);
            }
            Some(
                flat_mut_owned
                    .iter()
                    .any(|list| jit_value_contains_list_rc(value, list)),
            )
        })
        .unwrap_or(false)
    })
}

#[cfg(feature = "native-jit")]
fn jit_selected_heap_inputs_alias_flat_mut(
    input_slots: &[(usize, usize)],
    flat_mut_owned: &[Rc<RefCell<TypedVec>>],
    frame_base: usize,
    heap_input_regs: &[usize],
) -> bool {
    if flat_mut_owned.is_empty() || input_slots.is_empty() || heap_input_regs.is_empty() {
        return false;
    }
    let Some(ctx) = JitHostCallCtx::active() else {
        return false;
    };
    input_slots.iter().any(|(input, absolute_reg)| {
        let Some(reg) = absolute_reg.checked_sub(frame_base) else {
            return false;
        };
        if !heap_input_regs.contains(&reg) {
            return false;
        }
        ctx.with_heap_arg(*input, |value| {
            if !jit_value_may_contain_list(value) {
                return Some(false);
            }
            Some(
                flat_mut_owned
                    .iter()
                    .any(|list| jit_value_contains_list_rc(value, list)),
            )
        })
        .unwrap_or(false)
    })
}

#[cfg(feature = "native-jit")]
fn jit_snapshot_map_before_write(handle: i64, map: &Rc<RefCell<ValueMap>>) -> bool {
    if !JitCallCtx::is_active() {
        return false;
    }
    if !jit_heap_handle_needs_write_undo(handle) {
        return true;
    }
    if !jit_mark_heap_snapshot(JitHeapSnapshotKey::Map(Rc::as_ptr(map))) {
        return true;
    }
    JIT_HEAP_WRITE_UNDO.with(|undo| {
        undo.borrow_mut().push(JitHeapWriteUndo::Map(
            Rc::clone(map),
            clone_value_map_preserving_capacity(&map.borrow()),
        ));
    });
    true
}

#[cfg(feature = "native-jit")]
fn jit_snapshot_deque_before_write(handle: i64, deque: &Rc<RefCell<VecDeque<VmValue>>>) -> bool {
    if !JitCallCtx::is_active() {
        return false;
    }
    if !jit_heap_handle_needs_write_undo(handle) {
        return true;
    }
    if !jit_mark_heap_snapshot(JitHeapSnapshotKey::Deque(Rc::as_ptr(deque))) {
        return true;
    }
    JIT_HEAP_WRITE_UNDO.with(|undo| {
        undo.borrow_mut().push(JitHeapWriteUndo::Deque(
            Rc::clone(deque),
            deque.borrow().clone(),
        ));
    });
    true
}

#[cfg(feature = "native-jit")]
fn jit_with_journaled_list_write_with_ctx<R>(
    ctx: JitHostCallCtx,
    handle: i64,
    write: impl FnOnce(&mut TypedVec) -> Option<R>,
) -> Option<R> {
    let list = ctx.heap_list_handle(handle)?;
    if !jit_snapshot_list_before_write(handle, &list) {
        return None;
    }
    write(&mut list.borrow_mut())
}

#[cfg(feature = "native-jit")]
fn jit_with_journaled_map_write_with_ctx<R>(
    ctx: JitHostCallCtx,
    handle: i64,
    write: impl FnOnce(&mut ValueMap) -> Option<R>,
) -> Option<R> {
    let map = ctx.heap_map_handle(handle)?;
    if !jit_snapshot_map_before_write(handle, &map) {
        return None;
    }
    write(&mut map.borrow_mut())
}

#[cfg(feature = "native-jit")]
fn jit_with_journaled_deque_write_with_ctx<R>(
    ctx: JitHostCallCtx,
    handle: i64,
    write: impl FnOnce(&mut VecDeque<VmValue>) -> Option<R>,
) -> Option<R> {
    let deque = ctx.heap_deque_handle(handle)?;
    if !jit_snapshot_deque_before_write(handle, &deque) {
        return None;
    }
    write(&mut deque.borrow_mut())
}

#[cfg(feature = "native-jit")]
fn jit_restore_heap_writes() {
    JIT_HEAP_WRITE_UNDO.with(|undo| {
        for entry in undo.borrow_mut().drain(..).rev() {
            match entry {
                JitHeapWriteUndo::List(list, original) => {
                    *list.borrow_mut() = original;
                }
                JitHeapWriteUndo::Map(map, original) => {
                    *map.borrow_mut() = original;
                }
                JitHeapWriteUndo::Deque(deque, original) => {
                    *deque.borrow_mut() = original;
                }
            }
        }
    });
}

#[cfg(feature = "native-jit")]
fn jit_struct_field_int(value: &VmValue, slot: usize) -> Option<i64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => match data.fields.get(slot)? {
            VmValue::Int(v) => Some(*v),
            _ => None,
        },
        VmValue::Managed(inner) => jit_struct_field_int(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_struct_with_int_field_updates(value: &VmValue, updates: &[(usize, i64)]) -> Option<VmValue> {
    match value {
        VmValue::Struct(data) => {
            let mut fields = data.fields.clone();
            for (slot, updated) in updates {
                let field = fields.get_mut(*slot)?;
                if !matches!(field, VmValue::Int(_)) {
                    return None;
                }
                *field = VmValue::Int(*updated);
            }
            Some(VmValue::Struct(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        VmValue::Variant(data) => {
            let mut fields = data.fields.clone();
            for (slot, updated) in updates {
                let field = fields.get_mut(*slot)?;
                if !matches!(field, VmValue::Int(_)) {
                    return None;
                }
                *field = VmValue::Int(*updated);
            }
            Some(VmValue::Variant(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        VmValue::Managed(inner) => jit_struct_with_int_field_updates(&inner.borrow(), updates),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_struct_field_float(value: &VmValue, slot: usize) -> Option<f64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => match data.fields.get(slot)? {
            VmValue::Float(v) => Some(*v),
            _ => None,
        },
        VmValue::Managed(inner) => jit_struct_field_float(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_int(_ctx: vm_jit::HostCtx, handle: i64, slot: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match usize::try_from(slot)
        .ok()
        .and_then(|slot| _ctx.heap_read_handle(handle, |value| jit_struct_field_int(value, slot)))
    {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_set_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    slot: i64,
    value: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_field_set_int_with_ctx(_ctx, handle, slot, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_field_set_int_with_ctx(ctx: JitHostCallCtx, handle: i64, slot: i64, value: i64) -> i64 {
    let Some(slot) = usize::try_from(slot).ok() else {
        vm_jit::signal_bail();
        return 0;
    };
    let root = jit_heap_result_root_with_ctx(ctx, handle);
    let updated = ctx.heap_read_handle(handle, |heap| match heap {
        VmValue::Struct(data) => {
            let mut fields = data.fields.clone();
            let field = fields.get_mut(slot)?;
            if !matches!(field, VmValue::Int(_)) {
                return None;
            }
            *field = VmValue::Int(value);
            Some(VmValue::Struct(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        VmValue::Variant(data) => {
            let mut fields = data.fields.clone();
            let field = fields.get_mut(slot)?;
            if !matches!(field, VmValue::Int(_)) {
                return None;
            }
            *field = VmValue::Int(value);
            Some(VmValue::Variant(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        _ => None,
    });
    match updated {
        Some(value) => {
            let handle = jit_push_heap_result_with_root_with_ctx(ctx, value, root);
            if let Some(root) = root {
                ctx.push_heap_writeback(root, handle);
            }
            handle
        }
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// J0.4 #1 (heap-value struct write): set a struct/variant field to a **heap** value —
/// the heap analog of [`rss_jit_field_set_int`]. Resolves the value handle, then COW-
/// rebuilds the struct with the field replaced and publishes the new value (ReplacesInput
/// + writeback to the root). A scalar field at the slot is a shape mismatch ⇒ bail.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_set_handle(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    slot: i64,
    value_handle: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_field_set_handle_with_ctx(_ctx, handle, slot, value_handle)
}

#[cfg(feature = "native-jit")]
fn rss_jit_field_set_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    slot: i64,
    value_handle: i64,
) -> i64 {
    let Some(slot) = usize::try_from(slot).ok() else {
        vm_jit::signal_bail();
        return 0;
    };
    // Resolve the new heap field value before the COW write.
    let Some(new_value) = ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
        vm_jit::signal_bail();
        return 0;
    };
    let root = jit_heap_result_root_with_ctx(ctx, handle);
    let updated = ctx.heap_read_handle(handle, |heap| {
        let (mut fields, layout, is_variant) = match heap {
            VmValue::Struct(data) => (data.fields.clone(), Rc::clone(&data.layout), false),
            VmValue::Variant(data) => (data.fields.clone(), Rc::clone(&data.layout), true),
            _ => return None,
        };
        let field = fields.get_mut(slot)?;
        // A scalar field can never hold a heap value ⇒ shape mismatch ⇒ bail.
        if matches!(
            field,
            VmValue::Int(_) | VmValue::Float(_) | VmValue::Bool(_)
        ) {
            return None;
        }
        *field = new_value;
        let s = Rc::new(VmStruct::with_layout(layout, fields));
        Some(if is_variant {
            VmValue::Variant(s)
        } else {
            VmValue::Struct(s)
        })
    });
    match updated {
        Some(value) => {
            let handle = jit_push_heap_result_with_root_with_ctx(ctx, value, root);
            if let Some(root) = root {
                ctx.push_heap_writeback(root, handle);
            }
            handle
        }
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_set_float(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    slot: i64,
    value: f64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_field_set_float_with_ctx(_ctx, handle, slot, value)
}

/// Copy-on-write set of a `Float` struct/variant field — the write-side mirror of
/// `rss_jit_field_float`. A non-Float field (or out-of-range slot / wrong handle)
/// bails out-of-band, so a mis-typed lowering falls back to the interpreter rather
/// than corrupting the value.
#[cfg(feature = "native-jit")]
fn rss_jit_field_set_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    slot: i64,
    value: f64,
) -> i64 {
    let Some(slot) = usize::try_from(slot).ok() else {
        vm_jit::signal_bail();
        return 0;
    };
    let root = jit_heap_result_root_with_ctx(ctx, handle);
    let updated = ctx.heap_read_handle(handle, |heap| match heap {
        VmValue::Struct(data) => {
            let mut fields = data.fields.clone();
            let field = fields.get_mut(slot)?;
            if !matches!(field, VmValue::Float(_)) {
                return None;
            }
            *field = VmValue::Float(value);
            Some(VmValue::Struct(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        VmValue::Variant(data) => {
            let mut fields = data.fields.clone();
            let field = fields.get_mut(slot)?;
            if !matches!(field, VmValue::Float(_)) {
                return None;
            }
            *field = VmValue::Float(value);
            Some(VmValue::Variant(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        _ => None,
    });
    match updated {
        Some(value) => {
            let handle = jit_push_heap_result_with_root_with_ctx(ctx, value, root);
            if let Some(root) = root {
                ctx.push_heap_writeback(root, handle);
            }
            handle
        }
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
#[cfg(test)]
thread_local! {
    static JIT_COLLECTION_METADATA_HELPER_CALLS: RefCell<usize> = const { RefCell::new(0) };
}

#[cfg(all(feature = "native-jit", test))]
fn record_jit_collection_metadata_helper_call() {
    JIT_COLLECTION_METADATA_HELPER_CALLS.with(|calls| {
        *calls.borrow_mut() += 1;
    });
}

#[cfg(all(feature = "native-jit", test))]
#[allow(dead_code)]
fn reset_jit_collection_metadata_helper_calls() {
    JIT_COLLECTION_METADATA_HELPER_CALLS.with(|calls| {
        *calls.borrow_mut() = 0;
    });
}

#[cfg(all(feature = "native-jit", test))]
#[allow(dead_code)]
fn jit_collection_metadata_helper_calls() -> usize {
    JIT_COLLECTION_METADATA_HELPER_CALLS.with(|calls| *calls.borrow())
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    match i64::try_from(list.borrow().len()) {
        Ok(value) => value,
        Err(_) => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    i64::from(list.borrow().is_empty())
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_get_int(_ctx: vm_jit::HostCtx, handle: i64, index: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(index) = usize::try_from(index).ok() else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    let borrowed = list.borrow();
    match &*borrowed {
        TypedVec::Ints(values) => match values.get(index) {
            Some(value) => *value,
            None => {
                vm_jit::signal_bail();
                0
            }
        },
        TypedVec::Boxed(values) => match values.get(index) {
            Some(VmValue::Int(value)) => *value,
            Some(_) | None => {
                vm_jit::signal_bail();
                0
            }
        },
        TypedVec::Floats(_) => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_set_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    index: i64,
    value: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_list_set_int_with_ctx(_ctx, handle, index, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_list_set_int_with_ctx(ctx: JitHostCallCtx, handle: i64, index: i64, value: i64) -> i64 {
    let Some(index) = usize::try_from(index).ok() else {
        vm_jit::signal_bail();
        return 0;
    };
    match ctx.with_journaled_list_write(handle, |list| {
        if index >= list.len() {
            return None;
        }
        list.checked_set(index, VmValue::Int(value)).ok()?;
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_set_float(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    index: i64,
    value: f64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_list_set_float_with_ctx(_ctx, handle, index, value)
}

/// Set a `Float` list element — the write-side mirror of `rss_jit_list_get_float`.
/// A non-Float list / out-of-bounds index bails out-of-band.
#[cfg(feature = "native-jit")]
fn rss_jit_list_set_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    index: i64,
    value: f64,
) -> i64 {
    let Some(index) = usize::try_from(index).ok() else {
        vm_jit::signal_bail();
        return 0;
    };
    match ctx.with_journaled_list_write(handle, |list| {
        if index >= list.len() {
            return None;
        }
        list.checked_set(index, VmValue::Float(value)).ok()?;
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_push_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_list_push_int_with_ctx(_ctx, handle, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_list_push_int_with_ctx(ctx: JitHostCallCtx, handle: i64, value: i64) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        // `checked_push_accounted` returns the flat-capacity growth in bytes — exactly
        // what the interpreter's `List.push` bills to `allocation_budget` (`account_bytes`).
        list.checked_push_accounted(VmValue::Int(value)).ok()
    }) {
        Some(grew) => {
            if jit_mem_charge(grew as i64) {
                0
            } else {
                // Over budget: bail. The OSR rolls back this loop's list writes and
                // reruns on the interpreter, which recharges and errors at the exact push.
                vm_jit::signal_bail();
                0
            }
        }
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// J0.4 #1 (heap-value collection write): push a **heap** element onto a
/// `List<HeapType>` — the value side of item #1 (the key side is
/// [`rss_jit_map_insert_handle_key_int`]). The value handle is resolved to its heap
/// value (host-owned, input or output table) and appended via the journaled list write
/// (rolled back on a later bail, §7.2). A wrong-type/invalid handle bails.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_push_handle(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    value_handle: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_list_push_handle_with_ctx(_ctx, handle, value_handle)
}

#[cfg(feature = "native-jit")]
fn rss_jit_list_push_handle_with_ctx(ctx: JitHostCallCtx, handle: i64, value_handle: i64) -> i64 {
    // Resolve the heap value (clone it out of its table) before the journaled write.
    let Some(value) = ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
        vm_jit::signal_bail();
        return 0;
    };
    match ctx.with_journaled_list_write(handle, move |list| list.checked_push_accounted(value).ok())
    {
        Some(grew) => {
            if jit_mem_charge(grew as i64) {
                0
            } else {
                vm_jit::signal_bail();
                0
            }
        }
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_push_float(_ctx: vm_jit::HostCtx, handle: i64, value: f64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_list_push_float_with_ctx(_ctx, handle, value)
}

/// Push a `Float` onto a flat `List<Float>` — the write-side mirror of
/// `rss_jit_list_get_float`. A non-Float list / invalid handle bails out-of-band,
/// so a mis-typed lowering falls back to the interpreter.
#[cfg(feature = "native-jit")]
fn rss_jit_list_push_float_with_ctx(ctx: JitHostCallCtx, handle: i64, value: f64) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        list.checked_push_accounted(VmValue::Float(value)).ok()
    }) {
        Some(grew) => {
            if jit_mem_charge(grew as i64) {
                0
            } else {
                vm_jit::signal_bail();
                0
            }
        }
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_sort_int(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_list_sort_int_with_ctx(_ctx, handle)
}

#[cfg(feature = "native-jit")]
fn rss_jit_list_sort_int_with_ctx(ctx: JitHostCallCtx, handle: i64) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        let TypedVec::Ints(values) = list else {
            return None;
        };
        values.sort_unstable();
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_new_int(_ctx: vm_jit::HostCtx) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    _ctx.publish_heap_result(VmValue::List(Rc::new(RefCell::new(TypedVec::Ints(
        Vec::new(),
    )))))
}

#[cfg(feature = "native-jit")]
fn jit_int_key(value: i64) -> VmMapKey {
    VmMapKey::new(VmValue::Int(value))
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_insert_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key: i64,
    value: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_map_insert_int_with_ctx(_ctx, handle, key, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_map_insert_int_with_ctx(ctx: JitHostCallCtx, handle: i64, key: i64, value: i64) -> i64 {
    match ctx.with_journaled_map_write(handle, |map| {
        map.insert(jit_int_key(key), VmValue::Int(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// J0.4 #1 (heap-key collection write): insert an `Int` value under a **heap key**
/// (e.g. a `String`) — the non-`Int`-key analog of [`rss_jit_map_insert_int`]. The key
/// handle is resolved to its heap value and wrapped in `VmMapKey`, so hashing/equality
/// is the host's own canonical map-key semantics (never re-implemented in native). The
/// map write is journaled, so a later bail rolls it back (§7.2). A wrong container/key
/// shape signals a bail.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_insert_handle_key_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key_handle: i64,
    value: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_map_insert_handle_key_int_with_ctx(_ctx, handle, key_handle, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_map_insert_handle_key_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    key_handle: i64,
    value: i64,
) -> i64 {
    // Resolve the heap key to the host's canonical map key BEFORE the journaled write.
    let Some(key) = ctx.heap_read_handle(key_handle, |value| Some(VmMapKey::new(value.clone())))
    else {
        vm_jit::signal_bail();
        return 0;
    };
    match ctx.with_journaled_map_write(handle, |map| {
        map.insert(key, VmValue::Int(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_insert_float(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key: i64,
    value: f64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_map_insert_float_with_ctx(_ctx, handle, key, value)
}

/// Insert a `Float` value into an Int-keyed map — the value-side mirror of
/// `rss_jit_map_insert_int`. A bad handle bails out-of-band.
#[cfg(feature = "native-jit")]
fn rss_jit_map_insert_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    key: i64,
    value: f64,
) -> i64 {
    match ctx.with_journaled_map_write(handle, |map| {
        map.insert(jit_int_key(key), VmValue::Float(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_get_int(_ctx: vm_jit::HostCtx, handle: i64, key: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(map) = _ctx.heap_map_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    match map.borrow().get(&jit_int_key(key)) {
        Some(VmValue::Int(value)) => *value,
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_get_match_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key: i64,
    found: &mut i64,
) -> i64 {
    *found = 0;
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(map) = _ctx.heap_map_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    match map.borrow().get(&jit_int_key(key)) {
        Some(VmValue::Int(value)) => {
            *found = 1;
            *value
        }
        None => 0,
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// Float value-side mirror of `rss_jit_map_get_match_int`: the lookup is the
/// interpreter's own `map.get`; this only extracts the `Float` payload (f64 channel)
/// and writes the `found` output in the same host call. A non-Float payload bails
/// out-of-band.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_get_match_float(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key: i64,
    found: &mut i64,
) -> f64 {
    *found = 0;
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    let Some(map) = _ctx.heap_map_handle(handle) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    match map.borrow().get(&jit_int_key(key)) {
        Some(VmValue::Float(value)) => {
            *found = 1;
            *value
        }
        None => 0.0,
        _ => {
            vm_jit::signal_bail();
            0.0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_contains_int(_ctx: vm_jit::HostCtx, handle: i64, key: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(map) = _ctx.heap_map_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    i64::from(map.borrow().contains_key(&jit_int_key(key)))
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(map) = _ctx.heap_map_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    match i64::try_from(map.borrow().len()) {
        Ok(len) => len,
        Err(_) => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_map_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(map) = _ctx.heap_map_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    i64::from(map.borrow().is_empty())
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_set_insert_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_set_insert_int_with_ctx(_ctx, handle, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_set_insert_int_with_ctx(ctx: JitHostCallCtx, handle: i64, value: i64) -> i64 {
    match ctx.with_journaled_map_write(handle, |map| {
        Some(i64::from(
            map.insert(jit_int_key(value), VmValue::Unit).is_none(),
        ))
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// J0.4 #1 (heap-value collection write): insert a **heap** value (e.g. a `String`) into
/// a `Set<HeapType>`. The value handle is resolved to its heap value and wrapped in
/// `VmMapKey` — hashing/equality is the host's own canonical key, never re-implemented in
/// native (a set is a map with `Unit` values, like [`rss_jit_set_insert_int`]). The write
/// is journaled (§7.2 rollback). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_set_insert_handle(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    value_handle: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_set_insert_handle_with_ctx(_ctx, handle, value_handle)
}

#[cfg(feature = "native-jit")]
fn rss_jit_set_insert_handle_with_ctx(ctx: JitHostCallCtx, handle: i64, value_handle: i64) -> i64 {
    let Some(key) = ctx.heap_read_handle(value_handle, |value| Some(VmMapKey::new(value.clone())))
    else {
        vm_jit::signal_bail();
        return 0;
    };
    match ctx.with_journaled_map_write(handle, move |map| {
        Some(i64::from(map.insert(key, VmValue::Unit).is_none()))
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_set_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(map) = _ctx.heap_map_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    match i64::try_from(map.borrow().len()) {
        Ok(len) => len,
        Err(_) => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_set_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(map) = _ctx.heap_map_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    i64::from(map.borrow().is_empty())
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_set_insert_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_sorted_set_insert_int_with_ctx(_ctx, handle, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_sorted_set_insert_int_with_ctx(ctx: JitHostCallCtx, handle: i64, value: i64) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        sorted_insert_vm(list.as_boxed_mut(), VmValue::Int(value))
            .ok()
            .map(i64::from)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// J0.4 #1 (heap-value collection write): insert a **heap** value (e.g. `String`) into a
/// sorted set — the heap analog of [`rss_jit_sorted_set_insert_int`]. The value handle is
/// resolved and the host's own `sorted_insert_vm` (ordering + dedup) does the work; the
/// write is journaled (§7.2). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_set_insert_handle(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    value_handle: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(value) = _ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.with_journaled_list_write(handle, move |list| {
        sorted_insert_vm(list.as_boxed_mut(), value)
            .ok()
            .map(i64::from)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_set_contains_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    value: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.heap_read_handle(handle, |heap| match heap {
        VmValue::List(list) => Some(Rc::clone(list)),
        VmValue::Managed(inner) => match &*inner.borrow() {
            VmValue::List(list) => Some(Rc::clone(list)),
            _ => None,
        },
        _ => None,
    }) {
        Some(list) => match sorted_contains_vm(&list.borrow(), &VmValue::Int(value)) {
            Ok(found) => i64::from(found),
            Err(_) => {
                vm_jit::signal_bail();
                0
            }
        },
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_set_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    i64::from(list.borrow().is_empty())
}

#[cfg(feature = "native-jit")]
fn jit_sorted_map_entry_int(
    backing: &TypedVec,
    index: usize,
) -> Result<Option<(i64, i64)>, EvalError> {
    let Some(entry) = backing.get(index) else {
        return Ok(None);
    };
    let pair = expect_list_ref(&entry)?;
    let pair = pair.borrow();
    let entry_key = pair
        .first()
        .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing key.".to_string()))?;
    let entry_value = pair
        .get(1)
        .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing value.".to_string()))?;
    match (entry_key, entry_value) {
        (VmValue::Int(entry_key), VmValue::Int(entry_value)) => Ok(Some((entry_key, entry_value))),
        _ => Err(EvalError::Runtime(
            "reg VM SortedMap<Int, Int> native helper saw non-Int entry.".to_string(),
        )),
    }
}

#[cfg(feature = "native-jit")]
fn jit_sorted_map_find_int(
    backing: &TypedVec,
    key: i64,
) -> Result<Option<(usize, i64)>, EvalError> {
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let Some((entry_key, entry_value)) = jit_sorted_map_entry_int(backing, mid)? else {
            return Err(EvalError::Runtime(
                "reg VM SortedMap entry missing during native lookup.".to_string(),
            ));
        };
        match entry_key.cmp(&key) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(Some((mid, entry_value))),
        }
    }
    Ok(None)
}

/// Int-key / Float-value sorted-map entry at `index` — the value-side mirror of
/// `jit_sorted_map_entry_int` (key still Int, value Float).
#[cfg(feature = "native-jit")]
fn jit_sorted_map_entry_int_key_float(
    backing: &TypedVec,
    index: usize,
) -> Result<Option<(i64, f64)>, EvalError> {
    let Some(entry) = backing.get(index) else {
        return Ok(None);
    };
    let pair = expect_list_ref(&entry)?;
    let pair = pair.borrow();
    let entry_key = pair
        .first()
        .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing key.".to_string()))?;
    let entry_value = pair
        .get(1)
        .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing value.".to_string()))?;
    match (entry_key, entry_value) {
        (VmValue::Int(entry_key), VmValue::Float(entry_value)) => {
            Ok(Some((entry_key, entry_value)))
        }
        _ => Err(EvalError::Runtime(
            "reg VM SortedMap<Int, Float> native helper saw non-(Int,Float) entry.".to_string(),
        )),
    }
}

/// Binary search an Int-keyed sorted map for `key`, returning the Float value.
#[cfg(feature = "native-jit")]
fn jit_sorted_map_find_int_key_float(
    backing: &TypedVec,
    key: i64,
) -> Result<Option<f64>, EvalError> {
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let Some((entry_key, entry_value)) = jit_sorted_map_entry_int_key_float(backing, mid)?
        else {
            return Err(EvalError::Runtime(
                "reg VM SortedMap entry missing during native lookup.".to_string(),
            ));
        };
        match entry_key.cmp(&key) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(Some(entry_value)),
        }
    }
    Ok(None)
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_map_insert_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key: i64,
    value: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_sorted_map_insert_int_with_ctx(_ctx, handle, key, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_sorted_map_insert_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    key: i64,
    value: i64,
) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        sorted_map_insert_in_place(list.as_boxed_mut(), VmValue::Int(key), VmValue::Int(value))
            .ok()?;
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// J0.4 #1 (heap-key collection write): insert an `Int` value under a **heap** key (e.g.
/// `String`) into a sorted map — the heap-key analog of [`rss_jit_sorted_map_insert_int`].
/// The key handle is resolved and the host's own `sorted_map_insert_in_place` (ordering)
/// does the work; the write is journaled (§7.2). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_map_insert_handle_key_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key_handle: i64,
    value: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(key) = _ctx.heap_read_handle(key_handle, |key| Some(key.clone())) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.with_journaled_list_write(handle, move |list| {
        sorted_map_insert_in_place(list.as_boxed_mut(), key, VmValue::Int(value)).ok()?;
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_map_get_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key: i64,
    found: &mut i64,
) -> i64 {
    *found = 0;
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    let backing = list.borrow();
    if let Some(cached) = JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
        let cache_value = *cache.borrow();
        let cache_value = cache_value.filter(|cache| cache.handle == handle)?;
        match jit_sorted_map_entry_int(&backing, cache_value.next_index) {
            Ok(Some((entry_key, entry_value))) if entry_key == key => {
                cache.borrow_mut().replace(JitSortedMapScanCache {
                    handle,
                    next_index: cache_value.next_index.saturating_add(1),
                });
                Some(Ok(entry_value))
            }
            Ok(_) => None,
            Err(err) => Some(Err(err)),
        }
    }) {
        return match cached {
            Ok(value) => {
                *found = 1;
                value
            }
            Err(_) => {
                JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
                    cache.borrow_mut().take();
                });
                vm_jit::signal_bail();
                0
            }
        };
    }
    match jit_sorted_map_find_int(&backing, key) {
        Ok(Some((index, value))) => {
            *found = 1;
            JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
                cache.borrow_mut().replace(JitSortedMapScanCache {
                    handle,
                    next_index: index.saturating_add(1),
                });
            });
            value
        }
        Ok(None) => {
            JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
                cache.borrow_mut().take();
            });
            0
        }
        _ => {
            JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
                cache.borrow_mut().take();
            });
            vm_jit::signal_bail();
            0
        }
    }
}

/// Int-key / Float-value sorted-map get (mirror of `rss_jit_sorted_map_get_int`),
/// writing the same-call `found` output. Plain binary search — it omits the sequential
/// scan cache (a perf-only fast path), so the result is identical. A non-Float value
/// or wrong shape bails to the interpreter.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_map_get_float(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key: i64,
    found: &mut i64,
) -> f64 {
    *found = 0;
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    match jit_sorted_map_find_int_key_float(&list.borrow(), key) {
        Ok(Some(value)) => {
            *found = 1;
            value
        }
        Ok(None) => 0.0,
        Err(_) => {
            vm_jit::signal_bail();
            0.0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_map_contains_key_int(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    key: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    match jit_sorted_map_find_int(&list.borrow(), key) {
        Ok(found) => i64::from(found.is_some()),
        Err(_) => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_map_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    i64::from(list.borrow().is_empty())
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_sorted_map_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    match i64::try_from(list.borrow().len()) {
        Ok(len) => len,
        Err(_) => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(deque) = _ctx.heap_deque_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    match i64::try_from(deque.borrow().len()) {
        Ok(len) => len,
        Err(_) => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    #[cfg(test)]
    record_jit_collection_metadata_helper_call();
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(deque) = _ctx.heap_deque_handle(handle) else {
        vm_jit::signal_bail();
        return 0;
    };
    i64::from(deque.borrow().is_empty())
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_push_back_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_deque_push_back_int_with_ctx(_ctx, handle, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_deque_push_back_int_with_ctx(ctx: JitHostCallCtx, handle: i64, value: i64) -> i64 {
    match ctx.with_journaled_deque_write(handle, |deque| {
        deque.push_back(VmValue::Int(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// J0.4 #1 (heap-value collection write): push a **heap** value onto the back of a
/// `Deque<HeapType>` — the heap analog of [`rss_jit_deque_push_back_int`]. Resolves the
/// value handle; the write is journaled (§7.2). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_push_back_handle(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    value_handle: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(value) = _ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.with_journaled_deque_write(handle, move |deque| {
        deque.push_back(value);
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_push_back_float(_ctx: vm_jit::HostCtx, handle: i64, value: f64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_deque_push_back_float_with_ctx(_ctx, handle, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_deque_push_back_float_with_ctx(ctx: JitHostCallCtx, handle: i64, value: f64) -> i64 {
    match ctx.with_journaled_deque_write(handle, |deque| {
        deque.push_back(VmValue::Float(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_push_front_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_deque_push_front_int_with_ctx(_ctx, handle, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_deque_push_front_int_with_ctx(ctx: JitHostCallCtx, handle: i64, value: i64) -> i64 {
    match ctx.with_journaled_deque_write(handle, |deque| {
        deque.push_front(VmValue::Int(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// J0.4 #1 (heap-value collection write): push a **heap** value onto the front of a
/// `Deque<HeapType>` — the heap analog of [`rss_jit_deque_push_front_int`]. Resolves the
/// value handle; the write is journaled (§7.2). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_push_front_handle(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    value_handle: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let Some(value) = _ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.with_journaled_deque_write(handle, move |deque| {
        deque.push_front(value);
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_push_front_float(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    value: f64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    rss_jit_deque_push_front_float_with_ctx(_ctx, handle, value)
}

#[cfg(feature = "native-jit")]
fn rss_jit_deque_push_front_float_with_ctx(ctx: JitHostCallCtx, handle: i64, value: f64) -> i64 {
    match ctx.with_journaled_deque_write(handle, |deque| {
        deque.push_front(VmValue::Float(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
fn jit_deque_pop_int(
    ctx: JitHostCallCtx,
    handle: i64,
    pop: impl FnOnce(&mut VecDeque<VmValue>) -> Option<VmValue>,
) -> i64 {
    match ctx.with_journaled_deque_write(handle, pop) {
        Some(VmValue::Int(value)) => value,
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_pop_front_int(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    jit_deque_pop_int(_ctx, handle, VecDeque::pop_front)
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_pop_back_int(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    jit_deque_pop_int(_ctx, handle, VecDeque::pop_back)
}

/// Float value-side mirror of `jit_deque_pop_int`: pop a `Float`; an empty deque or
/// non-Float element bails (the interpreter then runs the `None` path).
#[cfg(feature = "native-jit")]
fn jit_deque_pop_float(
    ctx: JitHostCallCtx,
    handle: i64,
    pop: impl FnOnce(&mut VecDeque<VmValue>) -> Option<VmValue>,
) -> f64 {
    match ctx.with_journaled_deque_write(handle, pop) {
        Some(VmValue::Float(value)) => value,
        _ => {
            vm_jit::signal_bail();
            0.0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_pop_front_float(_ctx: vm_jit::HostCtx, handle: i64) -> f64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    jit_deque_pop_float(_ctx, handle, VecDeque::pop_front)
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_deque_pop_back_float(_ctx: vm_jit::HostCtx, handle: i64) -> f64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    jit_deque_pop_float(_ctx, handle, VecDeque::pop_back)
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_float(_ctx: vm_jit::HostCtx, handle: i64, slot: i64) -> f64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    match usize::try_from(slot)
        .ok()
        .and_then(|slot| _ctx.heap_read_handle(handle, |value| jit_struct_field_float(value, slot)))
    {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0.0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_get_float(_ctx: vm_jit::HostCtx, handle: i64, index: i64) -> f64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    let Some(index) = usize::try_from(index).ok() else {
        vm_jit::signal_bail();
        return 0.0;
    };
    let Some(list) = _ctx.heap_list_handle(handle) else {
        vm_jit::signal_bail();
        return 0.0;
    };
    let borrowed = list.borrow();
    match &*borrowed {
        TypedVec::Floats(values) => match values.get(index) {
            Some(value) => *value,
            None => {
                vm_jit::signal_bail();
                0.0
            }
        },
        TypedVec::Boxed(values) => match values.get(index) {
            Some(VmValue::Float(value)) => *value,
            Some(_) | None => {
                vm_jit::signal_bail();
                0.0
            }
        },
        TypedVec::Ints(_) => {
            vm_jit::signal_bail();
            0.0
        }
    }
}

/// The underlying function id of the closure behind `handle`, as `i64`. Used by the
/// J2 monomorphic-inlining guard ([`vm_jit::JitInstr::GuardClosureId`]). Total: a
/// non-closure / invalid handle, or a function id too large for `i64`, returns `-1`,
/// which never equals a real (`>= 0`) callee id, so the guard simply bails. Never
/// signals the out-of-band bail flag.
#[cfg(feature = "native-jit")]
fn jit_closure_function_id(value: &VmValue) -> Option<i64> {
    match value {
        VmValue::Closure(closure) => i64::try_from(closure.function).ok(),
        VmValue::Managed(inner) => jit_closure_function_id(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_closure_id(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        return -1;
    };
    _ctx.heap_read(handle, jit_closure_function_id)
        .unwrap_or(-1)
}

/// The scalar bits of capture `index` of the closure behind `handle`, as `i64` (an
/// `Int` directly, a `Float` reinterpreted via [`f64::to_bits`], a `Bool` as 0/1).
/// Used by the capturing-closure inline support
/// ([`vm_jit::HostHelper::ClosureCapture`]) to materialize a scalar capture into the
/// inlined callee body. A non-scalar (heap) capture, an out-of-range index, or a
/// non-closure handle signals the out-of-band bail flag — defensive, since the
/// producer only emits `ClosureCapture` for captures it proved scalar.
#[cfg(feature = "native-jit")]
fn jit_closure_capture_scalar(value: &VmValue, index: usize) -> Option<i64> {
    match value {
        VmValue::Closure(closure) => match closure.captures.get(index)? {
            VmValue::Int(v) => Some(*v),
            VmValue::Float(v) => Some(v.to_bits() as i64),
            VmValue::Bool(b) => Some(i64::from(*b)),
            _ => None,
        },
        VmValue::Managed(inner) => jit_closure_capture_scalar(&inner.borrow(), index),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_closure_capture(_ctx: vm_jit::HostCtx, handle: i64, index: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match usize::try_from(index)
        .ok()
        .and_then(|index| _ctx.heap_read(handle, |value| jit_closure_capture_scalar(value, index)))
    {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
fn jit_struct_field_closure_function_id(value: &VmValue, slot: usize) -> Option<i64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            jit_closure_function_id(data.fields.get(slot)?)
        }
        VmValue::Managed(inner) => jit_struct_field_closure_function_id(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_closure_id(_ctx: vm_jit::HostCtx, handle: i64, slot: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        return -1;
    };
    usize::try_from(slot)
        .ok()
        .and_then(|slot| {
            _ctx.heap_read(handle, |value| {
                jit_struct_field_closure_function_id(value, slot)
            })
        })
        .unwrap_or(-1)
}

#[cfg(feature = "native-jit")]
fn jit_struct_field_closure_capture_scalar(
    value: &VmValue,
    slot: usize,
    index: usize,
) -> Option<i64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            jit_closure_capture_scalar(data.fields.get(slot)?, index)
        }
        VmValue::Managed(inner) => {
            jit_struct_field_closure_capture_scalar(&inner.borrow(), slot, index)
        }
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_closure_capture(
    _ctx: vm_jit::HostCtx,
    handle: i64,
    slot: i64,
    index: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match usize::try_from(slot).ok().and_then(|slot| {
        usize::try_from(index).ok().and_then(|index| {
            _ctx.heap_read(handle, |value| {
                jit_struct_field_closure_capture_scalar(value, slot, index)
            })
        })
    }) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// A clone of the struct/variant field `slot` IF it is itself a heap value (a
/// stored closure, struct, variant, or list) — the only fields a `FieldHandle`
/// read is allowed to fetch as a fresh handle. A scalar/absent field returns
/// `None` (→ bail), so a misclassified slot never produces a bogus handle.
#[cfg(feature = "native-jit")]
fn jit_struct_field_heap_value(value: &VmValue, slot: usize) -> Option<VmValue> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            let field = data.fields.get(slot)?;
            jit_heap_value_clone(field)
        }
        VmValue::Managed(inner) => jit_struct_field_heap_value(&inner.borrow(), slot),
        _ => None,
    }
}

/// A clone of list element `index` IF it is itself a heap value (e.g. a struct
/// holding a stored closure). A scalar/absent element returns `None` (→ bail).
#[cfg(feature = "native-jit")]
fn jit_list_get_heap_value(value: &VmValue, index: i64) -> Option<VmValue> {
    match value {
        VmValue::List(list) => {
            let index = usize::try_from(index).ok()?;
            let elem = list.borrow().get(index)?;
            jit_heap_value_clone(&elem)
        }
        VmValue::Managed(inner) => jit_list_get_heap_value(&inner.borrow(), index),
        _ => None,
    }
}

/// `Some(clone)` only when `value` is a heap value the host helpers can read
/// through a handle (closure/struct/variant/list, transparently unwrapping a
/// `Managed` cell). A scalar is `None`: handles only ever name heap values.
#[cfg(feature = "native-jit")]
fn jit_heap_value_clone(value: &VmValue) -> Option<VmValue> {
    match value {
        VmValue::Closure(_)
        | VmValue::Struct(_)
        | VmValue::Variant(_)
        | VmValue::List(_)
        | VmValue::Managed(_) => Some(value.clone()),
        _ => None,
    }
}

/// Push a freshly-fetched heap value into the per-call handle table and return its
/// index, or signal the standard re-run-from-top bail (returning 0) when the field/
/// element was not a heap value the helper could fetch.
#[cfg(feature = "native-jit")]
fn jit_push_heap_result_with_root_with_ctx(
    ctx: JitHostCallCtx,
    value: VmValue,
    root: Option<usize>,
) -> i64 {
    match ctx.push_heap_result(value, root) {
        Some(handle) => handle,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_from_int(_ctx: vm_jit::HostCtx, value: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    _ctx.publish_heap_result(VmValue::String(Rc::new(value.to_string())))
}

#[cfg(feature = "native-jit")]
fn jit_string_len(value: &VmValue) -> Option<i64> {
    match value {
        VmValue::String(value) => i64::try_from(value.len()).ok(),
        VmValue::Managed(inner) => jit_string_len(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_string_clone(value: &VmValue) -> Option<Rc<String>> {
    match value {
        VmValue::String(value) => Some(Rc::clone(value)),
        VmValue::Managed(inner) => jit_string_clone(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.heap_read_handle(handle, jit_string_len) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_concat(_ctx: vm_jit::HostCtx, left: i64, right: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let left = _ctx.heap_read_handle(left, jit_string_clone);
    let right = _ctx.heap_read_handle(right, jit_string_clone);
    match (left, right) {
        (Some(left), Some(right)) => {
            let mut value = String::with_capacity(left.len() + right.len());
            value.push_str(&left);
            value.push_str(&right);
            _ctx.publish_heap_result(VmValue::string(value))
        }
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_slice(_ctx: vm_jit::HostCtx, value: i64, start: i64, len: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.heap_read_handle(value, jit_string_clone) {
        Some(value) => {
            _ctx.publish_heap_result(VmValue::string(string_slice_range(&value, start, len)))
        }
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_pad_left(
    _ctx: vm_jit::HostCtx,
    value: i64,
    width: i64,
    fill: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = _ctx.heap_read_handle(value, jit_string_clone);
    let fill = _ctx.heap_read_handle(fill, jit_string_clone);
    match (value, fill) {
        (Some(value), Some(fill)) => {
            _ctx.publish_heap_result(VmValue::string(string_pad(&value, width, &fill, true)))
        }
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_pad_left_len(
    _ctx: vm_jit::HostCtx,
    value: i64,
    width: i64,
    fill: i64,
) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = _ctx.heap_read_handle(value, jit_string_clone);
    let fill = _ctx.heap_read_handle(fill, jit_string_clone);
    match (value, fill) {
        (Some(value), Some(fill)) => match string_pad_len(&value, width, &fill) {
            Some(len) => len,
            None => {
                vm_jit::signal_bail();
                0
            }
        },
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_split(_ctx: vm_jit::HostCtx, value: i64, delimiter: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = _ctx.heap_read_handle(value, jit_string_clone);
    let delimiter = _ctx.heap_read_handle(delimiter, jit_string_clone);
    match (value, delimiter) {
        (Some(value), Some(delimiter)) => {
            let parts = value
                .split(delimiter.as_str())
                .map(VmValue::string)
                .collect::<Vec<_>>();
            _ctx.publish_heap_result(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                parts,
            )))))
        }
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_starts_with(_ctx: vm_jit::HostCtx, value: i64, prefix: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = _ctx.heap_read_handle(value, jit_string_clone);
    let prefix = _ctx.heap_read_handle(prefix, jit_string_clone);
    match (value, prefix) {
        (Some(value), Some(prefix)) => i64::from(value.starts_with(prefix.as_str())),
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_split_count(_ctx: vm_jit::HostCtx, value: i64, delimiter: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = _ctx.heap_read_handle(value, jit_string_clone);
    let delimiter = _ctx.heap_read_handle(delimiter, jit_string_clone);
    match (value, delimiter) {
        (Some(value), Some(delimiter)) => {
            match i64::try_from(value.split(delimiter.as_str()).count()) {
                Ok(count) => count,
                Err(_) => {
                    vm_jit::signal_bail();
                    0
                }
            }
        }
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_string_literal(_ctx: vm_jit::HostCtx, literal_id: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = usize::try_from(literal_id)
        .ok()
        .and_then(|index| JIT_STRING_LITERALS.with(|table| table.borrow().get(index).cloned()));
    match value {
        Some(value) => _ctx.publish_heap_result(VmValue::String(value)),
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
fn jit_bytes_len(value: &VmValue) -> Option<i64> {
    match value {
        VmValue::Bytes(value) => i64::try_from(value.len()).ok(),
        VmValue::Managed(inner) => jit_bytes_len(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_bytes_clone(value: &VmValue) -> Option<Rc<Vec<u8>>> {
    match value {
        VmValue::Bytes(value) => Some(Rc::clone(value)),
        VmValue::Managed(inner) => jit_bytes_clone(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_bytes_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.heap_read_handle(handle, jit_bytes_len) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_bytes_slice(_ctx: vm_jit::HostCtx, handle: i64, start: i64, len: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx.heap_read_handle(handle, jit_bytes_clone) {
        Some(value) => {
            _ctx.publish_heap_result(VmValue::Bytes(Rc::new(bytes_slice(&value, start, len))))
        }
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
fn jit_json_clone(value: &VmValue) -> Option<Rc<crate::serde_json::Value>> {
    match value {
        VmValue::Json(value) => Some(Rc::clone(value)),
        VmValue::Managed(inner) => jit_json_clone(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_json_parse(_ctx: vm_jit::HostCtx, text: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    match _ctx
        .heap_read_handle(text, jit_string_clone)
        .and_then(|text| crate::serde_json::from_str::<crate::serde_json::Value>(&text).ok())
    {
        Some(value) => _ctx.publish_heap_result(VmValue::Json(Rc::new(value))),
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_json_field(_ctx: vm_jit::HostCtx, value: i64, name: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = _ctx.heap_read_handle(value, jit_json_clone);
    let name = _ctx.heap_read_handle(name, jit_string_clone);
    match (value, name) {
        (Some(value), Some(name)) => match value.as_object().and_then(|obj| obj.get(name.as_str()))
        {
            Some(field) => _ctx.publish_heap_result(VmValue::Json(Rc::new(field.clone()))),
            None => {
                vm_jit::signal_bail();
                0
            }
        },
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_json_field_int(_ctx: vm_jit::HostCtx, value: i64, name: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = _ctx.heap_read_handle(value, jit_json_clone);
    let name = _ctx.heap_read_handle(name, jit_string_clone);
    match (value, name) {
        (Some(value), Some(name)) => match value
            .as_object()
            .and_then(|obj| obj.get(name.as_str()))
            .and_then(crate::serde_json::Value::as_i64)
        {
            Some(value) => value,
            None => {
                vm_jit::signal_bail();
                0
            }
        },
        _ => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_handle(_ctx: vm_jit::HostCtx, handle: i64, slot: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = usize::try_from(slot).ok().and_then(|slot| {
        _ctx.heap_read_handle(handle, |value| jit_struct_field_heap_value(value, slot))
    });
    _ctx.publish_heap_handle(value)
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_get_handle(_ctx: vm_jit::HostCtx, handle: i64, index: i64) -> i64 {
    let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
        vm_jit::signal_bail();
        return 0;
    };
    let value = _ctx.heap_read_handle(handle, |value| jit_list_get_heap_value(value, index));
    _ctx.publish_heap_handle(value)
}

#[cfg(feature = "native-jit")]
impl NativeState {
    fn new_with_plan(plan: &NativeExecutionPlan) -> Result<Self, EvalError> {
        Self::new_with_opt_and_forced_safepoint(
            plan.tier_up_threshold,
            plan.force_bail,
            plan.collect_stats,
            plan.baseline,
            plan.precise_deopt,
            plan.osr_enabled,
            plan.report,
            plan.forced_safepoint,
            plan.force_all_safepoints,
            plan.allow_recursive_calls,
            plan.admission,
        )
    }

    // Used by the in-crate `#[cfg(test)]` unit tests; the optimizing/baseline
    // production paths go through `new_with_opt`, so the lib-only build sees no
    // caller. Keep the back-compat 3-arg constructor without a dead-code warning.
    #[allow(dead_code)]
    fn new(
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
    ) -> Result<Self, EvalError> {
        Self::new_with_opt(
            tier_up_threshold,
            force_bail,
            collect_stats,
            false,
            false,
            false,
            false,
        )
    }

    /// Build the native state at a selectable optimization level. `baseline ==
    /// true` selects the Phase-2 path-B baseline tier (`opt_level="none"`):
    /// faster compiles, identical observable behavior (same IR, same host
    /// helpers, same deopt protocol — only the Cranelift opt flag differs). The
    /// compiled subset is unchanged (side-effect-free scalar + read-only heap),
    /// so the interpreter/`run_jit` deopt oracle stays valid verbatim.
    fn new_with_opt(
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        baseline: bool,
        precise_deopt: bool,
        osr_enabled: bool,
        report: bool,
    ) -> Result<Self, EvalError> {
        Self::new_with_opt_and_forced_safepoint(
            tier_up_threshold,
            force_bail,
            collect_stats,
            baseline,
            precise_deopt,
            osr_enabled,
            report,
            None,
            false,
            true,
            NativeAdmissionPolicy::from_environment(tier_up_threshold),
        )
    }

    fn new_with_opt_and_forced_safepoint(
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        baseline: bool,
        precise_deopt: bool,
        osr_enabled: bool,
        report: bool,
        forced_safepoint: Option<u32>,
        force_all_safepoints: bool,
        allow_recursive_calls: bool,
        admission_policy: NativeAdmissionPolicy,
    ) -> Result<Self, EvalError> {
        let max_code_bytes = admission_policy.max_code_bytes;
        let max_compile_millis = admission_policy.max_compile_millis;
        let optimize_work_threshold = admission_policy.optimize_work_threshold;
        // A zero threshold is the explicit eager/benchmark mode. Preserve the
        // pre-ladder contract by compiling its only tier at `speed`; compiling
        // baseline and optimized copies on every fresh evaluation doubles cold
        // compile cost and can make helper-heavy kernels run in baseline code.
        let eager_optimized = tier_up_threshold == 0 && !baseline;
        let executable_memory_budget = vm_jit::ExecutableMemoryBudget::new(max_code_bytes);
        let has_optimized_module = !baseline && !eager_optimized;
        let baseline_arena_bytes = if has_optimized_module {
            max_code_bytes / 2
        } else {
            max_code_bytes
        };
        let optimized_arena_bytes = max_code_bytes.saturating_sub(baseline_arena_bytes);
        Ok(Self {
            baseline_module: vm_jit::NativeModule::new_with_opt_and_memory_budget(
                jit_host_helpers(),
                !eager_optimized,
                executable_memory_budget.clone(),
                baseline_arena_bytes,
            )
            .map_err(|e| EvalError::Runtime(e.to_string()))?,
            optimized_module: if baseline || eager_optimized {
                None
            } else {
                Some(
                    vm_jit::NativeModule::new_with_opt_and_memory_budget(
                        jit_host_helpers(),
                        false,
                        executable_memory_budget.clone(),
                        optimized_arena_bytes,
                    )
                    .map_err(|e| EvalError::Runtime(e.to_string()))?,
                )
            },
            admission: NativeAdmissionBudget {
                max_code_bytes,
                max_compile_nanos: u128::from(max_compile_millis) * 1_000_000,
                admitted_code_bytes: 0,
                compile_nanos: 0,
                code_exhausted: false,
            },
            cache: HashMap::new(),
            optimized_cache: HashMap::new(),
            optimization_sources: HashMap::new(),
            counts: HashMap::new(),
            promotion_work: HashMap::new(),
            optimize_work_threshold,
            bail_counts: HashMap::new(),
            noamortize_counts: HashMap::new(),
            tier_up_threshold,
            force_bail,
            forced_safepoint,
            force_all_safepoints,
            allow_recursive_calls,
            stats: NativeStats::default(),
            collect_stats,
            precise_deopt,
            osr_enabled,
            osr_candidates: HashMap::new(),
            osr_triggers: HashMap::new(),
            osr_cache: HashMap::new(),
            optimized_osr_cache: HashMap::new(),
            osr_optimization_sources: HashMap::new(),
            osr_promotion_work: HashMap::new(),
            osr_bail_counts: HashMap::new(),
            self_recursive_native: HashMap::new(),
            mutual_recursive_native: HashMap::new(),
            scratch_args: Vec::new(),
            scratch_lens: Vec::new(),
            scratch_flat_owned: Vec::new(),
            scratch_flat_mut_owned: Vec::new(),
            scratch_heap_input_slots: Vec::new(),
            scratch_osr_window: Vec::new(),
            scratch_osr_lens: Vec::new(),
            scratch_osr_flat_owned: Vec::new(),
            scratch_osr_flat_mut_owned: Vec::new(),
            scratch_osr_flat_slots: Vec::new(),
            scratch_osr_flat_mut_slots: Vec::new(),
            scratch_osr_heap_input_slots: Vec::new(),
            report,
            report_native_ok: std::collections::HashSet::new(),
            report_osr_ok: std::collections::HashSet::new(),
            osr_dynamic_bail: false,
        })
    }

    /// Record a consecutive failure against one shape version. Reaching the
    /// threshold negative-caches only that version; invariant translation
    /// failures remain the sole owner of evaluation-local JIT native status.
    fn record_bail(&mut self, version_key: &NativeVersionKey) {
        let count = self.bail_counts.entry(version_key.clone()).or_insert(0);
        *count += 1;
        if self.collect_stats {
            self.stats.shape_bails += 1;
        }
        if *count >= NATIVE_BAIL_GIVEUP_THRESHOLD {
            self.cache.insert(version_key.clone(), None);
            self.optimized_cache.remove(version_key);
            self.optimization_sources.remove(version_key);
            self.bail_counts.remove(version_key);
        }
    }

    fn whole_shape_count(&self, function: usize) -> usize {
        self.cache
            .keys()
            .filter(|key| key.function == function)
            .count()
    }

    fn osr_shape_count(&self, region: RegionKey) -> usize {
        self.osr_cache
            .keys()
            .filter(|key| key.region == region)
            .count()
    }
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
