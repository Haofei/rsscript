//! Native (Cranelift) baseline JIT for the RSScript register VM's numeric /
//! boolean / control-flow core — the native tier of
//! `docs/spec/RSScript_Execution_Spec_v0.1.md` (§7; status in Appendix B).
//!
//! # What it compiles
//!
//! A [`JitFunction`] is a stable, versioned slice of the VM's bytecode: the subset
//! that operates on unboxed scalar registers — `i64` (integers, and booleans as
//! `0`/`1`) and `f64` (floats) — plus *side-effect-free* heap **reads** of `Int`
//! struct fields and list elements (via [`HostHelpers`]) and narrow VM-owned
//! transactional heap helper calls. It has no general heap mutation, no general
//! calls, no async, and no other side effects. The main `rsscript` crate
//! translates an eligible `RegFunction` into this IR; everything outside the subset
//! stays on the interpreter (per-function fallback). [`NativeModule::compile`]
//! re-validates the IR ([`validate`]) before codegen, so a malformed producer fails
//! as a clean [`JitError`] rather than panicking or miscompiling.
//!
//! # Why a separate crate
//!
//! `rsscript` is `#![forbid(unsafe_code)]`. Executing generated machine code and
//! transmuting a code pointer to a callable function require `unsafe`, so they
//! live here behind a **safe** API ([`NativeModule::call`]): the only `unsafe` is
//! the call through a pointer whose ABI this crate itself emitted, so the safety
//! invariant is locally verifiable.
//!
//! # Gap-freeness
//!
//! Integer arithmetic in RSScript is *checked* (overflow, divide/modulo by zero
//! are language-level runtime errors). Rather than reproduce those error paths in
//! native code, the generated function **bails** (returns "not completed") on any
//! such edge — overflow, division by zero, `i64::MIN / -1`, or an out-of-range
//! shift — and likewise on a heap read the helper can't satisfy (wrong type or out
//! of bounds, signalled via the bail flag). Float arithmetic never traps (it
//! mirrors the interpreter's `f64` semantics, NaN/±inf included), so it needs no
//! bail. Because the compiled subset is side-effect-free (reads only), the caller
//! can then simply re-run the function on the interpreter, which is the single
//! source of semantic truth. So the native tier can only ever be *faster*, never
//! different.

use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, Block, InstBuilder, MemFlags, StackSlotData, StackSlotKind, Value, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};

/// Opaque VM-owned native helper context. The JIT never interprets this value; it
/// just forwards it from [`NativeModule::call`] to every imported host helper.
pub type HostCtx = i64;

/// `(struct_handle, slot) -> i64`: the struct's `slot`-th field as an `Int`.
pub type FieldIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(struct_handle, slot, value) -> i64`: copy-on-write set of an `Int` field,
/// returning a VM-owned output-table handle for the updated struct/variant.
pub type FieldSetIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(list_handle) -> i64`: list length.
pub type ListLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(handle) -> i64`: return `1` when a collection is empty, else `0`.
pub type IsEmptyFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(list_handle, index) -> i64`: the list element at `index` as an `Int`.
pub type ListGetIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(list_handle, index, value) -> i64`: set an `Int` list element. Returns `0`
/// on success; a wrong-type/out-of-bounds write signals a bail out-of-band.
pub type ListSetIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(list_handle, value) -> i64`: push an `Int` list element. Returns `0` on
/// success; a wrong-type/invalid handle signals a bail out-of-band.
pub type ListPushIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(list_handle) -> i64`: sort a flat `List<Int>` in place. Returns `0` on
/// success; a wrong-type/invalid handle signals a bail out-of-band.
pub type ListSortIntFn = extern "C" fn(HostCtx, i64) -> i64;
/// `() -> i64`: allocate a fresh empty flat `List<Int>` and return its VM-owned
/// output-table handle. A failure signals a bail out-of-band.
pub type ListNewIntFn = extern "C" fn(HostCtx) -> i64;
/// `(struct_handle, slot) -> f64`: the struct's `slot`-th field as a `Float`.
/// A wrong-type/out-of-range field signals a bail out-of-band (the f64 return
/// channel needs no tagging — the bail flag is separate), so the returned value
/// is unused on failure.
pub type FieldFloatFn = extern "C" fn(HostCtx, i64, i64) -> f64;
/// `(list_handle, index) -> f64`: the list element at `index` as a `Float`.
/// Like [`FieldFloatFn`], a wrong-type/out-of-bounds element signals a bail.
pub type ListGetFloatFn = extern "C" fn(HostCtx, i64, i64) -> f64;
/// `(closure_handle) -> i64`: the closure's underlying function id (its stable
/// callee identity). Returns `-1` for a non-closure / invalid handle, which never
/// matches a real (`>= 0`) function id, so the identity guard then bails. Total —
/// it never sets the out-of-band bail flag.
pub type ClosureIdFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(closure_handle, index) -> i64`: the scalar bits of capture `index` of the
/// closure handle (an `Int` directly, a `Float` reinterpreted via `to_bits`, or a
/// `Bool` as 0/1). A non-scalar (heap) capture, an out-of-range index, or a
/// non-closure handle signals a bail **out-of-band** (the shared bail flag), so the
/// returned value is unused on failure — exactly like [`FieldIntFn`]. The producer
/// only emits this for captures it has already proven scalar, so the bail path is a
/// defensive backstop.
pub type ClosureCaptureFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(struct_handle, slot) -> i64`: read the closure function id directly from a
/// heap struct/variant field. Total like [`ClosureIdFn`]: invalid/non-closure
/// fields return `-1` and do not set the bail flag.
pub type FieldClosureIdFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(struct_handle, slot, index) -> i64`: read scalar capture `index` directly
/// from a closure stored in a heap struct/variant field. Failure signals bail.
pub type FieldClosureCaptureFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(struct_handle, slot) -> i64`: a fresh **handle** (heap-table index) for the
/// struct's `slot`-th field, when that field is itself a heap value (e.g. a stored
/// closure). The helper clones the nested value into the per-call heap table and
/// returns its index, so a subsequent closure read (`closure_id`/`closure_capture`)
/// can address it like any other handle. A wrong-type/out-of-range field (or a
/// non-heap field) signals a bail out-of-band — exactly like [`FieldIntFn`].
pub type FieldHandleFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(list_handle, index) -> i64`: a fresh **handle** for the list element at
/// `index`, when that element is a heap value (e.g. a struct holding a closure).
/// Clones the nested value into the per-call heap table and returns its index; an
/// out-of-bounds index or non-heap element signals a bail out-of-band.
pub type ListGetHandleFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(value) -> i64`: allocate a fresh String from an Int and return its
/// VM-owned output-table handle. A failure signals a bail out-of-band.
pub type StringFromIntFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(string_handle) -> i64`: string byte length. A wrong-type/invalid handle
/// signals a bail out-of-band.
pub type StringLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(left_handle, right_handle) -> i64`: allocate `left + right` and return its
/// VM-owned output-table handle. A wrong-type/invalid handle signals a bail.
pub type StringConcatFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(string_handle, start, len) -> i64`: allocate the sliced string and return
/// its VM-owned output-table handle. A wrong-type/invalid handle signals a bail.
pub type StringSliceFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(value_handle, width, fill_handle) -> i64`: allocate the padded string and
/// return its VM-owned output-table handle. A wrong-type/invalid handle signals a
/// bail.
pub type StringPadLeftFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(value_handle, width, fill_handle) -> i64`: return the byte length of
/// `String.pad_left(value, width, fill)` without materializing the padded string.
/// A wrong-type/invalid handle signals a bail.
pub type StringPadLeftLenFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(value_handle, delimiter_handle) -> i64`: allocate `value.split(delimiter)`
/// as a `List<String>` and return its VM-owned output-table handle. A
/// wrong-type/invalid handle signals a bail.
pub type StringSplitFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(value_handle, prefix_handle) -> i64`: return `1` when `value` starts with
/// `prefix`, else `0`. A wrong-type/invalid handle signals a bail.
pub type StringStartsWithFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(value_handle, delimiter_handle) -> i64`: return
/// `value.split(delimiter).count()` without materializing the intermediate list.
/// A wrong-type/invalid handle signals a bail.
pub type StringSplitCountFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(literal_id) -> i64`: allocate a string literal from the VM-installed
/// per-call literal table and return its VM-owned output-table handle.
pub type StringLiteralFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(text_handle) -> i64`: parse JSON text and return a VM-owned output-table
/// Json handle. A parse/type failure signals a bail out-of-band.
pub type JsonParseFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(json_handle, name_handle) -> i64`: read an object field and return a
/// VM-owned output-table Json handle. A missing/non-object field signals a bail.
pub type JsonFieldFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(json_handle, name_handle) -> i64`: read an object integer field. A
/// missing/non-integer field signals a bail.
pub type JsonFieldIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(bytes_handle) -> i64`: byte length. A wrong-type/invalid handle signals a
/// bail out-of-band.
pub type BytesLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(bytes_handle, start, len) -> i64`: allocate a sliced Bytes value and return
/// its VM-owned output-table handle. A wrong-type/invalid handle signals a bail.
pub type BytesSliceFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(map_handle, key) -> i64`: insert/update an Int-keyed, Int-valued map.
/// A wrong container/key/value shape signals a bail.
pub type MapInsertIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(map_handle, key) -> i64`: return an Int payload for an existing map key.
/// Missing keys or non-Int payloads signal a bail.
pub type MapGetIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(map_handle, key) -> i64`: return an Int payload for a map key, or 0 for a
/// missing key. Call `map_get_match_found` to distinguish missing from a real
/// zero payload. Wrong shape/non-Int payloads signal a bail.
pub type MapGetMatchIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `() -> i64`: return whether the previous map-get-match helper found its key.
pub type MapGetMatchFoundFn = extern "C" fn(HostCtx) -> i64;
/// `(map_handle, key) -> i64`: return `1` when the Int key exists, else `0`.
/// A wrong container/key shape signals a bail.
pub type MapContainsIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(handle) -> i64`: collection length. A wrong container shape signals a bail.
pub type CollectionLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(set_handle, value) -> i64`: insert an Int into a set, returning whether it
/// was newly inserted. A wrong container/value shape signals a bail.
pub type SetInsertIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(set_handle, value) -> i64`: insert an Int into a sorted set, returning whether
/// it was newly inserted. A wrong container/value shape signals a bail.
pub type SortedSetInsertIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(set_handle, value) -> i64`: return whether an Int exists in a sorted set.
/// A wrong container/value shape signals a bail.
pub type SortedSetContainsIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(map_handle, key, value) -> i64`: insert/update an Int-keyed, Int-valued
/// sorted map. A wrong container/key/value shape signals a bail.
pub type SortedMapInsertIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(map_handle, key) -> i64`: return an Int payload for an existing sorted-map
/// key, or 0 for a missing key. Call `sorted_map_get_found` to distinguish
/// missing from a real zero payload. Wrong shape/non-Int payloads signal a bail.
pub type SortedMapGetIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `() -> i64`: return whether the previous sorted-map get helper found its key.
pub type SortedMapGetFoundFn = extern "C" fn(HostCtx) -> i64;
/// `(map_handle, key) -> i64`: return `1` when the Int key exists, else `0`.
/// A wrong container/key shape signals a bail.
pub type SortedMapContainsKeyIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(map_handle) -> i64`: sorted map length. A wrong container shape signals a bail.
pub type SortedMapLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(deque_handle) -> i64`: deque length. A wrong container shape signals a bail.
pub type DequeLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(deque_handle, value) -> i64`: push an Int to the back of a deque.
pub type DequePushBackIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(deque_handle, value) -> i64`: push an Int to the front of a deque.
pub type DequePushFrontIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(deque_handle) -> i64`: pop an Int from the front of a deque. Empty or non-Int
/// payloads signal a bail; RSScript's interpreter then executes the `None` path.
pub type DequePopFrontIntFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(deque_handle) -> i64`: pop an Int from the back of a deque. Empty or non-Int
/// payloads signal a bail; RSScript's interpreter then executes the `None` path.
pub type DequePopBackIntFn = extern "C" fn(HostCtx, i64) -> i64;

/// Host helper functions the compiled code calls to read heap values (struct
/// fields, list elements) that don't fit in a scalar register. The `rsscript`
/// crate supplies these `extern "C"` functions; they look the value up in a
/// per-call table the VM populates and return it unboxed as `i64`, signalling any
/// type/bounds mismatch out-of-band (the VM checks and falls back). The native
/// code just calls and uses the result.
///
/// These are **typed** function pointers, not raw `*const u8`: a safe caller can
/// only supply a real `extern "C"` function with the matching signature, so the
/// raw-address-to-symbol conversion (which is the part with an actual safety
/// obligation) stays private to this crate. The conversion to the `*const u8`
/// that Cranelift's symbol table wants happens in [`NativeModule::new`].
#[derive(Clone, Copy)]
pub struct HostHelpers {
    pub field_int: FieldIntFn,
    pub field_set_int: FieldSetIntFn,
    pub list_len: ListLenFn,
    pub list_is_empty: IsEmptyFn,
    pub list_get_int: ListGetIntFn,
    pub list_set_int: ListSetIntFn,
    pub list_push_int: ListPushIntFn,
    pub list_sort_int: ListSortIntFn,
    pub list_new_int: ListNewIntFn,
    pub field_float: FieldFloatFn,
    pub list_get_float: ListGetFloatFn,
    pub closure_id: ClosureIdFn,
    pub closure_capture: ClosureCaptureFn,
    pub field_closure_id: FieldClosureIdFn,
    pub field_closure_capture: FieldClosureCaptureFn,
    pub field_handle: FieldHandleFn,
    pub list_get_handle: ListGetHandleFn,
    pub string_from_int: StringFromIntFn,
    pub string_len: StringLenFn,
    pub string_concat: StringConcatFn,
    pub string_slice: StringSliceFn,
    pub string_pad_left: StringPadLeftFn,
    pub string_pad_left_len: StringPadLeftLenFn,
    pub string_split: StringSplitFn,
    pub string_starts_with: StringStartsWithFn,
    pub string_split_count: StringSplitCountFn,
    pub string_literal: StringLiteralFn,
    pub json_parse: JsonParseFn,
    pub json_field: JsonFieldFn,
    pub json_field_int: JsonFieldIntFn,
    pub bytes_len: BytesLenFn,
    pub bytes_slice: BytesSliceFn,
    pub map_insert_int: MapInsertIntFn,
    pub map_get_int: MapGetIntFn,
    pub map_get_match_int: MapGetMatchIntFn,
    pub map_get_match_found: MapGetMatchFoundFn,
    pub map_contains_int: MapContainsIntFn,
    pub map_len: CollectionLenFn,
    pub map_is_empty: IsEmptyFn,
    pub set_insert_int: SetInsertIntFn,
    pub set_len: CollectionLenFn,
    pub set_is_empty: IsEmptyFn,
    pub sorted_set_insert_int: SortedSetInsertIntFn,
    pub sorted_set_contains_int: SortedSetContainsIntFn,
    pub sorted_set_is_empty: IsEmptyFn,
    pub sorted_map_insert_int: SortedMapInsertIntFn,
    pub sorted_map_get_int: SortedMapGetIntFn,
    pub sorted_map_get_found: SortedMapGetFoundFn,
    pub sorted_map_contains_key_int: SortedMapContainsKeyIntFn,
    pub sorted_map_is_empty: IsEmptyFn,
    pub sorted_map_len: SortedMapLenFn,
    pub deque_len: DequeLenFn,
    pub deque_is_empty: IsEmptyFn,
    pub deque_push_back_int: DequePushBackIntFn,
    pub deque_push_front_int: DequePushFrontIntFn,
    pub deque_pop_front_int: DequePopFrontIntFn,
    pub deque_pop_back_int: DequePopBackIntFn,
}

/// Version of the [`JitInstr`]/[`JitFunction`] IR this crate consumes. The
/// producer (`rsscript`) translates its private bytecode into this stable,
/// versioned surface, so the two crates are decoupled: a breaking IR change bumps
/// this and the producer is updated in lock-step.
pub const IR_VERSION: u32 = 21;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFailureMode {
    CannotFail,
    BailFlag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostHeapEffect {
    ReadOnly,
    ExtendsInputHandles,
    AllocatesResult,
    MutatesInput,
    ReplacesInput,
}

impl HostHeapEffect {
    pub fn requires_transaction(self) -> bool {
        !matches!(self, HostHeapEffect::ReadOnly)
    }

    pub fn writes_existing_heap(self) -> bool {
        matches!(
            self,
            HostHeapEffect::MutatesInput | HostHeapEffect::ReplacesInput
        )
    }

    pub fn extends_input_handles(self) -> bool {
        matches!(self, HostHeapEffect::ExtendsInputHandles)
    }

    pub fn produces_heap_result(self) -> bool {
        matches!(
            self,
            HostHeapEffect::AllocatesResult | HostHeapEffect::ReplacesInput
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct HostHelperSig {
    args: &'static [JitValueType],
    result: HostResult,
    failure: HostFailureMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostResult {
    Exact(JitValueType),
    IntOrFloatBits,
}

#[derive(Debug, Clone, Copy)]
struct HostHelperDescriptor {
    helper: HostHelper,
    symbol: &'static str,
    sig: HostHelperSig,
    heap_effect: HostHeapEffect,
}

macro_rules! host_heap_effect {
    () => {
        HostHeapEffect::ReadOnly
    };
    ($effect:expr) => {
        $effect
    };
}

macro_rules! host_helpers {
    ($(
        $helper:ident => {
            field: $field:ident,
            symbol: $symbol:literal,
            args: [$($arg:expr),* $(,)?],
            result: $result:expr,
            failure: $failure:expr,
            $(heap_effect: $heap_effect:expr,)?
        }
    ),+ $(,)?) => {
        /// Generic host helper a [`JitInstr::HostCall`] can invoke. Adding a helper
        /// is one row in `host_helpers!`; the enum, import symbol, ABI signature,
        /// address lookup, and declaration order are generated together.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum HostHelper {
            $($helper),+
        }

        impl HostHelpers {
            fn addr(self, helper: HostHelper) -> *const u8 {
                match helper {
                    $(HostHelper::$helper => self.$field as *const u8),+
                }
            }
        }

        impl HostHelper {
            const ALL: &'static [HostHelper] = &[
                $(HostHelper::$helper),+
            ];

            const DESCRIPTORS: &'static [HostHelperDescriptor] = &[
                $(HostHelperDescriptor {
                    helper: HostHelper::$helper,
                    symbol: $symbol,
                    sig: HostHelperSig {
                        args: &[$($arg),*],
                        result: $result,
                        failure: $failure,
                    },
                    heap_effect: host_heap_effect!($($heap_effect)?),
                }),+
            ];

            fn all() -> &'static [HostHelper] {
                Self::ALL
            }

            fn descriptor(self) -> &'static HostHelperDescriptor {
                Self::DESCRIPTORS
                    .iter()
                    .find(|descriptor| descriptor.helper == self)
                    .expect("host helper descriptor missing")
            }

            fn symbol(self) -> &'static str {
                self.descriptor().symbol
            }

            pub fn arg_types(self) -> &'static [JitValueType] {
                self.signature().args
            }

            pub fn result_type(self) -> Option<JitValueType> {
                match self.signature().result {
                    HostResult::Exact(ty) => Some(ty),
                    HostResult::IntOrFloatBits => None,
                }
            }

            pub fn heap_effect(self) -> HostHeapEffect {
                self.descriptor().heap_effect
            }

            fn signature(self) -> HostHelperSig {
                self.descriptor().sig
            }
        }
    };
}

host_helpers! {
    FieldInt => {
        field: field_int,
        symbol: "rss_jit_field_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    FieldSetInt => {
        field: field_set_int,
        symbol: "rss_jit_field_set_int",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::ReplacesInput,
    },
    ListLen => {
        field: list_len,
        symbol: "rss_jit_list_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    ListIsEmpty => {
        field: list_is_empty,
        symbol: "rss_jit_list_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    ListGetInt => {
        field: list_get_int,
        symbol: "rss_jit_list_get_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    ListSetInt => {
        field: list_set_int,
        symbol: "rss_jit_list_set_int",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    ListPushInt => {
        field: list_push_int,
        symbol: "rss_jit_list_push_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    ListSortInt => {
        field: list_sort_int,
        symbol: "rss_jit_list_sort_int",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    ListNewInt => {
        field: list_new_int,
        symbol: "rss_jit_list_new_int",
        args: [],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    FieldFloat => {
        field: field_float,
        symbol: "rss_jit_field_float",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Float),
        failure: HostFailureMode::BailFlag,
    },
    ListGetFloat => {
        field: list_get_float,
        symbol: "rss_jit_list_get_float",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Float),
        failure: HostFailureMode::BailFlag,
    },
    ClosureId => {
        field: closure_id,
        symbol: "rss_jit_closure_id",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::CannotFail,
    },
    ClosureCapture => {
        field: closure_capture,
        symbol: "rss_jit_closure_capture",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::IntOrFloatBits,
        failure: HostFailureMode::BailFlag,
    },
    FieldClosureId => {
        field: field_closure_id,
        symbol: "rss_jit_field_closure_id",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::CannotFail,
    },
    FieldClosureCapture => {
        field: field_closure_capture,
        symbol: "rss_jit_field_closure_capture",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Int],
        result: HostResult::IntOrFloatBits,
        failure: HostFailureMode::BailFlag,
    },
    FieldHandle => {
        field: field_handle,
        symbol: "rss_jit_field_handle",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::ExtendsInputHandles,
    },
    ListGetHandle => {
        field: list_get_handle,
        symbol: "rss_jit_list_get_handle",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::ExtendsInputHandles,
    },
    StringFromInt => {
        field: string_from_int,
        symbol: "rss_jit_string_from_int",
        args: [JitValueType::Int],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    StringLen => {
        field: string_len,
        symbol: "rss_jit_string_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    StringConcat => {
        field: string_concat,
        symbol: "rss_jit_string_concat",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    StringSlice => {
        field: string_slice,
        symbol: "rss_jit_string_slice",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    StringPadLeft => {
        field: string_pad_left,
        symbol: "rss_jit_string_pad_left",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    StringPadLeftLen => {
        field: string_pad_left_len,
        symbol: "rss_jit_string_pad_left_len",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    StringSplit => {
        field: string_split,
        symbol: "rss_jit_string_split",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    StringStartsWith => {
        field: string_starts_with,
        symbol: "rss_jit_string_starts_with",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    StringSplitCount => {
        field: string_split_count,
        symbol: "rss_jit_string_split_count",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    StringLiteral => {
        field: string_literal,
        symbol: "rss_jit_string_literal",
        args: [JitValueType::Int],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    JsonParse => {
        field: json_parse,
        symbol: "rss_jit_json_parse",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    JsonField => {
        field: json_field,
        symbol: "rss_jit_json_field",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    JsonFieldInt => {
        field: json_field_int,
        symbol: "rss_jit_json_field_int",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    BytesLen => {
        field: bytes_len,
        symbol: "rss_jit_bytes_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    BytesSlice => {
        field: bytes_slice,
        symbol: "rss_jit_bytes_slice",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::AllocatesResult,
    },
    MapInsertInt => {
        field: map_insert_int,
        symbol: "rss_jit_map_insert_int",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    MapGetInt => {
        field: map_get_int,
        symbol: "rss_jit_map_get_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    MapGetMatchInt => {
        field: map_get_match_int,
        symbol: "rss_jit_map_get_match_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    MapGetMatchFound => {
        field: map_get_match_found,
        symbol: "rss_jit_map_get_match_found",
        args: [],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::CannotFail,
    },
    MapContainsInt => {
        field: map_contains_int,
        symbol: "rss_jit_map_contains_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    MapLen => {
        field: map_len,
        symbol: "rss_jit_map_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    MapIsEmpty => {
        field: map_is_empty,
        symbol: "rss_jit_map_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    SetInsertInt => {
        field: set_insert_int,
        symbol: "rss_jit_set_insert_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    SetLen => {
        field: set_len,
        symbol: "rss_jit_set_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    SetIsEmpty => {
        field: set_is_empty,
        symbol: "rss_jit_set_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    SortedSetInsertInt => {
        field: sorted_set_insert_int,
        symbol: "rss_jit_sorted_set_insert_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    SortedSetContainsInt => {
        field: sorted_set_contains_int,
        symbol: "rss_jit_sorted_set_contains_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    SortedSetIsEmpty => {
        field: sorted_set_is_empty,
        symbol: "rss_jit_sorted_set_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    SortedMapInsertInt => {
        field: sorted_map_insert_int,
        symbol: "rss_jit_sorted_map_insert_int",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    SortedMapGetInt => {
        field: sorted_map_get_int,
        symbol: "rss_jit_sorted_map_get_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    SortedMapGetFound => {
        field: sorted_map_get_found,
        symbol: "rss_jit_sorted_map_get_found",
        args: [],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::CannotFail,
    },
    SortedMapContainsKeyInt => {
        field: sorted_map_contains_key_int,
        symbol: "rss_jit_sorted_map_contains_key_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    SortedMapIsEmpty => {
        field: sorted_map_is_empty,
        symbol: "rss_jit_sorted_map_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    SortedMapLen => {
        field: sorted_map_len,
        symbol: "rss_jit_sorted_map_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    DequeLen => {
        field: deque_len,
        symbol: "rss_jit_deque_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    DequeIsEmpty => {
        field: deque_is_empty,
        symbol: "rss_jit_deque_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
    },
    DequePushBackInt => {
        field: deque_push_back_int,
        symbol: "rss_jit_deque_push_back_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    DequePushFrontInt => {
        field: deque_push_front_int,
        symbol: "rss_jit_deque_push_front_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    DequePopFrontInt => {
        field: deque_pop_front_int,
        symbol: "rss_jit_deque_pop_front_int",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
    DequePopBackInt => {
        field: deque_pop_back_int,
        symbol: "rss_jit_deque_pop_back_int",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
    },
}

/// Argument to a generic host-helper call. Most helper operands are registers; field
/// slots and capture indices are compile-time immediates that still flow through the
/// same shared call/bail machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostArg {
    Reg(u32),
    ImmI64(i64),
}

/// Signed integer comparison (the four ordered comparisons; equality is its own
/// instruction so it can also apply to booleans).
#[derive(Debug, Clone, Copy)]
pub enum JitCompare {
    Lt,
    Le,
    Gt,
    Ge,
}

impl JitCompare {
    fn cc(self) -> IntCC {
        match self {
            JitCompare::Lt => IntCC::SignedLessThan,
            JitCompare::Le => IntCC::SignedLessThanOrEqual,
            JitCompare::Gt => IntCC::SignedGreaterThan,
            JitCompare::Ge => IntCC::SignedGreaterThanOrEqual,
        }
    }

    /// Ordered float comparison (NaN → false), matching Rust's `<`/`<=`/`>`/`>=`
    /// on `f64` (the interpreter's float comparison).
    fn fcc(self) -> FloatCC {
        match self {
            JitCompare::Lt => FloatCC::LessThan,
            JitCompare::Le => FloatCC::LessThanOrEqual,
            JitCompare::Gt => FloatCC::GreaterThan,
            JitCompare::Ge => FloatCC::GreaterThanOrEqual,
        }
    }
}

/// One instruction of the JIT IR. Registers are `u32` indices; jump `target`s are
/// indices into the function's instruction vector (matching the VM's bytecode, so
/// translation is 1:1 and target indices need no remapping).
#[derive(Debug, Clone)]
pub enum JitInstr {
    /// Placeholder that preserves 1:1 index alignment with the source bytecode
    /// (e.g. a deep-copy of an `Int`, which is a no-op on an unboxed register).
    Nop,
    LoadInt {
        dst: u32,
        value: i64,
    },
    LoadFloat {
        dst: u32,
        value: f64,
    },
    LoadBool {
        dst: u32,
        value: bool,
    },
    Move {
        dst: u32,
        src: u32,
    },
    Add {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Sub {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Mul {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Div {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Mod {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    /// `dst (Float) = src (Int) as f64` — a signed-int→f64 value-preserving
    /// conversion (`fcvt_from_sint`), mirroring the interpreter's `i as f64`
    /// (`Int.to_float`). Not a bitcast.
    IntToFloat {
        dst: u32,
        src: u32,
    },
    /// Generic i64-returning host helper call. Helpers use the shared out-of-band
    /// bail flag protocol: if the helper cannot satisfy the operation, native
    /// deopts and the interpreter re-runs the function. Helper signatures are
    /// validated by [`HostHelper::signature`].
    HostCall {
        helper: HostHelper,
        dst: u32,
        args: Vec<HostArg>,
    },
    /// A pure scalar host helper whose arguments are loop-invariant. The first
    /// executed visit calls the helper, stores its result in `cache`, and flips
    /// `flag` to true. Later visits copy `cache` into `dst`. This keeps the IR
    /// source-index aligned: the memoization expands inside codegen, so deopt
    /// resume IPs still point at the original helper instruction.
    MemoizedHostCall {
        helper: HostHelper,
        dst: u32,
        args: Vec<HostArg>,
        cache: u32,
        flag: u32,
    },
    /// Staged native-call ABI: call another function already compiled in the same
    /// [`NativeModule`]. Callee params/results may be scalar or heap `Handle`s
    /// carried in the shared host context; flat-array params stay top-level only.
    /// If a callee
    /// deopts, the caller deopts at this call site and chains the child
    /// safepoint/payload into its own deopt payload, preserving the native-frame
    /// stack for embedders that can consume it.
    CallNative {
        callee: CompiledId,
        dst: u32,
        args: Vec<u32>,
    },
    /// `Map.get` fused with an Option match for Int-keyed maps. The helper first
    /// tests membership; on `Some`, it loads the Int payload into `value_dst` and
    /// jumps to `some_ip`, otherwise it jumps to `none_ip`.
    MatchMapGetInt {
        map: u32,
        key: u32,
        value_dst: u32,
        some_ip: u32,
        none_ip: u32,
    },
    /// `SortedMap.get` fused with an Option match for Int-keyed sorted maps.
    MatchSortedMapGetInt {
        map: u32,
        key: u32,
        value_dst: u32,
        some_ip: u32,
        none_ip: u32,
    },
    BitAnd {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    BitOr {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    BitXor {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Shl {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Shr {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    /// `dst = (lhs <op> rhs) as 0/1`.
    Compare {
        dst: u32,
        op: JitCompare,
        lhs: u32,
        rhs: u32,
    },
    Equal {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    NotEqual {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Jump {
        target: u32,
    },
    JumpIfBool {
        cond: u32,
        expected: bool,
        target: u32,
    },
    /// Profile-guided conditional branch. The branch follows only the edge that
    /// was hot in interpreter feedback; the opposite edge deopts to the
    /// interpreter. `hot_target == true` means the explicit `target` edge is hot,
    /// otherwise the fallthrough edge is hot.
    ProfiledJumpIfBool {
        cond: u32,
        expected: bool,
        target: u32,
        hot_target: bool,
    },
    JumpIfIntCompare {
        lhs: u32,
        rhs: u32,
        op: JitCompare,
        expected: bool,
        target: u32,
    },
    /// Profile-guided integer/float compare branch. See
    /// [`JitInstr::ProfiledJumpIfBool`] for the hot/cold edge contract.
    ProfiledJumpIfIntCompare {
        lhs: u32,
        rhs: u32,
        op: JitCompare,
        expected: bool,
        target: u32,
        hot_target: bool,
    },
    Return {
        src: u32,
    },
    /// Unconditionally bail to the interpreter (e.g. a `RuntimeError` instruction:
    /// re-running on the interpreter reproduces the exact error).
    Bail,
    // Heap-reading host helpers (`field_int`, `list_len`, `list_get_*`,
    // `closure_capture`, handle fetches, and string helpers) are represented by
    // `HostCall` plus `HostHelper` metadata.
    /// TV2 direct read: `dst = base_ptr[index]` where `base` is a **`FlatInt`**
    /// param register holding the raw `*const i64` of a flat `List<Int>` buffer.
    /// Bounds-checked against the param's length (the `lens` slot for `base`); an
    /// out-of-bounds index branches to fallback exactly like the helper's bail —
    /// no per-element host call. `dst` is an `Int` register.
    ListGetIntDirect {
        dst: u32,
        base: u32,
        index: u32,
    },
    /// TV2 direct write: `base_ptr[index] = value` where `base` is a mutable
    /// flat `List<Int>` param. Bounds-checked against `lens[base]`; OOB bails and
    /// the VM-side heap transaction restores the list snapshot.
    ListSetIntDirect {
        dst: u32,
        base: u32,
        index: u32,
        value: u32,
    },
    /// TV2 direct read: `dst = base_ptr[index]` where `base` is a **`FlatFloat`**
    /// param register holding the raw `*const f64` of a flat `List<Float>` buffer.
    /// Bounds-checked against the param's length; OOB → fallback. `dst` is a
    /// `Float` register.
    ListGetFloatDirect {
        dst: u32,
        base: u32,
        index: u32,
    },
    /// TV2 direct read: `dst = len` of the flat-array param `base` (a `FlatInt` or
    /// `FlatFloat` register). Reads the length from the param's `lens` slot — no
    /// host call. `dst` is an `Int` register.
    ListLenDirect {
        dst: u32,
        base: u32,
    },
    /// Profile-guided monomorphic inlining guard (J2). `base` is a `Handle`
    /// register holding a closure handle; reads its underlying function id via
    /// [`HostHelpers::closure_id`] and, if it differs from `expected`, **bails** to
    /// the interpreter (the existing re-run-from-top fallback). Emitted just before
    /// the inlined body of the profiled-monomorphic callee, so a different callee
    /// than the one speculated never runs native code. Writes no register; the
    /// matching (hot) path falls through with zero extra work beyond the compare.
    GuardClosureId {
        base: u32,
        expected: i64,
    },
    /// OSR-exit (J5.2). Marks the post-loop instruction at the loop's exit edge: a
    /// function compiled with an OSR-entry (see [`NativeModule::compile_osr`]) runs
    /// only the loop region natively, so reaching this instruction means the loop
    /// has exited and control must return to the interpreter. It lowers to an
    /// *unconditional* deopt safepoint whose `resume_ip` is this instruction's own
    /// index and whose `live` set is the registers definitely assigned on entry to
    /// it (the loop's live-out): the captured live-out window plus `resume_ip` are
    /// surfaced via [`NativeOutcome::Deopt`], and the host resumes the interpreter
    /// there (the precise-deopt resume path). Only valid in an OSR compilation; the
    /// default [`compile`](NativeModule::compile) path never emits it.
    OsrExit,
}

/// Storage class of a register: an unboxed `i64` (integers and booleans) or an
/// unboxed `f64` (floats). The arithmetic/compare instructions are
/// type-polymorphic — the same `Add`/`Compare`/… opcode lowers to integer or
/// float machine ops depending on the operand registers' types (mirroring the
/// VM, where `AddInt` etc. dispatch on the runtime `VmValue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitValueType {
    Int,
    Float,
    /// An opaque handle (index into the VM's per-call heap-value table) to a heap
    /// value — a struct/list/etc. — that can't live in a scalar register. Stored
    /// as `i64`; only valid as the `base` of a heap-read instruction.
    Handle,
    /// TV2: a flat `List<Int>` param passed as a raw `*const i64` data pointer (in
    /// the args word) plus its element count (in the parallel `lens` word). Stored
    /// as `i64` (the pointer bits); only valid as the `base` of a `*Direct` read.
    FlatInt,
    /// TV2: a flat `List<Float>` param passed as a raw `*const f64` data pointer
    /// plus its element count. Only valid as the `base` of a `*Direct` read.
    FlatFloat,
}

/// A compilable function: register count, per-register storage class, and the
/// instruction stream. Callers unbox each argument to `i64` bits (an `f64`'s bit
/// pattern for float registers) and read the result back the same way.
#[derive(Debug, Clone)]
pub struct JitFunction {
    pub n_params: u32,
    pub n_regs: u32,
    /// Storage class per register index (length `n_regs`).
    pub reg_types: Vec<JitValueType>,
    pub code: Vec<JitInstr>,
    /// Instruction indices that should start cold blocks. Producers may populate
    /// this from dynamic profile feedback; codegen treats it as a layout hint only.
    /// It never changes control flow or values.
    pub cold_blocks: Vec<u32>,
}

impl JitFunction {
    fn is_float(&self, reg: u32) -> bool {
        self.reg_types[reg as usize] == JitValueType::Float
    }
}

/// The native ABI of every compiled function:
/// `(args_ptr, n_args, lens_ptr, out_ptr, bail_ptr, safepoint_ptr) -> completed`.
/// Returns `1` and writes the result to `*out` on success, or `0` (leaving `*out`
/// untouched) to request fallback. `lens_ptr` points at an `i64` array parallel to
/// `args`: for a TV2 flat-array param (`FlatInt`/`FlatFloat`) the args word holds
/// the raw data pointer and the `lens` word holds the element count (for in-register
/// bounds-checked direct reads); other params' `lens` words are unused. `bail_ptr`
/// points at a `u8` flag the host helpers set when a heap read can't be satisfied;
/// the generated code loads it after every helper call and branches to fallback
/// immediately, so a bad read can't keep executing. `safepoint_ptr` points at a
/// host-owned `i64` cell into which the generated code *stores* the unique
/// [`SafepointId`] of the bail site on the bail edge (and only there — the hot
/// fall-through path never touches it); `0` means no bail was recorded.
/// `payload_ptr` points at a host-owned `i64` array of width
/// `deopt_map.payload_words` into which the generated code *stores* each live
/// register's value on the bail edge only (slot `reg` ← that register's 8-byte
/// word; an f64 register's bit pattern lands in its slot). Native-call bail edges
/// also chain the callee safepoint id and payload after the caller register
/// window. The hot fall-through path never writes it.
type CompiledAbi = unsafe extern "C" fn(
    *const i64,
    usize,
    *const i64,
    HostCtx,
    *mut i64,
    *const u8,
    *mut i64,
    *mut i64,
) -> u8;

#[derive(Debug)]
pub struct JitError(pub String);

impl std::fmt::Display for JitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "vm-jit error: {}", self.0)
    }
}
impl std::error::Error for JitError {}

fn err(context: &str, e: impl std::fmt::Display) -> JitError {
    JitError(format!("{context}: {e}"))
}

fn native_scalar_leaf_callable(function: &JitFunction, osr: bool, _returns_handle: bool) -> bool {
    !osr && function.reg_types.iter().all(|ty| {
        matches!(
            ty,
            JitValueType::Int
                | JitValueType::Float
                | JitValueType::Handle
                | JitValueType::FlatInt
                | JitValueType::FlatFloat
        )
    }) && function.code.iter().all(|instr| {
        matches!(
            instr,
            JitInstr::Nop
                | JitInstr::LoadInt { .. }
                | JitInstr::LoadFloat { .. }
                | JitInstr::LoadBool { .. }
                | JitInstr::Move { .. }
                | JitInstr::Add { .. }
                | JitInstr::Sub { .. }
                | JitInstr::Mul { .. }
                | JitInstr::Div { .. }
                | JitInstr::Mod { .. }
                | JitInstr::IntToFloat { .. }
                | JitInstr::BitAnd { .. }
                | JitInstr::BitOr { .. }
                | JitInstr::BitXor { .. }
                | JitInstr::Shl { .. }
                | JitInstr::Shr { .. }
                | JitInstr::Compare { .. }
                | JitInstr::Equal { .. }
                | JitInstr::NotEqual { .. }
                | JitInstr::Jump { .. }
                | JitInstr::JumpIfBool { .. }
                | JitInstr::JumpIfIntCompare { .. }
                | JitInstr::ProfiledJumpIfBool { .. }
                | JitInstr::ProfiledJumpIfIntCompare { .. }
                | JitInstr::CallNative { .. }
                | JitInstr::HostCall { .. }
                | JitInstr::MemoizedHostCall { .. }
                | JitInstr::Return { .. }
                | JitInstr::Bail
                | JitInstr::ListGetIntDirect { .. }
                | JitInstr::ListSetIntDirect { .. }
                | JitInstr::ListGetFloatDirect { .. }
                | JitInstr::ListLenDirect { .. }
        ) && !matches!(
            instr,
            JitInstr::HostCall { helper, .. } | JitInstr::MemoizedHostCall { helper, .. }
                if helper.heap_effect().extends_input_handles()
        )
    })
}

/// Declare an imported host helper with one opaque `HostCtx` word plus `n_args`
/// logical `i64` params and an `i64` result.
fn declare_import(module: &mut JITModule, name: &str, n_args: usize) -> Result<FuncId, JitError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    for _ in 0..n_args {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(name, Linkage::Import, &sig)
        .map_err(|e| err("declare import", e))
}

/// Declare an imported host helper with one opaque `HostCtx` word plus `n_args`
/// logical `i64` params and an `f64` result (the Float read helpers). The bail
/// signal stays out-of-band (the shared bail flag), so the f64 return channel
/// carries only the value.
fn declare_import_f64(
    module: &mut JITModule,
    name: &str,
    n_args: usize,
) -> Result<FuncId, JitError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    for _ in 0..n_args {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::F64));
    module
        .declare_function(name, Linkage::Import, &sig)
        .map_err(|e| err("declare import", e))
}

/// A compiled function plus the metadata `call` needs to invoke it safely: the
/// param count, so `call` can reject an argument slice of the wrong length (the
/// generated entry block reads exactly `n_params` words from `args_ptr` and does
/// not bound-check against `n_args`).
struct CompiledFunc {
    f: CompiledAbi,
    id: FuncId,
    /// Machine-code bytes emitted by Cranelift for this function, including any
    /// constant data reported by `CompiledCode::code_info`.
    code_size_bytes: u64,
    /// Longest native-to-native call chain reachable from this function. A function
    /// with no `CallNative` edges has depth 0; a direct native leaf call has depth 1.
    native_call_depth: u32,
    n_params: usize,
    /// Register count of the source [`JitFunction`] (the width of each site's
    /// register space).
    n_regs: usize,
    /// Per-safepoint deopt state-map (resume_ip + live registers), built host-side
    /// during `compile`. See [`DeoptMap`].
    deopt_map: DeoptMap,
    /// OSR-entry (J5.2): when `true`, the function was compiled with
    /// [`compile_osr`](NativeModule::compile_osr). Its `args_ptr` is the
    /// interpreter's full `n_regs`-wide register *window* (indexed by register),
    /// not a packed `n_params` arg array, so `call` validates the slice length
    /// against `n_regs` instead of `n_params`.
    osr: bool,
    /// Heap-result return ABI: `true` when this function's return
    /// register is a [`JitValueType::Handle`], so its completed `i64` result is an
    /// **output-table handle** (the host materializes a heap [`VmValue`] from it)
    /// rather than a scalar. Computed once at compile time from the function's
    /// `Return` source register type. A scalar-returning function has this `false`
    /// and takes the unchanged [`NativeOutcome::Completed`] path. An OSR function
    /// never has a top-level `Return` (it exits via `OsrExit`), so it is `false`.
    returns_handle: bool,
    param_types: Vec<JitValueType>,
    return_type: Option<JitValueType>,
    scalar_leaf_callable: bool,
}

#[derive(Clone)]
struct NativeCallee {
    handle: CompiledId,
    func_id: FuncId,
    n_params: usize,
    param_types: Vec<JitValueType>,
    deopt_payload_words: usize,
    return_type: JitValueType,
}

/// Process-wide source of per-module identities, so a [`CompiledId`] minted by one
/// [`NativeModule`] is rejected by another (it would otherwise index a different
/// module's function table). Monotonic; wraparound is not a practical concern.
static NEXT_MODULE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Owns the JIT-compiled machine code. Compiled functions live as long as the
/// module, so callers keep this alive and invoke by [`CompiledId`].
pub struct NativeModule {
    module: JITModule,
    ctx: Context,
    fbctx: FunctionBuilderContext,
    funcs: Vec<CompiledFunc>,
    counter: u32,
    /// Identity stamped into every [`CompiledId`] this module mints (see
    /// [`NEXT_MODULE_ID`]).
    id: u64,
    /// Declared host-helper imports (see [`HostHelpers`]).
    imports: HostFuncs,
}

/// `FuncId`s of the declared host helpers, resolved into per-function `FuncRef`s
/// at codegen time.
#[derive(Clone)]
struct HostFuncs {
    funcs: Vec<(HostHelper, FuncId)>,
}

impl HostFuncs {
    fn get(&self, helper: HostHelper) -> FuncId {
        self.funcs
            .iter()
            .find_map(|(candidate, id)| (*candidate == helper).then_some(*id))
            .expect("host helper import declared")
    }
}

/// Handle to a function compiled into a [`NativeModule`]. Carries the minting
/// module's identity so it can't be used against a different module (which would
/// index the wrong function table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledId {
    module_id: u64,
    index: usize,
}

/// Identity of a deopt (bail) point in a compiled function. Codegen assigns every
/// distinct guard/bail site a unique id numbered from 1 (see `build_function`);
/// the generated code stores it into the host's safepoint cell on the bail edge,
/// and [`NativeModule::call`] surfaces it as [`NativeOutcome::Deopt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafepointId(pub u32);

impl SafepointId {
    /// Reserved id `0`: no bail was recorded (the call either fell through to
    /// completion or bailed before any site stored its id). Real bail sites are
    /// numbered from `1`.
    pub const ANONYMOUS: SafepointId = SafepointId(0);
}

/// The deopt state for one safepoint: where the interpreter must resume and which
/// registers carry live state into that resume point.
///
/// `resume_ip` is the [`JitInstr`] index the interpreter re-executes when this
/// guard fires. It is the very instruction whose guard bailed: native code bails
/// *before* completing that instruction (e.g. before storing an `Add`'s checked
/// result), so the interpreter must run it again. Its inputs are therefore exactly
/// the registers definitely assigned on entry to that instruction.
///
/// `live` lists those entry-assigned registers (definite-assignment / "must"
/// analysis — see [`definite_assignment`]), each paired with its storage class. A
/// register absent from `live` is not guaranteed assigned on every path to the
/// resume point, so it carries no meaningful value to reconstruct.
///
/// This slice (J0.1a) only *computes and stores* the map; nothing reads it into the
/// running ABI yet (no payload is captured, no emitted code changes). It is the
/// schema foundation for J0.1b's payload capture and J0.2's reconstruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeoptSite {
    /// The `JitInstr` index to resume interpretation at (the bailing instruction).
    pub resume_ip: u32,
    /// Registers definitely assigned on entry to `resume_ip`, each `(reg, type)`.
    pub live: Vec<(u32, JitValueType)>,
    /// If this safepoint is a native-call edge, the callee's deopt payload is
    /// chained into this function's payload buffer so the host can inspect the
    /// complete native call stack.
    pub child: Option<DeoptChildSite>,
}

/// Location of a child native frame's deopt payload inside its caller's payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeoptChildSite {
    /// The compiled callee that produced the nested payload.
    pub callee: CompiledId,
    /// Slot containing the child [`SafepointId`] as an `i64`.
    pub safepoint_slot: u32,
    /// First slot of the child's payload buffer, copied verbatim from the child
    /// call. The child deopt map interprets this region.
    pub payload_slot: u32,
    /// Number of payload words copied from the child call.
    pub payload_words: u32,
}

/// Per-function deopt state-map, indexed by safepoint id.
///
/// Codegen mints safepoint ids from `1` (id `0` is [`SafepointId::ANONYMOUS`]), one
/// per [`bail_if`] call in emission order. This map mirrors that numbering with a
/// **0-based** vector: `sites[id - 1]` is the [`DeoptSite`] for `safepoint_id == id`
/// (so `sites[0]` is id `1`). The alignment is structural — codegen pushes exactly
/// one [`DeoptSite`] per `bail_if` call, in the same traversal that increments the
/// id counter — so indices never drift from ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeoptMap {
    /// `sites[id - 1]` is the site for `safepoint_id == id` (ids start at 1).
    pub sites: Vec<DeoptSite>,
    /// Total width of the deopt payload buffer required by this function: its own
    /// register window plus any chained child native-call payload regions.
    pub payload_words: usize,
}

/// The runtime value of a live register captured at a deopt, typed by its storage
/// class so the caller can reconstruct it faithfully (an `i64` integer/boolean, or
/// an exact `f64`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeoptValue {
    /// An integer (or boolean `0`/`1`) register's value.
    Int(i64),
    /// A float register's value (decoded from its captured 8-byte bit pattern).
    Float(f64),
}

/// One live register's captured value at a deopt: its register index plus the
/// decoded [`DeoptValue`]. See [`NativeOutcome::Deopt`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeoptReg {
    /// The VM register index.
    pub reg: u32,
    /// The register's value at the bail edge.
    pub value: DeoptValue,
}

/// A nested native frame captured when a `CallNative` callee deopts. The top-level
/// [`NativeOutcome::Deopt`] still names the caller safepoint; this chain preserves
/// the callee safepoint and live payload so embedders can build a full native-frame
/// deopt later instead of losing the child frame at the call edge.
#[derive(Debug, Clone, PartialEq)]
pub struct DeoptFrame {
    /// Compiled function whose frame deopted.
    pub function: CompiledId,
    /// Safepoint in `function`.
    pub safepoint_id: SafepointId,
    /// Live registers decoded with `function`'s [`DeoptMap`].
    pub live: Vec<DeoptReg>,
    /// Further nested child, when native calls are chained.
    pub child: Option<Box<DeoptFrame>>,
}

/// Outcome of running a compiled function via [`NativeModule::call`]: either the
/// function ran to completion with a 64-bit result, or it deopted at a named
/// safepoint and the interpreter should re-run it.
#[derive(Debug, Clone, PartialEq)]
pub enum NativeOutcome {
    /// The function completed; the payload is the result bits (an `i64`, or an
    /// `f64` bit pattern for a float-returning function).
    Completed(i64),
    /// The function completed and its result is a **heap value** (a struct/list),
    /// not a scalar. The payload is an **opaque output-table handle**: the host
    /// materializes the actual [`VmValue`] (host-side type) from its VM-owned output
    /// table at this index. Emitted only on a clean completion (bail flag clear) of
    /// a function whose return register is a [`JitValueType::Handle`]; the scalar
    /// [`Completed`](NativeOutcome::Completed) path is byte-for-byte unchanged.
    ///
    /// **§7.2-safety:** the host materializes the result **only** on this clean
    /// completion; **any** bail returns [`Deopt`](NativeOutcome::Deopt) and the
    /// output table is cleared by the VM-side guard, so a bailed attempt has no
    /// observable heap result and §7.2's fallback-equivalence proof holds.
    CompletedHandle(i64),
    /// The function deopted at `safepoint_id` (a guard bail or a host-helper bail)
    /// and the caller must fall back to the interpreter. `live` carries each
    /// register definitely assigned at the resume point with its captured value
    /// (per the J0.1a state-map); it is empty for a deopt rejected before the call
    /// (id/length mismatch). The caller's behavior depends on its mode (J0.2): by
    /// default it re-runs the function from the top and ignores `live` (sound for the
    /// side-effect-free subset); with precise deopt enabled (`RSS_JIT_PRECISE_DEOPT`)
    /// it consumes `live` to reconstruct the interpreter window and resumes at the
    /// safepoint's `resume_ip` instead. `child` is populated when this deopt came
    /// from a nested [`JitInstr::CallNative`] callee; embedders that support full
    /// native frame-chain deopt can inspect it, while conservative embedders may
    /// still resume/re-run at this caller safepoint.
    Deopt {
        safepoint_id: SafepointId,
        live: Vec<DeoptReg>,
        child: Option<Box<DeoptFrame>>,
    },
}

impl NativeOutcome {
    /// The completed **scalar** result bits, or `None` otherwise. `Some` ONLY for
    /// [`Completed`](NativeOutcome::Completed) (a genuine `i64`/`f64`-bits scalar);
    /// [`CompletedHandle`](NativeOutcome::CompletedHandle) and
    /// [`Deopt`](NativeOutcome::Deopt) both yield `None`. A `CompletedHandle` payload
    /// is an OPAQUE output-table handle, not a scalar value, so conflating it here
    /// would let a caller misread a heap-table index as a result — hence this method
    /// is deliberately scalar-only. Use [`completed_handle`](NativeOutcome::completed_handle)
    /// for the handle, or [`completed_any_bits`](NativeOutcome::completed_any_bits)
    /// when you only need the raw bits of either completed variant.
    pub fn completed(self) -> Option<i64> {
        match self {
            NativeOutcome::Completed(value) => Some(value),
            NativeOutcome::CompletedHandle(_) | NativeOutcome::Deopt { .. } => None,
        }
    }

    /// The completed **heap-value handle**, or `None` otherwise. `Some` ONLY for
    /// [`CompletedHandle`](NativeOutcome::CompletedHandle) — the opaque output-table
    /// index the host materializes the [`VmValue`] from. [`Completed`](NativeOutcome::Completed)
    /// (a scalar) and [`Deopt`](NativeOutcome::Deopt) yield `None`. The returned value
    /// is NOT a scalar result; it is meaningful only as an output-table handle.
    pub fn completed_handle(self) -> Option<i64> {
        match self {
            NativeOutcome::CompletedHandle(handle) => Some(handle),
            NativeOutcome::Completed(_) | NativeOutcome::Deopt { .. } => None,
        }
    }

    /// The raw 64-bit payload of EITHER completed variant, or `None` on a deopt.
    /// `Some` for both [`Completed`](NativeOutcome::Completed) (scalar result bits)
    /// and [`CompletedHandle`](NativeOutcome::CompletedHandle) (an opaque output-table
    /// handle); `None` only for [`Deopt`](NativeOutcome::Deopt). The two cases are
    /// indistinguishable in the returned `i64`, so this is for callers that genuinely
    /// only need "did it complete, give me the raw bits" and disambiguate elsewhere
    /// (e.g. by the return register's [`JitValueType`]). When the scalar/handle
    /// distinction matters, use [`completed`](NativeOutcome::completed) /
    /// [`completed_handle`](NativeOutcome::completed_handle) instead.
    pub fn completed_any_bits(self) -> Option<i64> {
        match self {
            NativeOutcome::Completed(value) | NativeOutcome::CompletedHandle(value) => Some(value),
            NativeOutcome::Deopt { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForcedDeopt {
    Site(u32),
    All,
}

impl ForcedDeopt {
    fn forces(self, site_id: i64) -> bool {
        match self {
            ForcedDeopt::Site(site) => i64::from(site) == site_id,
            ForcedDeopt::All => true,
        }
    }
}

impl NativeModule {
    /// Optimizing native tier (back-compat default): `opt_level="speed"`.
    pub fn new(helpers: HostHelpers) -> Result<Self, JitError> {
        Self::new_with_opt(helpers, false)
    }

    /// Build a native module at a selectable optimization level.
    ///
    /// `baseline == true` selects the Phase-2 path-B **baseline tier**:
    /// `opt_level="none"`. Everything else — IR translation, host helpers, the
    /// bail-flag deopt protocol — is byte-for-byte identical to the optimizing
    /// path; only the Cranelift ISA `opt_level` flag changes. The win is
    /// *compile latency* (less codegen work), at the cost of slightly less
    /// optimized machine code. Because the compiled subset is the
    /// side-effect-free scalar + read-only-heap set, the interpreter/`run_jit`
    /// deopt oracle remains valid verbatim regardless of opt level.
    ///
    /// `baseline == false` keeps the optimizing hot-path tier (`opt_level="speed"`).
    pub fn new_with_opt(helpers: HostHelpers, baseline: bool) -> Result<Self, JitError> {
        let mut flags = settings::builder();
        // Plain JIT: no PIC. Optimize for speed on the hot path, or skip
        // optimization entirely in baseline mode to minimize compile latency.
        flags
            .set("use_colocated_libcalls", "false")
            .map_err(|e| err("settings", e))?;
        flags
            .set("is_pic", "false")
            .map_err(|e| err("settings", e))?;
        flags
            .set("opt_level", if baseline { "none" } else { "speed" })
            .map_err(|e| err("settings", e))?;
        let flags = settings::Flags::new(flags);
        // Build the host ISA with our flags (so `opt_level` actually applies),
        // then a JIT module on top of it.
        let isa = cranelift_native::builder()
            .map_err(|e| err("host isa", e))?
            .finish(flags)
            .map_err(|e| err("isa finish", e))?;
        let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
        // Register the host helper addresses so imported calls link to them.
        // The typed `extern "C"` pointers become the `*const u8` Cranelift's symbol
        // table wants here, where this crate owns the obligation that the address
        // matches the imported signature declared just below.
        for &helper in HostHelper::all() {
            builder.symbol(helper.symbol(), helpers.addr(helper));
        }
        let mut module = JITModule::new(builder);
        let imports = HostFuncs {
            funcs: HostHelper::all()
                .iter()
                .map(|&helper| {
                    let sig = helper.signature();
                    let id = if matches!(sig.result, HostResult::Exact(JitValueType::Float)) {
                        declare_import_f64(&mut module, helper.symbol(), sig.args.len())
                    } else {
                        declare_import(&mut module, helper.symbol(), sig.args.len())
                    }?;
                    Ok((helper, id))
                })
                .collect::<Result<Vec<_>, JitError>>()?,
        };
        let ctx = module.make_context();
        Ok(Self {
            module,
            ctx,
            fbctx: FunctionBuilderContext::new(),
            funcs: Vec::new(),
            counter: 0,
            id: NEXT_MODULE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            imports,
        })
    }

    /// Compile `function` to native code and return a handle to call it.
    pub fn compile(&mut self, function: &JitFunction) -> Result<CompiledId, JitError> {
        self.compile_inner(function, None, None)
    }

    /// Compile `function` while forcing the safepoint with id `force_site` (sites are
    /// numbered from 1 in emission order) to bail *unconditionally*: any execution
    /// reaching that site deopts there regardless of its guard condition, capturing
    /// the same live set the natural bail would. All other sites behave normally.
    ///
    /// This is a test/diagnostic hook for exercising the deopt capture + map at every
    /// safepoint, including ones that never fire under normal inputs. The default
    /// [`compile`](Self::compile) path passes `None` and emits identical code, so this
    /// option has zero production cost. An out-of-range `force_site` simply matches no
    /// site and the function compiles as usual.
    pub fn compile_forcing_bail(
        &mut self,
        function: &JitFunction,
        force_site: u32,
    ) -> Result<CompiledId, JitError> {
        self.compile_inner(function, Some(ForcedDeopt::Site(force_site)), None)
    }

    /// Compile `function` while forcing every generated safepoint to bail
    /// unconditionally. This is a diagnostic/deopt-stress hook: any executed guard
    /// edge exercises the real deopt capture path regardless of the guard's natural
    /// condition. The default [`compile`](Self::compile) path emits byte-identical
    /// guarded code and never sets this mode.
    pub fn compile_forcing_all_bails(
        &mut self,
        function: &JitFunction,
    ) -> Result<CompiledId, JitError> {
        self.compile_inner(function, Some(ForcedDeopt::All), None)
    }

    /// Compile `function` as an **OSR (on-stack replacement) entry** at `header_ip`
    /// (J5.2). Instead of the normal param-loading entry, the generated function's
    /// entry block treats its `args_ptr` argument as the interpreter's **register
    /// window** (an `i64`/`f64` array of width `n_regs`, indexed by register), loads
    /// the registers definitely-assigned on entry to `header_ip` (the loop's
    /// live-in) out of it, then jumps directly to the block for `header_ip` — so
    /// native execution begins *inside* the loop rather than at the function top.
    ///
    /// The loop exits by reaching a [`JitInstr::OsrExit`] (the post-loop ip), which
    /// deopts with the live-out window and that ip as `resume_ip`; the host then
    /// resumes the interpreter there via the precise-deopt path. Everything outside
    /// the loop (which the host never reaches natively under OSR) is `Bail`/`OsrExit`.
    ///
    /// The window ABI reuses the `CompiledAbi` `args_ptr` slot: it must be a buffer
    /// of `n_regs` 8-byte words (each register's value at offset `reg * 8`; an f64
    /// register's bit pattern in its slot), and the caller passes `n_args = n_regs`
    /// with an `n_regs`-long `lens` slice. An out-of-range `header_ip` (no leader
    /// block) is rejected as a [`JitError`].
    pub fn compile_osr(
        &mut self,
        function: &JitFunction,
        header_ip: u32,
    ) -> Result<CompiledId, JitError> {
        self.compile_inner(function, None, Some(header_ip))
    }

    fn compile_inner(
        &mut self,
        function: &JitFunction,
        forced: Option<ForcedDeopt>,
        osr_header: Option<u32>,
    ) -> Result<CompiledId, JitError> {
        // `JitFunction` is a public, versioned surface: a malformed producer must
        // fail cleanly here, not panic inside `build_function` (out-of-range index)
        // or trip Cranelift's verifier (a type mismatch) deep in codegen.
        validate(function, osr_header.is_some())?;
        // Heap-result return ABI: a non-OSR function whose `Return` source is a
        // `Handle` register returns an output-table handle, not a scalar. Determined
        // purely from the (validated) IR, before codegen. OSR functions never have a
        // top-level `Return` (they exit via `OsrExit`), so this stays `false`.
        let returns_handle = osr_header.is_none()
            && function.code.iter().any(|instr| {
                matches!(instr, JitInstr::Return { src }
                    if function.reg_types[*src as usize] == JitValueType::Handle)
            });
        let return_type = if osr_header.is_none() {
            function.code.iter().find_map(|instr| {
                if let JitInstr::Return { src } = instr {
                    Some(function.reg_types[*src as usize])
                } else {
                    None
                }
            })
        } else {
            None
        };
        let scalar_leaf_callable =
            native_scalar_leaf_callable(function, osr_header.is_some(), returns_handle);
        let native_callees = self.resolve_native_callees(function)?;
        let native_call_depth = native_callees
            .iter()
            .filter_map(|callee| self.funcs.get(callee.handle.index))
            .map(|callee| callee.native_call_depth.saturating_add(1))
            .max()
            .unwrap_or(0);
        let ptr_ty = self.module.target_config().pointer_type();
        self.module.clear_context(&mut self.ctx);
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // args ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // n_args
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // lens ptr (TV2)
        self.ctx
            .func
            .signature
            .params
            .push(AbiParam::new(types::I64)); // host helper context
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // out ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // bail flag ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // safepoint id out ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // deopt payload out ptr
        self.ctx
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I8));

        let deopt_map = build_function(
            &mut self.ctx.func,
            &mut self.fbctx,
            &mut self.module,
            self.imports.clone(),
            function,
            forced,
            osr_header,
            &native_callees,
        )?;

        let name = format!("rss_jit_{}", self.counter);
        self.counter += 1;
        let id = self
            .module
            .declare_function(&name, Linkage::Local, &self.ctx.func.signature)
            .map_err(|e| err("declare", e))?;
        self.module
            .define_function(id, &mut self.ctx)
            .map_err(|e| err("define", e))?;
        let code_size_bytes = self
            .ctx
            .compiled_code()
            .map(|code| u64::from(code.code_info().total_size))
            .unwrap_or(0);
        self.module.clear_context(&mut self.ctx);
        self.module
            .finalize_definitions()
            .map_err(|e| err("finalize", e))?;
        let code = self.module.get_finalized_function(id);
        // SAFETY: `code` points at the machine code we just emitted with exactly
        // the `CompiledAbi` signature declared above.
        let f: CompiledAbi = unsafe { std::mem::transmute::<*const u8, CompiledAbi>(code) };
        let handle = CompiledId {
            module_id: self.id,
            index: self.funcs.len(),
        };
        self.funcs.push(CompiledFunc {
            f,
            id,
            code_size_bytes,
            native_call_depth,
            n_params: function.n_params as usize,
            n_regs: function.n_regs as usize,
            deopt_map,
            osr: osr_header.is_some(),
            returns_handle,
            param_types: function.reg_types[..function.n_params as usize].to_vec(),
            return_type,
            scalar_leaf_callable,
        });
        Ok(handle)
    }

    /// Machine-code bytes emitted for a compiled function, including any constant
    /// data reported by Cranelift. Used only for host-side telemetry.
    pub fn code_size_bytes(&self, id: CompiledId) -> Option<u64> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| func.code_size_bytes)
    }

    /// Longest native-to-native call chain reachable from a compiled function.
    /// Used only for host-side telemetry.
    pub fn native_call_depth(&self, id: CompiledId) -> Option<u32> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| func.native_call_depth)
    }

    fn resolve_native_callees(
        &self,
        function: &JitFunction,
    ) -> Result<Vec<NativeCallee>, JitError> {
        let mut callees = Vec::new();
        for instr in &function.code {
            let JitInstr::CallNative { callee, dst, args } = instr else {
                continue;
            };
            if callee.module_id != self.id {
                return Err(JitError(
                    "CallNative callee belongs to a different module".into(),
                ));
            }
            let compiled = self
                .funcs
                .get(callee.index)
                .ok_or_else(|| JitError("CallNative callee index is out of range".into()))?;
            if compiled.osr {
                return Err(JitError(
                    "CallNative does not support OSR callees yet".into(),
                ));
            }
            if !compiled.scalar_leaf_callable {
                return Err(JitError(
                    "CallNative callee is not a scalar-callable function".into(),
                ));
            }
            if compiled.n_params != args.len() {
                return Err(JitError(format!(
                    "CallNative got {} args, callee expects {}",
                    args.len(),
                    compiled.n_params
                )));
            }
            let Some(return_type) = compiled.return_type else {
                return Err(JitError(
                    "CallNative callee has no scalar Return instruction".into(),
                ));
            };
            if matches!(return_type, JitValueType::FlatInt | JitValueType::FlatFloat) {
                return Err(JitError(format!(
                    "CallNative callee return type {return_type:?} is not callable"
                )));
            }
            if function.reg_types[*dst as usize] != return_type {
                return Err(JitError(format!(
                    "CallNative result register {dst} is {:?}, callee returns {return_type:?}",
                    function.reg_types[*dst as usize]
                )));
            }
            for (i, (&arg, &expected)) in args.iter().zip(compiled.param_types.iter()).enumerate() {
                if function.reg_types[arg as usize] != expected {
                    return Err(JitError(format!(
                        "CallNative arg {i}: caller register {arg} is {:?}, callee expects {expected:?}",
                        function.reg_types[arg as usize]
                    )));
                }
            }
            callees.push(NativeCallee {
                handle: *callee,
                func_id: compiled.id,
                n_params: compiled.n_params,
                param_types: compiled.param_types.clone(),
                deopt_payload_words: compiled.deopt_map.payload_words,
                return_type,
            });
        }
        Ok(callees)
    }

    /// Run a compiled function. Returns [`NativeOutcome::Completed`] with the result
    /// on completion, or [`NativeOutcome::Deopt`] when the native code bailed and the
    /// interpreter should re-run the function — either a guard bail (overflow/
    /// divide-by-zero edge) or a host-helper bail (an unsatisfiable heap read; see
    /// [`signal_bail`]). The returned [`SafepointId`] identifies the exact bail site
    /// (codegen numbers sites from 1; [`SafepointId::ANONYMOUS`] / `0` means no site
    /// recorded an id, e.g. an id/length mismatch rejected before the call).
    ///
    /// This is a **fully safe** boundary for scalar/handle args. The bail flag is a
    /// per-thread `u8` owned by this crate; `call` resets it, passes its own address
    /// into the generated code, and reports a set flag as a fallback.
    ///
    /// `args` and `lens` are parallel slices indexed by param (both length
    /// `n_params`). For a TV2 flat-array param the caller places the raw data
    /// pointer (`*const i64`/`*const f64` reinterpreted as `i64`) in `args[i]` and
    /// the element count in `lens[i]`. **SAFETY (TV2 borrow protocol — caller
    /// obligation):** any pointer placed in `args` for a flat-array param must point
    /// at a buffer that stays allocated, immovable, and unmutated for the entire
    /// duration of this call. The generated code reads at most `lens[i]` consecutive
    /// elements from it (every index is bounds-checked against `lens[i]` →
    /// `signal_bail` on OOB) and never writes or retains it. The VM caller satisfies
    /// this by pinning a shared `Ref` borrow of the backing `RefCell<TypedVec>` for
    /// the call (so no `borrow_mut`/realloc can occur); see `try_native`.
    pub fn call(&self, id: CompiledId, args: &[i64], lens: &[i64]) -> NativeOutcome {
        self.call_with_host_ctx(id, args, lens, 0)
    }

    fn decode_deopt_live(site: &DeoptSite, payload_base: usize, payload: &[i64]) -> Vec<DeoptReg> {
        site.live
            .iter()
            .filter_map(|&(reg, ty)| {
                payload.get(payload_base + reg as usize).map(|&bits| {
                    let value = match ty {
                        JitValueType::Float => DeoptValue::Float(f64::from_bits(bits as u64)),
                        _ => DeoptValue::Int(bits),
                    };
                    DeoptReg { reg, value }
                })
            })
            .collect()
    }

    fn decode_deopt_child(
        &self,
        child: DeoptChildSite,
        payload_base: usize,
        payload: &[i64],
    ) -> Option<Box<DeoptFrame>> {
        let safepoint_bits = *payload.get(payload_base + child.safepoint_slot as usize)?;
        if safepoint_bits <= 0 {
            return None;
        }
        self.decode_deopt_frame(
            child.callee,
            SafepointId(safepoint_bits as u32),
            payload_base + child.payload_slot as usize,
            payload,
        )
        .map(Box::new)
    }

    fn decode_deopt_frame(
        &self,
        function: CompiledId,
        safepoint_id: SafepointId,
        payload_base: usize,
        payload: &[i64],
    ) -> Option<DeoptFrame> {
        if function.module_id != self.id || safepoint_id.0 == 0 {
            return None;
        }
        let func = self.funcs.get(function.index)?;
        let site = func.deopt_map.sites.get(safepoint_id.0 as usize - 1)?;
        let live = Self::decode_deopt_live(site, payload_base, payload);
        let child = site
            .child
            .and_then(|child| self.decode_deopt_child(child, payload_base, payload));
        Some(DeoptFrame {
            function,
            safepoint_id,
            live,
            child,
        })
    }

    /// Run a compiled function while forwarding `host_ctx` to every imported host
    /// helper. The context is opaque to `vm-jit`; the embedding VM validates and
    /// interprets it.
    pub fn call_with_host_ctx(
        &self,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        host_ctx: HostCtx,
    ) -> NativeOutcome {
        // Reject an id from a different module and an out-of-range index: either
        // would invoke the wrong (or no) function. Falling back is always safe.
        if id.module_id != self.id {
            return NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live: Vec::new(),
                child: None,
            };
        }
        let func = match self.funcs.get(id.index) {
            Some(func) => func,
            None => {
                return NativeOutcome::Deopt {
                    safepoint_id: SafepointId::ANONYMOUS,
                    live: Vec::new(),
                    child: None,
                };
            }
        };
        // The generated entry block reads words from `args_ptr` (and `lens_ptr`)
        // without consulting `n_args`, so a slice shorter than what the entry reads
        // would read out of bounds. A normal compile reads `n_params` packed args; an
        // OSR-entry reads from the full `n_regs`-wide register window. Reject any
        // length mismatch against the required width.
        let required = if func.osr { func.n_regs } else { func.n_params };
        if args.len() != required || lens.len() != required {
            return NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live: Vec::new(),
                child: None,
            };
        }
        let f = func.f;
        let returns_handle = func.returns_handle;
        let deopt_map = &func.deopt_map;
        let mut out: i64 = 0;
        BAIL_FLAG.with(|bail| {
            SAFEPOINT_ID.with(|safepoint| {
                DEOPT_PAYLOAD.with(|payload| {
                    bail.set(0);
                    safepoint.set(0);
                    let bail_ptr = bail.as_ptr() as *const u8;
                    let safepoint_ptr = safepoint.as_ptr();
                    // Reused per-thread scratch buffer for the deopt payload: a valid
                    // pointer for every call, but only written on a bail edge. Grow-only
                    // (no per-call zeroing): the success hot path neither allocates nor
                    // memsets, and a bail only ever reads slots the generated code just
                    // wrote (the live-register set), so stale words in other slots are
                    // never observed.
                    let payload_ptr = {
                        let mut buf = payload.borrow_mut();
                        if buf.len() < deopt_map.payload_words {
                            buf.resize(deopt_map.payload_words, 0);
                        }
                        buf.as_mut_ptr()
                    };
                    // SAFETY: `f` was produced by `compile` with the `CompiledAbi`
                    // signature; it reads `args.len()` i64s from `args.as_ptr()` and
                    // `lens.as_ptr()`, writes one i64 to `&mut out`, and only ever loads
                    // (never stores) the `u8` at `bail_ptr` — this thread's `BAIL_FLAG`
                    // cell, valid for the call. It only ever *stores* to the `i64` at
                    // `safepoint_ptr` (the symmetric write-direction-opposite of
                    // `bail_ptr`) — this thread's `SAFEPOINT_ID` cell, also valid for the
                    // call, and only on a bail edge (the hot path never touches it).
                    // `payload_ptr` addresses this thread's `DEOPT_PAYLOAD` buffer, sized
                    // to `deopt_map.payload_words` above and held borrowed (so it stays
                    // valid and immovable) for the whole call; the generated code only
                    // ever *stores* live register words and copied child-frame payloads
                    // into it, and only on a bail edge (the hot path never touches it).
                    // Any flat-array data pointer in `args` is read in-bounds (against
                    // the matching `lens` entry) per the caller's borrow-protocol
                    // obligation documented above. The generated code never retains any
                    // of the pointers.
                    let completed = unsafe {
                        f(
                            args.as_ptr(),
                            args.len(),
                            lens.as_ptr(),
                            host_ctx,
                            &mut out as *mut i64,
                            bail_ptr,
                            safepoint_ptr,
                            payload_ptr,
                        )
                    };
                    if completed != 0 && bail.get() == 0 {
                        // Success: leave the payload buffer untouched, build no Vec.
                        // Heap-result return ABI: a Handle-returning function's
                        // `out` is an output-table handle, signalled distinctly so the
                        // host materializes a heap value from it. The scalar path is
                        // byte-for-byte unchanged. §7.2: this branch runs ONLY on a
                        // clean completion (`completed != 0 && bail.get() == 0`); any
                        // bail takes the `else` (`Deopt`) arm, so no heap result is
                        // ever reported on a bailed attempt.
                        if returns_handle {
                            NativeOutcome::CompletedHandle(out)
                        } else {
                            NativeOutcome::Completed(out)
                        }
                    } else {
                        let safepoint_id = SafepointId(safepoint.get() as u32);
                        // Decode the captured live registers via the J0.1a state-map.
                        // A real bail site (id >= 1) names a `sites[id - 1]` entry; an
                        // anonymous bail (id 0, e.g. fell off the end) has no site, so
                        // `live` is empty.
                        let buf = payload.borrow();
                        let frame = self.decode_deopt_frame(id, safepoint_id, 0, &buf);
                        NativeOutcome::Deopt {
                            safepoint_id,
                            live: frame
                                .as_ref()
                                .map_or_else(Vec::new, |frame| frame.live.clone()),
                            child: frame.and_then(|frame| frame.child),
                        }
                    }
                })
            })
        })
    }

    /// The per-function [`DeoptMap`] computed at compile time, or `None` if `id`
    /// is foreign / out of range (validated exactly like [`call`]). Index it by
    /// safepoint id via `map.sites[id - 1]` (ids start at 1). Host-side metadata
    /// only — reading it has no effect on execution.
    pub fn deopt_map(&self, id: CompiledId) -> Option<&DeoptMap> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| &func.deopt_map)
    }

    /// Register count of the compiled function (the width of its register space),
    /// or `None` for a foreign / out-of-range `id`. Companion to [`deopt_map`]: a
    /// site's `live` regs index into this space.
    pub fn n_regs(&self, id: CompiledId) -> Option<usize> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| func.n_regs)
    }
}

std::thread_local! {
    /// Per-thread bail flag shared between the in-flight compiled call (which loads
    /// it) and the host helpers (which set it via [`signal_bail`]). `call` resets it
    /// before each invocation, so it is only meaningful during a call.
    static BAIL_FLAG: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };

    /// Per-thread safepoint-id cell the in-flight compiled call *stores* into on a
    /// bail edge (mirrors [`BAIL_FLAG`], opposite write direction). `call` resets it
    /// to `0` before each invocation, so it is only meaningful during a call: `0`
    /// means no bail site fired; a non-zero value is the [`SafepointId`] of the site
    /// that bailed.
    static SAFEPOINT_ID: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };

    /// Per-thread reused scratch buffer for the deopt payload. `call` resizes it to
    /// the function's `deopt_map.payload_words` and passes its pointer into the
    /// compiled function, which *stores* each live register's value into its slot on
    /// a bail edge only; native-call bail edges also copy chained child payloads.
    /// Reused across calls so the success hot path performs no steady-state
    /// allocation.
    static DEOPT_PAYLOAD: std::cell::RefCell<Vec<i64>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Signal from a [`HostHelpers`] callback that the in-flight native call cannot be
/// satisfied (wrong type / out-of-bounds heap read), so the function must fall
/// back to the interpreter. The generated code loads the flag immediately after
/// each helper call and branches to fallback when it is set; [`NativeModule::call`]
/// also reports it. Safe to call any time — it is a no-op outside a `call`, since
/// `call` resets the flag at the start of every invocation.
pub fn signal_bail() {
    BAIL_FLAG.with(|flag| flag.set(1));
}

/// Validate public IR before codegen. `build_function` assumes well-formed input
/// (it indexes `reg_types`/`code` directly and relies on Cranelift register types
/// matching each opcode); this turns every such assumption into a clean
/// [`JitError`] so a buggy producer can never reach codegen with input that would
/// panic or generate invalid assumptions.
///
/// Storage-class rules mirror `build_function`'s lowering: arithmetic preserves the
/// operand class (and forbids `Handle`), the int-only ops (`Mod`, bit/shift) require
/// `Int`, comparisons yield an `Int` boolean, and `Handle` registers are only valid
/// as the `base` of a heap read.
fn validate(program: &JitFunction, osr: bool) -> Result<(), JitError> {
    let n_regs = program.n_regs as usize;
    let n = program.code.len();

    if program.reg_types.len() != n_regs {
        return Err(JitError(format!(
            "reg_types length {} does not match n_regs {n_regs}",
            program.reg_types.len()
        )));
    }
    if program.n_params > program.n_regs {
        return Err(JitError(format!(
            "n_params {} exceeds n_regs {n_regs}",
            program.n_params
        )));
    }
    for &cold_ip in &program.cold_blocks {
        if (cold_ip as usize) >= n {
            return Err(JitError(format!(
                "cold block instruction {cold_ip} is out of range for {n} instructions"
            )));
        }
    }

    let check_reg = |r: u32| -> Result<(), JitError> {
        if (r as usize) < n_regs {
            Ok(())
        } else {
            Err(JitError(format!(
                "register {r} out of range (n_regs {n_regs})"
            )))
        }
    };
    let class = |r: u32| program.reg_types[r as usize];
    // Non-scalar register classes (opaque handle or flat-array pointer): valid only
    // as the `base` of a heap/direct read, never in scalar/arith/move/return.
    let is_nonscalar = |r: u32| {
        matches!(
            class(r),
            JitValueType::Handle | JitValueType::FlatInt | JitValueType::FlatFloat
        )
    };
    let check_target = |t: u32| -> Result<(), JitError> {
        if (t as usize) < n {
            Ok(())
        } else {
            Err(JitError(format!(
                "jump target {t} out of range (code length {n})"
            )))
        }
    };

    // Two operands of the same scalar (non-`Handle`) class: the shape every
    // arithmetic/comparison opcode requires.
    let scalar_pair = |lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        check_reg(lhs)?;
        check_reg(rhs)?;
        if is_nonscalar(lhs) || is_nonscalar(rhs) {
            return Err(JitError(format!(
                "{op}: operand is a non-scalar (Handle/flat) register"
            )));
        }
        if class(lhs) != class(rhs) {
            return Err(JitError(format!(
                "{op}: operand classes differ ({:?} vs {:?})",
                class(lhs),
                class(rhs)
            )));
        }
        Ok(())
    };
    // Arithmetic: result register has the operands' class.
    let arith = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        scalar_pair(lhs, rhs, op)?;
        check_reg(dst)?;
        if class(dst) != class(lhs) {
            return Err(JitError(format!(
                "{op}: result {:?} does not match operands {:?}",
                class(dst),
                class(lhs)
            )));
        }
        Ok(())
    };
    // Integer-only ternary (Mod, bitwise, shift): every register must be `Int`.
    let int_op = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        check_reg(dst)?;
        check_reg(lhs)?;
        check_reg(rhs)?;
        for r in [dst, lhs, rhs] {
            if class(r) != JitValueType::Int {
                return Err(JitError(format!("{op}: register {r} must be Int")));
            }
        }
        Ok(())
    };
    // Comparison: operands share a scalar class, result is an `Int` boolean.
    let compare = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        scalar_pair(lhs, rhs, op)?;
        check_reg(dst)?;
        if class(dst) != JitValueType::Int {
            return Err(JitError(format!("{op}: boolean result must be Int")));
        }
        Ok(())
    };
    let require_class = |r: u32, want: JitValueType, op: &str| -> Result<(), JitError> {
        check_reg(r)?;
        if class(r) != want {
            return Err(JitError(format!(
                "{op}: register {r} is {:?}, expected {want:?}",
                class(r)
            )));
        }
        Ok(())
    };
    // A flat-array base must be the expected flat class *and* enter through the
    // caller's args/lens window (never produced internally). For a normal compile the
    // window is the packed params (`0..n_params`); for an OSR-entry the window is the
    // full `n_regs`-wide register file (args/lens are both `n_regs`-wide and loaded by
    // register index — see the OSR `load_set`), so any window register may carry a
    // flat pointer the host marshalled and bounds-checked against the parallel `lens`
    // slot. The host's borrow protocol pins the buffer for the call's duration either
    // way.
    let require_flat_param = |r: u32, want: JitValueType, op: &str| -> Result<(), JitError> {
        require_class(r, want, op)?;
        let window = if osr {
            program.n_regs as usize
        } else {
            program.n_params as usize
        };
        if (r as usize) >= window {
            return Err(JitError(format!("{op}: register {r} is not a parameter")));
        }
        Ok(())
    };

    for (i, instr) in program.code.iter().enumerate() {
        // Conditional branches fall through to `i + 1` (`build_function` indexes
        // `block_for[i + 1]`), so the instruction must not be the last one.
        let check_fallthrough = || -> Result<(), JitError> {
            if i + 1 < n {
                Ok(())
            } else {
                Err(JitError(format!(
                    "conditional branch at {i} has no fall-through instruction"
                )))
            }
        };
        match instr {
            JitInstr::Nop | JitInstr::Bail => {}
            JitInstr::LoadInt { dst, .. } => require_class(*dst, JitValueType::Int, "LoadInt")?,
            JitInstr::LoadFloat { dst, .. } => {
                require_class(*dst, JitValueType::Float, "LoadFloat")?
            }
            JitInstr::LoadBool { dst, .. } => require_class(*dst, JitValueType::Int, "LoadBool")?,
            JitInstr::Move { dst, src } => {
                check_reg(*dst)?;
                check_reg(*src)?;
                // A **Handle** register is a plain `i64` (a heap-table index), so a
                // Move just copies it — sound for a stored closure handle threaded
                // through a temporary (Pending #1). A **flat-array** register (pointer
                // + length, keyed by reg in the `lens` slice) genuinely cannot be
                // moved, so those stay rejected.
                let is_flat =
                    |r: u32| matches!(class(r), JitValueType::FlatInt | JitValueType::FlatFloat);
                if is_flat(*src) || is_flat(*dst) {
                    return Err(JitError(
                        "Move: flat-array registers cannot be moved".into(),
                    ));
                }
                if class(*dst) != class(*src) {
                    return Err(JitError(format!(
                        "Move: classes differ ({:?} vs {:?})",
                        class(*dst),
                        class(*src)
                    )));
                }
            }
            JitInstr::Add { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Add")?,
            JitInstr::Sub { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Sub")?,
            JitInstr::Mul { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Mul")?,
            JitInstr::Div { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Div")?,
            JitInstr::Mod { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Mod")?,
            JitInstr::IntToFloat { dst, src } => {
                require_class(*src, JitValueType::Int, "IntToFloat src")?;
                require_class(*dst, JitValueType::Float, "IntToFloat result")?;
            }
            JitInstr::HostCall { helper, dst, args } => {
                let sig = helper.signature();
                if args.len() != sig.args.len() {
                    return Err(JitError(format!(
                        "HostCall {helper:?}: got {} args, expected {}",
                        args.len(),
                        sig.args.len()
                    )));
                }
                for (i, (arg, expected)) in args.iter().zip(sig.args.iter()).enumerate() {
                    match arg {
                        HostArg::Reg(reg) => {
                            require_class(*reg, *expected, &format!("HostCall {helper:?} arg {i}"))?
                        }
                        HostArg::ImmI64(_) => {
                            if *expected != JitValueType::Int {
                                return Err(JitError(format!(
                                    "HostCall {helper:?} arg {i}: immediate used for {expected:?}"
                                )));
                            }
                        }
                    }
                }
                match sig.result {
                    HostResult::Exact(result) => {
                        require_class(*dst, result, &format!("HostCall {helper:?} result"))?;
                    }
                    HostResult::IntOrFloatBits => {
                        check_reg(*dst)?;
                        if !matches!(class(*dst), JitValueType::Int | JitValueType::Float) {
                            return Err(JitError(format!(
                                "HostCall {helper:?} result: register {dst} is {:?}, expected Int or Float",
                                class(*dst)
                            )));
                        }
                    }
                }
            }
            JitInstr::MemoizedHostCall {
                helper,
                dst,
                args,
                cache,
                flag,
            } => {
                let sig = helper.signature();
                if helper.heap_effect().writes_existing_heap() {
                    return Err(JitError(format!(
                        "MemoizedHostCall {helper:?}: heap-writing helpers cannot be memoized"
                    )));
                }
                if args.len() != sig.args.len() {
                    return Err(JitError(format!(
                        "MemoizedHostCall {helper:?}: got {} args, expected {}",
                        args.len(),
                        sig.args.len()
                    )));
                }
                for (i, (arg, expected)) in args.iter().zip(sig.args.iter()).enumerate() {
                    match arg {
                        HostArg::Reg(reg) => require_class(
                            *reg,
                            *expected,
                            &format!("MemoizedHostCall {helper:?} arg {i}"),
                        )?,
                        HostArg::ImmI64(_) => {
                            if *expected != JitValueType::Int {
                                return Err(JitError(format!(
                                    "MemoizedHostCall {helper:?} arg {i}: immediate used for {expected:?}"
                                )));
                            }
                        }
                    }
                }
                require_class(*flag, JitValueType::Int, "MemoizedHostCall flag")?;
                match sig.result {
                    HostResult::Exact(result) => {
                        require_class(
                            *dst,
                            result,
                            &format!("MemoizedHostCall {helper:?} result"),
                        )?;
                        require_class(
                            *cache,
                            result,
                            &format!("MemoizedHostCall {helper:?} cache"),
                        )?;
                    }
                    HostResult::IntOrFloatBits => {
                        check_reg(*dst)?;
                        check_reg(*cache)?;
                        if !matches!(class(*dst), JitValueType::Int | JitValueType::Float) {
                            return Err(JitError(format!(
                                "MemoizedHostCall {helper:?} result: register {dst} is {:?}, expected Int or Float",
                                class(*dst)
                            )));
                        }
                        if class(*cache) != class(*dst) {
                            return Err(JitError(format!(
                                "MemoizedHostCall {helper:?} cache: class {:?} does not match result {:?}",
                                class(*cache),
                                class(*dst)
                            )));
                        }
                    }
                }
            }
            JitInstr::CallNative { dst, args, .. } => {
                check_reg(*dst)?;
                if matches!(class(*dst), JitValueType::FlatInt | JitValueType::FlatFloat) {
                    return Err(JitError(format!(
                        "CallNative result register {dst} is a flat-array register"
                    )));
                }
                for arg in args {
                    check_reg(*arg)?;
                }
            }
            JitInstr::MatchMapGetInt {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require_class(*map, JitValueType::Handle, "MatchMapGetInt map")?;
                require_class(*key, JitValueType::Int, "MatchMapGetInt key")?;
                require_class(*value_dst, JitValueType::Int, "MatchMapGetInt value")?;
                check_target(*some_ip)?;
                check_target(*none_ip)?;
            }
            JitInstr::MatchSortedMapGetInt {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require_class(*map, JitValueType::Handle, "MatchSortedMapGetInt map")?;
                require_class(*key, JitValueType::Int, "MatchSortedMapGetInt key")?;
                require_class(*value_dst, JitValueType::Int, "MatchSortedMapGetInt value")?;
                check_target(*some_ip)?;
                check_target(*none_ip)?;
            }
            JitInstr::BitAnd { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitAnd")?,
            JitInstr::BitOr { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitOr")?,
            JitInstr::BitXor { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitXor")?,
            JitInstr::Shl { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Shl")?,
            JitInstr::Shr { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Shr")?,
            JitInstr::Compare { dst, lhs, rhs, .. } => compare(*dst, *lhs, *rhs, "Compare")?,
            JitInstr::Equal { dst, lhs, rhs } => compare(*dst, *lhs, *rhs, "Equal")?,
            JitInstr::NotEqual { dst, lhs, rhs } => compare(*dst, *lhs, *rhs, "NotEqual")?,
            JitInstr::Jump { target } => check_target(*target)?,
            JitInstr::JumpIfBool { cond, target, .. } => {
                require_class(*cond, JitValueType::Int, "JumpIfBool")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::ProfiledJumpIfBool { cond, target, .. } => {
                require_class(*cond, JitValueType::Int, "ProfiledJumpIfBool")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::JumpIfIntCompare {
                lhs, rhs, target, ..
            } => {
                scalar_pair(*lhs, *rhs, "JumpIfIntCompare")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::ProfiledJumpIfIntCompare {
                lhs, rhs, target, ..
            } => {
                scalar_pair(*lhs, *rhs, "ProfiledJumpIfIntCompare")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::Return { src } => {
                check_reg(*src)?;
                // Heap-result return ABI: a scalar (`Int`/`Float`) or a
                // `Handle` register may be returned. A `Handle` return's i64 is an
                // opaque output-table handle (the host materializes a heap value from
                // it); see [`NativeOutcome::CompletedHandle`]. A FLAT-array register is
                // still rejected — it is a (pointer, length) pair, not a single
                // returnable word.
                if matches!(class(*src), JitValueType::FlatInt | JitValueType::FlatFloat) {
                    return Err(JitError(
                        "Return: cannot return a flat-array register".into(),
                    ));
                }
            }
            JitInstr::ListGetIntDirect { dst, base, index } => {
                require_flat_param(*base, JitValueType::FlatInt, "ListGetIntDirect base")?;
                require_class(*index, JitValueType::Int, "ListGetIntDirect index")?;
                require_class(*dst, JitValueType::Int, "ListGetIntDirect result")?;
            }
            JitInstr::ListSetIntDirect {
                dst,
                base,
                index,
                value,
            } => {
                require_flat_param(*base, JitValueType::FlatInt, "ListSetIntDirect base")?;
                require_class(*index, JitValueType::Int, "ListSetIntDirect index")?;
                require_class(*value, JitValueType::Int, "ListSetIntDirect value")?;
                require_class(*dst, JitValueType::Int, "ListSetIntDirect result")?;
            }
            JitInstr::ListGetFloatDirect { dst, base, index } => {
                require_flat_param(*base, JitValueType::FlatFloat, "ListGetFloatDirect base")?;
                require_class(*index, JitValueType::Int, "ListGetFloatDirect index")?;
                require_class(*dst, JitValueType::Float, "ListGetFloatDirect result")?;
            }
            JitInstr::ListLenDirect { dst, base } => {
                check_reg(*base)?;
                if !matches!(
                    class(*base),
                    JitValueType::FlatInt | JitValueType::FlatFloat
                ) {
                    return Err(JitError(format!(
                        "ListLenDirect base: register {base} is {:?}, expected a flat-array param",
                        class(*base)
                    )));
                }
                let window = if osr {
                    program.n_regs as usize
                } else {
                    program.n_params as usize
                };
                if (*base as usize) >= window {
                    return Err(JitError(format!(
                        "ListLenDirect base: register {base} is not a parameter"
                    )));
                }
                require_class(*dst, JitValueType::Int, "ListLenDirect result")?;
            }
            JitInstr::GuardClosureId { base, expected } => {
                require_class(*base, JitValueType::Handle, "GuardClosureId base")?;
                if *expected < 0 {
                    return Err(JitError(format!(
                        "GuardClosureId expected: {expected} is not a valid function id"
                    )));
                }
            }
            // OSR-exit is a parameterless terminator (an unconditional deopt at its
            // own ip); its live set is computed from definite-assignment, so it
            // carries no operands to validate.
            JitInstr::OsrExit => {}
        }
    }
    Ok(())
}

/// The register an instruction definitely writes (its `dst`), if any. Control
/// instructions (`Return`/`Jump`/`JumpIf*`/`Bail`) and `Nop` write nothing.
fn instr_def(instr: &JitInstr) -> Option<u32> {
    match instr {
        JitInstr::LoadInt { dst, .. }
        | JitInstr::LoadFloat { dst, .. }
        | JitInstr::LoadBool { dst, .. }
        | JitInstr::Move { dst, .. }
        | JitInstr::Add { dst, .. }
        | JitInstr::Sub { dst, .. }
        | JitInstr::Mul { dst, .. }
        | JitInstr::Div { dst, .. }
        | JitInstr::Mod { dst, .. }
        | JitInstr::IntToFloat { dst, .. }
        | JitInstr::HostCall { dst, .. }
        | JitInstr::MemoizedHostCall { dst, .. }
        | JitInstr::CallNative { dst, .. }
        | JitInstr::BitAnd { dst, .. }
        | JitInstr::BitOr { dst, .. }
        | JitInstr::BitXor { dst, .. }
        | JitInstr::Shl { dst, .. }
        | JitInstr::Shr { dst, .. }
        | JitInstr::Compare { dst, .. }
        | JitInstr::Equal { dst, .. }
        | JitInstr::NotEqual { dst, .. }
        | JitInstr::MatchMapGetInt { value_dst: dst, .. }
        | JitInstr::MatchSortedMapGetInt { value_dst: dst, .. }
        | JitInstr::ListGetIntDirect { dst, .. }
        | JitInstr::ListSetIntDirect { dst, .. }
        | JitInstr::ListGetFloatDirect { dst, .. }
        | JitInstr::ListLenDirect { dst, .. } => Some(*dst),
        JitInstr::Nop
        | JitInstr::Jump { .. }
        | JitInstr::JumpIfBool { .. }
        | JitInstr::JumpIfIntCompare { .. }
        | JitInstr::ProfiledJumpIfBool { .. }
        | JitInstr::ProfiledJumpIfIntCompare { .. }
        | JitInstr::Return { .. }
        | JitInstr::GuardClosureId { .. }
        | JitInstr::OsrExit
        | JitInstr::Bail => None,
    }
}

/// The control-flow successors of instruction `i` (indices into `program.code`):
/// fallthrough to `i + 1` unless `i` is an unconditional `Jump`; conditional
/// branches add their target; `Jump` goes only to its target; `Return`/`Bail` (and
/// running off the end) go nowhere. Out-of-range targets are dropped — `validate`
/// rejects those before codegen, and the analysis stays total regardless.
fn successors(program: &JitFunction, i: usize) -> Vec<usize> {
    let n = program.code.len();
    let in_range = |t: u32| (t as usize) < n;
    let next = i + 1;
    match &program.code[i] {
        JitInstr::Jump { target } => {
            if in_range(*target) {
                vec![*target as usize]
            } else {
                vec![]
            }
        }
        JitInstr::JumpIfBool { target, .. }
        | JitInstr::JumpIfIntCompare { target, .. }
        | JitInstr::ProfiledJumpIfBool { target, .. }
        | JitInstr::ProfiledJumpIfIntCompare { target, .. } => {
            let mut succ = Vec::new();
            if next < n {
                succ.push(next);
            }
            if in_range(*target) {
                succ.push(*target as usize);
            }
            succ
        }
        JitInstr::MatchMapGetInt {
            some_ip, none_ip, ..
        }
        | JitInstr::MatchSortedMapGetInt {
            some_ip, none_ip, ..
        } => {
            let mut succ = Vec::new();
            if in_range(*some_ip) {
                succ.push(*some_ip as usize);
            }
            if in_range(*none_ip) {
                succ.push(*none_ip as usize);
            }
            succ
        }
        JitInstr::Return { .. } | JitInstr::Bail | JitInstr::OsrExit => vec![],
        _ => {
            if next < n {
                vec![next]
            } else {
                vec![]
            }
        }
    }
}

/// Definite-assignment ("must") analysis over the [`JitInstr`] CFG. Returns, per
/// instruction index `i`, the set (as a `reg -> bool` vector of width `n_regs`) of
/// registers **definitely assigned on entry to `i`** — i.e. assigned on *every*
/// path from the function entry to `i`.
///
/// Lattice: forward must-analysis. The entry-to-instruction-0 set is the parameter
/// registers `0..n_params`. `assigned_out[i] = assigned_in[i] ∪ defs(i)`, and for a
/// non-entry instruction `assigned_in[j] = ⋂ assigned_out[p]` over predecessors `p`
/// (intersection — a register is live on entry only if every incoming path assigns
/// it). Non-entry `assigned_in` starts at the full set and the intersection shrinks
/// it to the fixpoint; instruction 0's entry set is the params and is never
/// intersected down.
fn definite_assignment(program: &JitFunction) -> Vec<Vec<bool>> {
    let n = program.code.len();
    let n_regs = program.n_regs as usize;
    if n == 0 {
        return Vec::new();
    }

    // Predecessor lists, derived from the forward CFG.
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        for s in successors(program, i) {
            preds[s].push(i);
        }
    }

    // Entry set for instruction 0: the parameters are assigned, nothing else.
    let mut entry0 = vec![false; n_regs];
    for slot in entry0
        .iter_mut()
        .take((program.n_params as usize).min(n_regs))
    {
        *slot = true;
    }

    // `assigned_in[0]` is pinned to the params; every other block starts at the
    // full (all-true) set so intersection can only shrink it toward the fixpoint.
    let mut assigned_in: Vec<Vec<bool>> = (0..n)
        .map(|i| {
            if i == 0 {
                entry0.clone()
            } else {
                vec![true; n_regs]
            }
        })
        .collect();

    let out_of = |in_set: &[bool], i: usize| -> Vec<bool> {
        let mut out = in_set.to_vec();
        if let Some(d) = instr_def(&program.code[i])
            && (d as usize) < n_regs
        {
            out[d as usize] = true;
        }
        out
    };

    // Iterate to a fixpoint. Intersection is monotone (only clears bits), so the
    // loop terminates in at most `n_regs * n` bit-clears.
    let mut changed = true;
    while changed {
        changed = false;
        for j in 1..n {
            if preds[j].is_empty() {
                // Unreachable block: leave at the full set (it has no resume site of
                // its own that we rely on; its inputs are vacuously satisfied).
                continue;
            }
            let mut new_in = vec![true; n_regs];
            for &p in &preds[j] {
                let out = out_of(&assigned_in[p], p);
                for r in 0..n_regs {
                    new_in[r] = new_in[r] && out[r];
                }
            }
            if new_in != assigned_in[j] {
                assigned_in[j] = new_in;
                changed = true;
            }
        }
    }

    assigned_in
}

/// A sound integer interval `[lo, hi]` (inclusive) over `i128`, an
/// over-approximation of the set of `i64` values an Int register may hold. Held in
/// `i128` so the analysis arithmetic itself never overflows. `TOP` is the full
/// `i64` range — the safe default for any value we cannot track precisely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Interval {
    lo: i128,
    hi: i128,
}

impl Interval {
    const TOP: Interval = Interval {
        lo: i64::MIN as i128,
        hi: i64::MAX as i128,
    };

    fn constant(c: i64) -> Interval {
        Interval {
            lo: c as i128,
            hi: c as i128,
        }
    }

    /// Convex hull (union over-approximation) of two intervals.
    fn hull(self, other: Interval) -> Interval {
        Interval {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// True iff every value in `[lo, hi]` fits in an `i64`. When this holds for the
    /// *result* interval of an Int op, that op provably cannot overflow.
    fn fits_i64(self) -> bool {
        self.lo >= i64::MIN as i128 && self.hi <= i64::MAX as i128
    }
}

/// Forward integer-interval ("range") analysis over the Int registers of a native
/// function. Returns, per instruction index `i`, an `i128` interval per register
/// that **soundly over-approximates** every value the register may hold on entry to
/// `i`. Used solely to prove — conservatively, at compile time — that a particular
/// `Add`/`Sub`/`Mul` cannot overflow, so the checked `*_overflow` + bail can be
/// replaced by a plain `iadd`/`isub`/`imul` with byte-identical results.
///
/// Soundness is the whole point: every transfer function is an over-approximation,
/// and any register/operation we cannot prove a finite bound for is `TOP`
/// (`[i64::MIN, i64::MAX]`), which forces the checked path. Non-Int registers carry
/// `TOP` and are never consulted. The lattice has height 3 per register
/// (constant-derived range ⊑ wider range ⊑ TOP); a widening at every join jumps any
/// register whose merged interval grew straight to `TOP`, which both guarantees
/// termination (no infinite ascending chains across loop back-edges — an
/// unbounded-in-a-loop register widens to `TOP`, the safe answer) and keeps the
/// analysis cheap.
fn interval_analysis(program: &JitFunction) -> Vec<Vec<Interval>> {
    let n = program.code.len();
    let n_regs = program.n_regs as usize;
    if n == 0 {
        return Vec::new();
    }

    let is_int = |r: u32| program.reg_types[r as usize] == JitValueType::Int;
    // Read an operand's interval from an in-set: TOP for any non-Int register (we
    // only reason about Int arithmetic) and for out-of-range indices.
    let read = |set: &[Interval], r: u32| -> Interval {
        if is_int(r) {
            set.get(r as usize).copied().unwrap_or(Interval::TOP)
        } else {
            Interval::TOP
        }
    };

    // Predecessor lists from the forward CFG (same shape as definite_assignment).
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        for s in successors(program, i) {
            preds[s].push(i);
        }
    }

    // Transfer function: given the in-set at `i`, produce the out-set. Only the
    // proven cases narrow below TOP; everything else stays/becomes TOP.
    let out_of = |in_set: &[Interval], i: usize| -> Vec<Interval> {
        let mut out = in_set.to_vec();
        let set = |out: &mut Vec<Interval>, r: u32, v: Interval| {
            if (r as usize) < n_regs {
                // Collapse any range that doesn't fit i64 straight to TOP. Such a
                // range can never prove an op safe anyway, and clamping here keeps the
                // analysis's own i128 arithmetic bounded (a register always holds an
                // i64-fitting range or TOP, so downstream corner products stay well
                // within i128). Also never claim a tighter bound than the register's
                // storage class allows.
                out[r as usize] = if is_int(r) && v.fits_i64() {
                    v
                } else {
                    Interval::TOP
                };
            }
        };
        match &program.code[i] {
            JitInstr::LoadInt { dst, value } => set(&mut out, *dst, Interval::constant(*value)),
            JitInstr::LoadBool { dst, value } => {
                set(&mut out, *dst, Interval::constant(i64::from(*value)));
            }
            JitInstr::Move { dst, src } => {
                let v = read(in_set, *src);
                set(&mut out, *dst, v);
            }
            // A list/array length is a non-negative element count; we soundly bound
            // it to `[0, i64::MAX]` and deliberately assume NO tighter upper bound.
            JitInstr::ListLenDirect { dst, .. }
            | JitInstr::HostCall {
                helper: HostHelper::ListLen,
                dst,
                ..
            } => {
                set(
                    &mut out,
                    *dst,
                    Interval {
                        lo: 0,
                        hi: i64::MAX as i128,
                    },
                );
            }
            JitInstr::Add { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                set(
                    &mut out,
                    *dst,
                    Interval {
                        lo: a.lo + b.lo,
                        hi: a.hi + b.hi,
                    },
                );
            }
            JitInstr::Sub { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                set(
                    &mut out,
                    *dst,
                    Interval {
                        lo: a.lo - b.hi,
                        hi: a.hi - b.lo,
                    },
                );
            }
            JitInstr::Mul { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                // Product range is the hull of the four corner products (i128, so
                // the proof arithmetic cannot itself overflow for i64 operands).
                let c1 = a.lo * b.lo;
                let c2 = a.lo * b.hi;
                let c3 = a.hi * b.lo;
                let c4 = a.hi * b.hi;
                let lo = c1.min(c2).min(c3).min(c4);
                let hi = c1.max(c2).max(c3).max(c4);
                set(&mut out, *dst, Interval { lo, hi });
            }
            // Every other definer produces an untracked value ⇒ TOP. (Covers Div,
            // Mod, bitops, shifts, compares, heap reads, ClosureId, params, etc.)
            other => {
                if let Some(d) = instr_def(other) {
                    set(&mut out, d, Interval::TOP);
                }
            }
        }
        out
    };

    // Edge-sensitive (branch-conditioned) refinement (J4.3+). When predecessor `p`
    // is a `JumpIfIntCompare` and the edge `p -> succ` is governed by a comparison
    // fact, tighten the operand intervals flowed along that *specific* edge by the
    // asserted relation. This is what lets a loop counter `i` — TOP at the loop
    // header after the join — be proven `<= N - 1` on the loop-body edge (the guard's
    // taken edge), so the body's `i = i + 1` provably fits i64.
    //
    // Soundness: each rule narrows an interval only to values that genuinely satisfy
    // the asserted relation; all arithmetic is in i128 so it cannot overflow; if a
    // refinement would invert an interval (`lo > hi`) the edge is unreachable, so we
    // leave the operand at its un-refined (still sound) value rather than emit a
    // malformed interval. We refine ONLY the cmps/edges enumerated below; everything
    // else flows un-refined. The join still hulls + widens to TOP (termination is
    // unaffected — refinement only narrows along an edge, never grows the lattice).
    let refine_edge = |out: &mut Vec<Interval>, p: usize, succ: usize| {
        let (lhs, rhs, op, expected, target) = match &program.code[p] {
            JitInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            }
            | JitInstr::ProfiledJumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
                ..
            } => (*lhs, *rhs, *op, *expected, *target),
            _ => return,
        };
        if !is_int(lhs) || !is_int(rhs) {
            return;
        }
        // Is this edge the one on which `(lhs op rhs)` holds, or its negation?
        // The target edge is taken when `(lhs op rhs) == expected`.
        let is_target = succ == target as usize;
        // Could this edge also be the fall-through? (e.g. target == next). If the
        // edge is ambiguous (both target and fall-through reach `succ`), we cannot
        // attribute a single fact to it — flow un-refined (sound).
        let is_fallthrough = succ == p + 1;
        if is_target && is_fallthrough {
            return;
        }
        if !is_target && !is_fallthrough {
            return;
        }
        // The relation that holds on THIS edge: on the target edge it is the raw
        // comparison iff `expected` is true; on the fall-through edge it is the
        // negation of "comparison == expected".
        let cond_holds = if is_target { expected } else { !expected };
        // `cond_holds == true`  => `lhs op rhs`
        // `cond_holds == false` => `!(lhs op rhs)`
        let (a, b) = (out[lhs as usize], out[rhs as usize]);
        // Apply a refinement to register `r`, intersecting with [lo, hi]; if the
        // intersection inverts, the edge is unreachable -> leave un-refined (sound).
        let apply = |out: &mut Vec<Interval>, r: u32, lo: i128, hi: i128| {
            let cur = out[r as usize];
            let nlo = cur.lo.max(lo);
            let nhi = cur.hi.min(hi);
            if nlo <= nhi {
                out[r as usize] = Interval { lo: nlo, hi: nhi };
            }
        };
        // Effective relation between lhs and rhs that holds on this edge, expressed
        // as one of the four primitive forms by folding `cond_holds`.
        // Lt: lhs <  rhs ;  !Lt => lhs >= rhs
        // Le: lhs <= rhs ;  !Le => lhs >  rhs
        // Gt: lhs >  rhs ;  !Gt => lhs <= rhs
        // Ge: lhs >= rhs ;  !Ge => lhs <  rhs
        use JitCompare::*;
        // Normalize to: does `lhs < rhs`, `lhs <= rhs`, `lhs > rhs`, or `lhs >= rhs`
        // hold on this edge?
        let rel = match (op, cond_holds) {
            (Lt, true) | (Ge, false) => Lt,
            (Le, true) | (Gt, false) => Le,
            (Gt, true) | (Le, false) => Gt,
            (Ge, true) | (Lt, false) => Ge,
        };
        match rel {
            // lhs < rhs: lhs.hi <= rhs.hi - 1 ; rhs.lo >= lhs.lo + 1
            Lt => {
                apply(out, lhs, i64::MIN as i128, b.hi - 1);
                apply(out, rhs, a.lo + 1, i64::MAX as i128);
            }
            // lhs <= rhs: lhs.hi <= rhs.hi ; rhs.lo >= lhs.lo
            Le => {
                apply(out, lhs, i64::MIN as i128, b.hi);
                apply(out, rhs, a.lo, i64::MAX as i128);
            }
            // lhs > rhs: lhs.lo >= rhs.lo + 1 ; rhs.hi <= lhs.hi - 1
            Gt => {
                apply(out, lhs, b.lo + 1, i64::MAX as i128);
                apply(out, rhs, i64::MIN as i128, a.hi - 1);
            }
            // lhs >= rhs: lhs.lo >= rhs.lo ; rhs.hi <= lhs.hi
            Ge => {
                apply(out, lhs, b.lo, i64::MAX as i128);
                apply(out, rhs, i64::MIN as i128, a.hi);
            }
        }
    };

    // Entry to instruction 0: params are untracked (TOP); everything else TOP too.
    // Every per-instruction register starts at TOP and the analysis narrows it only
    // where a definer/merge proves a bound. `initialized[j]` tracks whether block
    // `j`'s in-set has been computed from predecessors at least once (the first such
    // pass *replaces* the all-TOP seed rather than hulling against it).
    let mut interval_in: Vec<Vec<Interval>> = (0..n).map(|_| vec![Interval::TOP; n_regs]).collect();
    let mut initialized = vec![false; n];
    // Sticky-widened marker per (instruction, register): once a register at block
    // `j` widens to TOP (e.g. across a loop back-edge), it is pinned to TOP and
    // never narrowed again. This makes each register monotone — narrowed at most
    // once, then only ever widened to the sticky TOP — which guarantees the fixpoint
    // terminates (no narrow/widen oscillation) and keeps any unbounded-in-a-loop
    // register at the safe TOP. Lattice height per register ≤ 3.
    let mut pinned_top: Vec<Vec<bool>> = (0..n).map(|_| vec![false; n_regs]).collect();

    // PHASE 1 — plain monotone fixpoint (unchanged J4.3 lattice). Branch refinement
    // is deliberately NOT applied here: it is a non-propagating, query-time narrowing
    // (Phase 2 below). Keeping it out of the fixpoint preserves the original
    // termination argument exactly (height-3 lattice + sticky-TOP pinning) — a refined
    // bound never feeds back across a loop back-edge, so there is no slow descending
    // ratchet, and the loop still converges in O(n_regs * n) passes.
    let mut changed = true;
    while changed {
        changed = false;
        for j in 0..n {
            // Instruction 0 and any predecessor-less block keep the all-TOP entry,
            // which is sound (params and unreachable-block inputs are untracked). Mark
            // it initialized so it counts as a real predecessor for the optimistic
            // join below (its all-TOP in-set is its final, correct value).
            if preds[j].is_empty() {
                initialized[j] = true;
                continue;
            }
            let bottom = Interval {
                lo: i128::MAX,
                hi: i128::MIN,
            };
            let mut new_in = vec![bottom; n_regs];
            // Optimistic worklist join: only hull predecessors whose own in-set has
            // been computed at least once. A not-yet-`initialized` predecessor still
            // carries the all-TOP seed (e.g. an unvisited loop back-edge on the first
            // pass); folding that seed in would prematurely widen loop-invariant
            // registers (like a constant increment) to TOP and never recover. Treating
            // it as bottom until it is real is the standard monotone worklist; once it
            // is initialized it joins normally, and the widening below still caps any
            // genuine loop-carried growth, so termination is unaffected.
            let mut any_pred = false;
            for &p in &preds[j] {
                if !initialized[p] {
                    continue;
                }
                any_pred = true;
                let out = out_of(&interval_in[p], p);
                for r in 0..n_regs {
                    new_in[r] = new_in[r].hull(out[r]);
                }
            }
            // No initialized predecessor yet (e.g. a block reachable only via a
            // back-edge whose source hasn't been seen): leave it for a later pass
            // rather than adopt the malformed bottom seed.
            if !any_pred {
                continue;
            }
            let first = !initialized[j];
            for r in 0..n_regs {
                if pinned_top[j][r] {
                    new_in[r] = Interval::TOP;
                    continue;
                }
                let merged = new_in[r];
                if first {
                    new_in[r] = merged;
                    continue;
                }
                let old = interval_in[j][r];
                // If the merged interval grew strictly wider than the current value
                // (e.g. across a loop back-edge), widen straight to TOP and pin it.
                if merged.lo < old.lo || merged.hi > old.hi {
                    new_in[r] = Interval::TOP;
                    pinned_top[j][r] = true;
                } else {
                    new_in[r] = merged;
                }
            }
            if new_in != interval_in[j] || first {
                interval_in[j] = new_in;
                initialized[j] = true;
                changed = true;
            }
        }
    }

    // PHASE 2 — branch-conditioned refinement (J4.3+), propagated through
    // single-predecessor body chains but NOT across joins.
    //
    // A loop counter is tightened on the loop GUARD's body edge (e.g. `i <= N - 1`),
    // but the increment `i = i + 1` may sit a few straight-line instructions later, so
    // the refined bound must flow forward to it. We therefore recompute a refined
    // in-set per block:
    //   * a MULTI-predecessor block (a join: loop header, if/else merge) is PINNED to
    //     its Phase-1 widened value — refinement never enters or crosses a join, so the
    //     widening/termination story is exactly Phase 1's;
    //   * a SINGLE-predecessor block `j` (pred `p`) takes `transfer(refined[p])` and
    //     then intersects the branch fact asserted on the `p -> j` edge.
    // Because every CFG cycle passes through a join (a loop header has ≥2 preds: entry
    // + back-edge), and joins are pinned to a fixed value, the refined overlay has NO
    // cycles — it is a DAG rooted at the pinned joins, so this fixpoint converges in at
    // most `n` ordered passes. Every value is `⊑` its Phase-1 counterpart (refinement
    // and the transfer only narrow off a sound base), so the result stays a sound
    // over-approximation. `refine_edge` already drops any inverted (unreachable-edge)
    // refinement, so no malformed interval is ever produced.
    let multi_pred: Vec<bool> = (0..n)
        .map(|j| preds[j].iter().filter(|&&p| initialized[p]).count() > 1)
        .collect();
    let mut refined_in = interval_in.clone();
    // DAG depth ≤ n, so `n + 1` ordered sweeps reach the fixpoint; the loop also exits
    // early once nothing changes.
    for _ in 0..=n {
        let mut changed = false;
        for j in 0..n {
            if multi_pred[j] || preds[j].is_empty() {
                // Joins and entry/unreachable blocks keep their Phase-1 value.
                continue;
            }
            // Exactly one (initialized) predecessor: attribute the edge fact to it.
            let Some(&p) = preds[j].iter().find(|&&p| initialized[p]) else {
                continue;
            };
            let mut row = out_of(&refined_in[p], p);
            refine_edge(&mut row, p, j);
            if row != refined_in[j] {
                refined_in[j] = row;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    refined_in
}

/// Whether the result of the `Add`/`Sub`/`Mul` at instruction `i` provably fits in
/// `i64` given the operand intervals on entry — i.e. the op cannot overflow, so the
/// checked `*_overflow` + bail may be replaced by a plain unchecked op with
/// identical results. Conservative: any TOP operand (or a non-Int op) makes the
/// result range TOP, which does NOT fit, so the checked path is kept. The result
/// interval is computed in `i128` exactly as the analysis transfer function does.
fn arith_cannot_overflow(intervals: &[Interval], instr: &JitInstr) -> bool {
    let get = |r: u32| intervals.get(r as usize).copied().unwrap_or(Interval::TOP);
    let result = match instr {
        JitInstr::Add { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            Interval {
                lo: a.lo + b.lo,
                hi: a.hi + b.hi,
            }
        }
        JitInstr::Sub { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            Interval {
                lo: a.lo - b.hi,
                hi: a.hi - b.lo,
            }
        }
        JitInstr::Mul { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            let c1 = a.lo * b.lo;
            let c2 = a.lo * b.hi;
            let c3 = a.hi * b.lo;
            let c4 = a.hi * b.hi;
            Interval {
                lo: c1.min(c2).min(c3).min(c4),
                hi: c1.max(c2).max(c3).max(c4),
            }
        }
        _ => return false,
    };
    result.fits_i64()
}

#[allow(clippy::too_many_arguments)]
fn build_function(
    func: &mut cranelift_codegen::ir::Function,
    fbctx: &mut FunctionBuilderContext,
    module: &mut JITModule,
    imports: HostFuncs,
    program: &JitFunction,
    forced: Option<ForcedDeopt>,
    osr_header: Option<u32>,
    native_callees: &[NativeCallee],
) -> Result<DeoptMap, JitError> {
    // Definite-assignment ("must") sets per instruction, computed once up front so
    // each bail site can record its live (entry-assigned) registers. Purely
    // host-side analysis — it shapes no emitted code.
    let assigned_in = definite_assignment(program);
    // Forward integer-interval analysis (J4.3): per-instruction sound ranges for
    // Int registers, used purely to elide provably-non-overflowing checks. Like
    // `definite_assignment`, host-side only — it shapes no code beyond choosing the
    // checked vs unchecked arithmetic form.
    let intervals = interval_analysis(program);
    // Sites accumulate in emission order, aligned 1:1 with the `next_id` counter
    // (`sites[id - 1]` is the site for id `id`).
    let mut sites: Vec<DeoptSite> = Vec::new();
    let mut payload_words = program.n_regs as usize;

    let mut bcx = FunctionBuilder::new(func, fbctx);
    let ptr_ty = module.target_config().pointer_type();

    // Per-function references to the imported host helpers (heap reads call these).
    let helper_refs: Vec<_> = HostHelper::all()
        .iter()
        .map(|&helper| {
            (
                helper,
                module.declare_func_in_func(imports.get(helper), bcx.func),
            )
        })
        .collect();
    let native_refs: Vec<_> = native_callees
        .iter()
        .map(|callee| {
            (
                callee.handle,
                module.declare_func_in_func(callee.func_id, bcx.func),
            )
        })
        .collect();

    let n = program.code.len();
    let n_regs = program.n_regs as usize;

    // One Cranelift variable per VM register, typed by storage class (i64 for
    // integers/booleans, f64 for floats).
    let var_ty = |reg: usize| {
        if program.reg_types[reg] == JitValueType::Float {
            types::F64
        } else {
            types::I64
        }
    };
    let vars: Vec<Variable> = (0..n_regs).map(|i| bcx.declare_var(var_ty(i))).collect();

    // Entry: read params from the args array, zero the rest, then jump to the
    // block for instruction 0. Args are passed as raw 64-bit words; loading a
    // float register's slot as `f64` reinterprets the caller's `f64::to_bits`.
    let entry = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let params = bcx.block_params(entry).to_vec();
    let args_ptr = params[0];
    let lens_ptr = params[2];
    let host_ctx = params[3];
    let out_ptr = params[4];
    let bail_ptr = params[5];
    let safepoint_ptr = params[6];
    let payload_ptr = params[7];
    // Running per-site bail-id counter. Starts at 1 (0 is reserved = no bail);
    // `bail_if` post-increments it so every guard/bail site gets a stable id.
    let mut next_id: i64 = 1;
    // The set of registers to load from the entry window. For a normal compile this
    // is the parameter registers (`0..n_params`, loaded by index from `args_ptr`).
    // For an OSR-entry it is the loop's *live-in* set: the registers definitely
    // assigned on entry to `header_ip`, loaded by register index from the window
    // (`args_ptr` is the interpreter's `n_regs`-wide register window, not a packed
    // arg array). Every other register is zero-initialized so the var is defined on
    // every path (SSA), exactly as on the normal entry.
    let load_set: Vec<usize> = match osr_header {
        Some(header) => {
            let header = header as usize;
            match assigned_in.get(header) {
                Some(set) => set
                    .iter()
                    .enumerate()
                    .filter(|&(_, &assigned)| assigned)
                    .map(|(r, _)| r)
                    .collect(),
                None => Vec::new(),
            }
        }
        None => (0..program.n_params as usize).collect(),
    };
    let mut is_loaded = vec![false; n_regs];
    for &r in &load_set {
        is_loaded[r] = true;
        let v = bcx
            .ins()
            .load(var_ty(r), MemFlags::trusted(), args_ptr, (r as i32) * 8);
        bcx.def_var(vars[r], v);
    }
    let zero_i = bcx.ins().iconst(types::I64, 0);
    let zero_f = bcx.ins().f64const(0.0);
    for (i, &var) in vars.iter().enumerate().take(n_regs) {
        if is_loaded[i] {
            continue;
        }
        bcx.def_var(
            var,
            if var_ty(i) == types::F64 {
                zero_f
            } else {
                zero_i
            },
        );
    }

    // The shared fallback block: "not completed".
    let fallback = bcx.create_block();

    // Block leaders: index 0, every jump target, and the instruction after any
    // control-transfer (so dead/own-block code never lands in a sealed block).
    let mut is_leader = vec![false; n];
    if n > 0 {
        is_leader[0] = true;
    }
    for (i, instr) in program.code.iter().enumerate() {
        match instr {
            JitInstr::Jump { target } => {
                is_leader[*target as usize] = true;
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
            }
            JitInstr::JumpIfBool { target, .. }
            | JitInstr::JumpIfIntCompare { target, .. }
            | JitInstr::ProfiledJumpIfBool { target, .. }
            | JitInstr::ProfiledJumpIfIntCompare { target, .. } => {
                is_leader[*target as usize] = true;
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
            }
            JitInstr::MatchMapGetInt {
                some_ip, none_ip, ..
            }
            | JitInstr::MatchSortedMapGetInt {
                some_ip, none_ip, ..
            } => {
                is_leader[*some_ip as usize] = true;
                is_leader[*none_ip as usize] = true;
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
            }
            JitInstr::Return { .. } | JitInstr::Bail | JitInstr::OsrExit if i + 1 < n => {
                is_leader[i + 1] = true;
            }
            _ => {}
        }
    }
    for &cold_ip in &program.cold_blocks {
        if (cold_ip as usize) >= n {
            return Err(JitError(format!(
                "cold block instruction {cold_ip} is out of range for {n} instructions"
            )));
        }
    }
    let block_for: Vec<Option<Block>> = (0..n)
        .map(|i| {
            if is_leader[i] {
                Some(bcx.create_block())
            } else {
                None
            }
        })
        .collect();
    for &cold_ip in &program.cold_blocks {
        if let Some(block) = block_for[cold_ip as usize] {
            bcx.set_cold_block(block);
        }
    }

    let reg = |program_reg: u32| vars[program_reg as usize];

    // Fresh per-call deopt context for the instruction currently being lowered
    // (`i`). Each `bail_if` consumes one and pushes one site, so ids and `sites`
    // indices stay in lock-step. A macro (not a closure) so the `&mut sites` borrow
    // lives only for the single `bail_if` call.
    macro_rules! deopt {
        ($ip:expr) => {
            &mut DeoptCtx {
                ip: $ip as u32,
                assigned_in: &assigned_in,
                reg_types: &program.reg_types,
                sites: &mut sites,
                payload_words: &mut payload_words,
                forced,
                unconditional: false,
            }
        };
        ($ip:expr, unconditional) => {
            &mut DeoptCtx {
                ip: $ip as u32,
                assigned_in: &assigned_in,
                reg_types: &program.reg_types,
                sites: &mut sites,
                payload_words: &mut payload_words,
                forced,
                unconditional: true,
            }
        };
    }

    let helper_ref = |helper: HostHelper| {
        helper_refs
            .iter()
            .find_map(|(candidate, func_ref)| (*candidate == helper).then_some(*func_ref))
            .expect("host helper func ref declared")
    };
    let native_ref = |callee: CompiledId| {
        native_refs
            .iter()
            .find_map(|(candidate, func_ref)| (*candidate == callee).then_some(*func_ref))
            .expect("native callee func ref declared")
    };
    let native_callee = |callee: CompiledId| {
        native_callees
            .iter()
            .find(|candidate| candidate.handle == callee)
            .expect("native callee metadata resolved")
    };

    match osr_header {
        // OSR-entry: begin native execution *inside* the loop at the header block.
        // The header is a backedge target, hence a leader, so its block exists; if a
        // caller passes a non-leader (or out-of-range) header we reject cleanly.
        Some(header) => {
            let header = header as usize;
            let target = block_for.get(header).copied().flatten().ok_or_else(|| {
                JitError(format!(
                    "OSR header ip {header} is not a leader / jump-target block"
                ))
            })?;
            bcx.ins().jump(target, &[]);
        }
        None if n == 0 => {
            bcx.ins().jump(fallback, &[]);
        }
        None => {
            bcx.ins().jump(block_for[0].unwrap(), &[]);
        }
    }

    let mut terminated = true;
    for i in 0..n {
        if let Some(b) = block_for[i] {
            if !terminated {
                bcx.ins().jump(b, &[]);
            }
            bcx.switch_to_block(b);
            terminated = false;
        }
        match &program.code[i] {
            JitInstr::Nop => {}
            JitInstr::LoadInt { dst, value } => {
                let v = bcx.ins().iconst(types::I64, *value);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::LoadFloat { dst, value } => {
                let v = bcx.ins().f64const(*value);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::LoadBool { dst, value } => {
                let v = bcx.ins().iconst(types::I64, i64::from(*value));
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Move { dst, src } => {
                let v = bcx.use_var(reg(*src));
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Add { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fadd(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    // Proven non-overflowing by range analysis ⇒ plain unchecked
                    // add, no overflow flag, no bail. Result is byte-identical to the
                    // checked form (which only differs by trapping on overflow).
                    let res = bcx.ins().iadd(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().sadd_overflow(a, b);
                    let cont = bail_if(
                        &mut bcx,
                        of,
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Sub { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fsub(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    let res = bcx.ins().isub(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().ssub_overflow(a, b);
                    let cont = bail_if(
                        &mut bcx,
                        of,
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Mul { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fmul(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    let res = bcx.ins().imul(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().smul_overflow(a, b);
                    let cont = bail_if(
                        &mut bcx,
                        of,
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Div { dst, lhs, rhs } => {
                if program.is_float(*lhs) {
                    // Float division never traps (x/0.0 = ±inf/NaN), matching the
                    // interpreter, so no bail.
                    let a = bcx.use_var(reg(*lhs));
                    let b = bcx.use_var(reg(*rhs));
                    let res = bcx.ins().fdiv(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let res = emit_checked_divrem(
                        &mut bcx,
                        reg(*lhs),
                        reg(*rhs),
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                        &vars,
                        &mut next_id,
                        deopt!(i),
                        false,
                    );
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Mod { dst, lhs, rhs } => {
                // Float modulo is a runtime error in the VM, so only integer
                // registers reach here (eligibility rejects float `%`).
                let res = emit_checked_divrem(
                    &mut bcx,
                    reg(*lhs),
                    reg(*rhs),
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    true,
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::IntToFloat { dst, src } => {
                // `Int.to_float`: signed-int (i64) → f64 value-preserving
                // conversion, identical to the interpreter's `i as f64`. The src
                // var is I64, the dst var F64 (per their register classes).
                let v = bcx.use_var(reg(*src));
                let res = bcx.ins().fcvt_from_sint(types::F64, v);
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::HostCall { helper, dst, args } => {
                let call_args: Vec<Value> = std::iter::once(host_ctx)
                    .chain(args.iter().map(|arg| match arg {
                        HostArg::Reg(arg) => bcx.use_var(reg(*arg)),
                        HostArg::ImmI64(value) => bcx.ins().iconst(types::I64, *value),
                    }))
                    .collect();
                let call = bcx.ins().call(helper_ref(*helper), &call_args);
                let result = bcx.inst_results(call)[0];
                match helper.signature().failure {
                    HostFailureMode::CannotFail => {}
                    HostFailureMode::BailFlag => {
                        let cont = bail_if_helper_failed(
                            &mut bcx,
                            bail_ptr,
                            fallback,
                            safepoint_ptr,
                            payload_ptr,
                            &vars,
                            &mut next_id,
                            deopt!(i),
                        );
                        bcx.switch_to_block(cont);
                    }
                }
                let stored = match helper.signature().result {
                    HostResult::Exact(_) => result,
                    HostResult::IntOrFloatBits if program.is_float(*dst) => {
                        bcx.ins().bitcast(types::F64, MemFlags::new(), result)
                    }
                    HostResult::IntOrFloatBits => result,
                };
                bcx.def_var(reg(*dst), stored);
            }
            JitInstr::MemoizedHostCall {
                helper,
                dst,
                args,
                cache,
                flag,
            } => {
                let cached_block = bcx.create_block();
                let call_block = bcx.create_block();
                let done_block = bcx.create_block();
                let flag_value = bcx.use_var(reg(*flag));
                let zero = bcx.ins().iconst(types::I64, 0);
                let cached = bcx.ins().icmp(IntCC::NotEqual, flag_value, zero);
                bcx.ins().brif(cached, cached_block, &[], call_block, &[]);

                bcx.switch_to_block(cached_block);
                bcx.seal_block(cached_block);
                let cached_value = bcx.use_var(reg(*cache));
                bcx.def_var(reg(*dst), cached_value);
                bcx.ins().jump(done_block, &[]);

                bcx.switch_to_block(call_block);
                bcx.seal_block(call_block);
                let call_args: Vec<Value> = std::iter::once(host_ctx)
                    .chain(args.iter().map(|arg| match arg {
                        HostArg::Reg(arg) => bcx.use_var(reg(*arg)),
                        HostArg::ImmI64(value) => bcx.ins().iconst(types::I64, *value),
                    }))
                    .collect();
                let call = bcx.ins().call(helper_ref(*helper), &call_args);
                let result = bcx.inst_results(call)[0];
                match helper.signature().failure {
                    HostFailureMode::CannotFail => {}
                    HostFailureMode::BailFlag => {
                        let cont = bail_if_helper_failed(
                            &mut bcx,
                            bail_ptr,
                            fallback,
                            safepoint_ptr,
                            payload_ptr,
                            &vars,
                            &mut next_id,
                            deopt!(i),
                        );
                        bcx.switch_to_block(cont);
                    }
                }
                let stored = match helper.signature().result {
                    HostResult::Exact(_) => result,
                    HostResult::IntOrFloatBits if program.is_float(*dst) => {
                        bcx.ins().bitcast(types::F64, MemFlags::new(), result)
                    }
                    HostResult::IntOrFloatBits => result,
                };
                bcx.def_var(reg(*dst), stored);
                bcx.def_var(reg(*cache), stored);
                let one = bcx.ins().iconst(types::I64, 1);
                bcx.def_var(reg(*flag), one);
                bcx.ins().jump(done_block, &[]);

                bcx.switch_to_block(done_block);
                bcx.seal_block(done_block);
            }
            JitInstr::CallNative { callee, dst, args } => {
                let meta = native_callee(*callee);
                let slot_bytes = |words: usize| (words.max(1) * 8) as u32;
                let args_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slot_bytes(meta.n_params),
                    3,
                ));
                let lens_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slot_bytes(meta.n_params),
                    3,
                ));
                let out_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let bail_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    1,
                    0,
                ));
                let safepoint_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let payload_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slot_bytes(meta.deopt_payload_words),
                    3,
                ));

                let zero_i64 = bcx.ins().iconst(types::I64, 0);
                let zero_i8 = bcx.ins().iconst(types::I8, 0);
                bcx.ins().stack_store(zero_i8, bail_slot, 0);
                bcx.ins().stack_store(zero_i64, safepoint_slot, 0);
                for (i_arg, &arg) in args.iter().enumerate() {
                    let value = bcx.use_var(reg(arg));
                    bcx.ins().stack_store(value, args_slot, (i_arg as i32) * 8);
                    let len = if matches!(
                        meta.param_types[i_arg],
                        JitValueType::FlatInt | JitValueType::FlatFloat
                    ) {
                        bcx.ins()
                            .load(types::I64, MemFlags::trusted(), lens_ptr, (arg as i32) * 8)
                    } else {
                        zero_i64
                    };
                    bcx.ins().stack_store(len, lens_slot, (i_arg as i32) * 8);
                }
                let args_ptr_v = bcx.ins().stack_addr(ptr_ty, args_slot, 0);
                let lens_ptr_v = bcx.ins().stack_addr(ptr_ty, lens_slot, 0);
                let out_ptr_v = bcx.ins().stack_addr(ptr_ty, out_slot, 0);
                let bail_ptr_v = bcx.ins().stack_addr(ptr_ty, bail_slot, 0);
                let safepoint_ptr_v = bcx.ins().stack_addr(ptr_ty, safepoint_slot, 0);
                let payload_ptr_v = bcx.ins().stack_addr(ptr_ty, payload_slot, 0);
                let nargs_v = bcx.ins().iconst(ptr_ty, meta.n_params as i64);
                let call = bcx.ins().call(
                    native_ref(*callee),
                    &[
                        args_ptr_v,
                        nargs_v,
                        lens_ptr_v,
                        host_ctx,
                        out_ptr_v,
                        bail_ptr_v,
                        safepoint_ptr_v,
                        payload_ptr_v,
                    ],
                );
                let completed = bcx.inst_results(call)[0];
                let child_bail = bcx.ins().stack_load(types::I8, bail_slot, 0);
                let one_i8 = bcx.ins().iconst(types::I8, 1);
                let zero_i8_again = bcx.ins().iconst(types::I8, 0);
                let not_completed = bcx.ins().icmp(IntCC::NotEqual, completed, one_i8);
                let child_bailed = bcx.ins().icmp(IntCC::NotEqual, child_bail, zero_i8_again);
                let failed = bcx.ins().bor(not_completed, child_bailed);
                let cont = bail_if_child_native_failed(
                    &mut bcx,
                    failed,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    safepoint_ptr_v,
                    payload_ptr_v,
                    meta,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                let result = if meta.return_type == JitValueType::Float {
                    bcx.ins().stack_load(types::F64, out_slot, 0)
                } else {
                    bcx.ins().stack_load(types::I64, out_slot, 0)
                };
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::MatchMapGetInt {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                let map_value = bcx.use_var(reg(*map));
                let key_value = bcx.use_var(reg(*key));
                let loaded = bcx.ins().call(
                    helper_ref(HostHelper::MapGetMatchInt),
                    &[host_ctx, map_value, key_value],
                );
                let value = bcx.inst_results(loaded)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                let found_call = bcx
                    .ins()
                    .call(helper_ref(HostHelper::MapGetMatchFound), &[host_ctx]);
                let found = bcx.inst_results(found_call)[0];

                let some_block = block_for[*some_ip as usize].unwrap();
                let none_block = block_for[*none_ip as usize].unwrap();
                let zero = bcx.ins().iconst(types::I64, 0);
                let is_found = bcx.ins().icmp(IntCC::NotEqual, found, zero);
                bcx.def_var(reg(*value_dst), value);
                bcx.ins().brif(is_found, some_block, &[], none_block, &[]);
                terminated = true;
            }
            JitInstr::MatchSortedMapGetInt {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                let map_value = bcx.use_var(reg(*map));
                let key_value = bcx.use_var(reg(*key));
                let loaded = bcx.ins().call(
                    helper_ref(HostHelper::SortedMapGetInt),
                    &[host_ctx, map_value, key_value],
                );
                let value = bcx.inst_results(loaded)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                let found_call = bcx
                    .ins()
                    .call(helper_ref(HostHelper::SortedMapGetFound), &[host_ctx]);
                let found = bcx.inst_results(found_call)[0];
                let some_block = block_for[*some_ip as usize].unwrap();
                let none_block = block_for[*none_ip as usize].unwrap();
                let zero = bcx.ins().iconst(types::I64, 0);
                let is_found = bcx.ins().icmp(IntCC::NotEqual, found, zero);
                bcx.def_var(reg(*value_dst), value);
                bcx.ins().brif(is_found, some_block, &[], none_block, &[]);
                terminated = true;
            }
            JitInstr::BitAnd { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().band(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::BitOr { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().bor(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::BitXor { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().bxor(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Shl { dst, lhs, rhs } => {
                let res = emit_checked_shift(
                    &mut bcx,
                    reg(*lhs),
                    reg(*rhs),
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    false,
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Shr { dst, lhs, rhs } => {
                let res = emit_checked_shift(
                    &mut bcx,
                    reg(*lhs),
                    reg(*rhs),
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    true,
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Compare { dst, op, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), a, b)
                } else {
                    bcx.ins().icmp(op.cc(), a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::Equal { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(FloatCC::Equal, a, b)
                } else {
                    bcx.ins().icmp(IntCC::Equal, a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::NotEqual { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(FloatCC::NotEqual, a, b)
                } else {
                    bcx.ins().icmp(IntCC::NotEqual, a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::Jump { target } => {
                bcx.ins().jump(block_for[*target as usize].unwrap(), &[]);
                terminated = true;
            }
            JitInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                let c = bcx.use_var(reg(*cond));
                let tb = block_for[*target as usize].unwrap();
                let fb = block_for[i + 1].unwrap();
                if *expected {
                    bcx.ins().brif(c, tb, &[], fb, &[]);
                } else {
                    bcx.ins().brif(c, fb, &[], tb, &[]);
                }
                terminated = true;
            }
            JitInstr::ProfiledJumpIfBool {
                cond,
                expected,
                target,
                hot_target,
            } => {
                let value = bcx.use_var(reg(*cond));
                let zero = bcx.ins().iconst(types::I64, 0);
                let is_true = bcx.ins().icmp(IntCC::NotEqual, value, zero);
                let taken = if *expected {
                    is_true
                } else {
                    bcx.ins().icmp(IntCC::Equal, value, zero)
                };
                let cold = if *hot_target {
                    let true_value = bcx.ins().icmp(IntCC::Equal, zero, zero);
                    bcx.ins().bxor(taken, true_value)
                } else {
                    taken
                };
                let cont = bail_if(
                    &mut bcx,
                    cold,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                if *hot_target {
                    bcx.ins().jump(block_for[*target as usize].unwrap(), &[]);
                    terminated = true;
                }
            }
            JitInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), a, b)
                } else {
                    bcx.ins().icmp(op.cc(), a, b)
                };
                let tb = block_for[*target as usize].unwrap();
                let fb = block_for[i + 1].unwrap();
                if *expected {
                    bcx.ins().brif(c, tb, &[], fb, &[]);
                } else {
                    bcx.ins().brif(c, fb, &[], tb, &[]);
                }
                terminated = true;
            }
            JitInstr::ProfiledJumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
                hot_target,
            } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let cmp = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), a, b)
                } else {
                    bcx.ins().icmp(op.cc(), a, b)
                };
                let taken = if *expected {
                    cmp
                } else {
                    let zero = bcx.ins().iconst(types::I64, 0);
                    let true_value = bcx.ins().icmp(IntCC::Equal, zero, zero);
                    bcx.ins().bxor(cmp, true_value)
                };
                let cold = if *hot_target {
                    let zero = bcx.ins().iconst(types::I64, 0);
                    let true_value = bcx.ins().icmp(IntCC::Equal, zero, zero);
                    bcx.ins().bxor(taken, true_value)
                } else {
                    taken
                };
                let cont = bail_if(
                    &mut bcx,
                    cold,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                if *hot_target {
                    bcx.ins().jump(block_for[*target as usize].unwrap(), &[]);
                    terminated = true;
                }
            }
            JitInstr::Return { src } => {
                let v = bcx.use_var(reg(*src));
                bcx.ins().store(MemFlags::trusted(), v, out_ptr, 0);
                let one = bcx.ins().iconst(types::I8, 1);
                bcx.ins().return_(&[one]);
                terminated = true;
            }
            JitInstr::Bail => {
                bcx.ins().jump(fallback, &[]);
                terminated = true;
            }
            JitInstr::OsrExit => {
                // OSR-exit (J5.2): the loop has exited. Deopt *unconditionally* at
                // this ip, capturing the live-out window so the host resumes the
                // interpreter here (precise-deopt). Reuse `bail_if` in its
                // unconditional mode: it mints a stable safepoint id, records the
                // `DeoptSite` (resume_ip = this ip, live = entry-assigned regs), and
                // emits the id-store + live-capture on the (unconditionally taken)
                // bail edge — the exact same machinery a guard bail uses.
                let always = bcx.ins().iconst(types::I8, 1);
                let cont = bail_if(
                    &mut bcx,
                    always,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i, unconditional),
                );
                // `bail_if` switched us to the (now unreachable, since the bail is
                // unconditional) `cont` block; terminate it so it stays well-formed
                // for `seal_all_blocks` (Cranelift DCE drops the dead block).
                bcx.switch_to_block(cont);
                bcx.ins().jump(fallback, &[]);
                terminated = true;
            }
            JitInstr::ListGetIntDirect { dst, base, index } => {
                let result = emit_direct_get(
                    &mut bcx,
                    lens_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    reg(*base),
                    reg(*index),
                    *base,
                    types::I64,
                );
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListSetIntDirect {
                dst,
                base,
                index,
                value,
            } => {
                emit_direct_set_int(
                    &mut bcx,
                    lens_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    reg(*base),
                    reg(*index),
                    reg(*value),
                    *base,
                );
                let zero = bcx.ins().iconst(types::I64, 0);
                bcx.def_var(reg(*dst), zero);
            }
            JitInstr::ListGetFloatDirect { dst, base, index } => {
                let result = emit_direct_get(
                    &mut bcx,
                    lens_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    reg(*base),
                    reg(*index),
                    *base,
                    types::F64,
                );
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListLenDirect { dst, base } => {
                // Length lives in the `lens` slot for the base param (param index ==
                // register index for flat params). No host call, no bail.
                let len = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    lens_ptr,
                    (*base as i32) * 8,
                );
                bcx.def_var(reg(*dst), len);
            }
            JitInstr::GuardClosureId { base, expected } => {
                // Read the closure handle's underlying function id and bail to the
                // interpreter if it isn't the speculated callee `expected`. The
                // helper is total (returns -1 on a non-closure handle), so the
                // compare alone decides; the bail reuses the standard re-run-from-top
                // fallback, sound because the inlined subset is side-effect-free.
                let handle = bcx.use_var(reg(*base));
                let call = bcx
                    .ins()
                    .call(helper_ref(HostHelper::ClosureId), &[host_ctx, handle]);
                let id = bcx.inst_results(call)[0];
                let want = bcx.ins().iconst(types::I64, *expected);
                let mismatch = bcx.ins().icmp(IntCC::NotEqual, id, want);
                let cont = bail_if(
                    &mut bcx,
                    mismatch,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
            }
        }
    }
    if !terminated {
        // Fell off the end without an explicit return: behave like the VM's
        // defensive `Unit` return by bailing (this path is unreachable for
        // well-formed bytecode, which always ends in `Return`).
        bcx.ins().jump(fallback, &[]);
    }

    // Fallback block body: not completed.
    bcx.switch_to_block(fallback);
    let zero8 = bcx.ins().iconst(types::I8, 0);
    bcx.ins().return_(&[zero8]);

    bcx.seal_all_blocks();
    bcx.finalize();

    Ok(DeoptMap {
        sites,
        payload_words,
    })
}

/// TV2 direct list read: `cont: dst = base_ptr[index]`, bounds-checked against the
/// param's `lens` slot. `base_var` holds the raw data pointer (i64); `base_param`
/// is its param/register index (used to index `lens`); `elem_ty` is `I64` for an
/// `Ints` list or `F64` for a `Floats` list. An index `< 0` or `>= len` branches to
/// `fallback` (→ the VM re-runs on the interpreter, matching the helper's OOB bail).
///
/// SAFETY (codegen contract): the generated load reads exactly one `elem_ty` at
/// `base_ptr + index * 8`, only after proving `0 <= index < len`. `base_ptr` and
/// `len` come from the caller's `args`/`lens` for the same param, which the
/// `NativeModule::call` borrow protocol guarantees point at a live, immovable,
/// unmutated buffer of `len` elements for the call's duration. So every in-bounds
/// element address is valid and the read cannot alias a concurrent mutation.
#[allow(clippy::too_many_arguments)]
fn emit_direct_get(
    bcx: &mut FunctionBuilder,
    lens_ptr: Value,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    base_var: Variable,
    index_var: Variable,
    base_param: u32,
    elem_ty: types::Type,
) -> Value {
    let index = bcx.use_var(index_var);
    let len = bcx.ins().load(
        types::I64,
        MemFlags::trusted(),
        lens_ptr,
        (base_param as i32) * 8,
    );
    // Single unsigned compare folds "index < 0" (huge unsigned) and "index >= len"
    // into one OOB test (len is a non-negative element count).
    let oob = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
    let cont = bail_if(
        bcx,
        oob,
        fallback,
        safepoint_ptr,
        payload_ptr,
        vars,
        next_id,
        deopt,
    );
    bcx.switch_to_block(cont);
    let base_ptr = bcx.use_var(base_var);
    let offset = bcx.ins().imul_imm(index, 8);
    let addr = bcx.ins().iadd(base_ptr, offset);
    bcx.ins().load(elem_ty, MemFlags::trusted(), addr, 0)
}

#[allow(clippy::too_many_arguments)]
fn emit_direct_set_int(
    bcx: &mut FunctionBuilder,
    lens_ptr: Value,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    base_var: Variable,
    index_var: Variable,
    value_var: Variable,
    base_param: u32,
) {
    let index = bcx.use_var(index_var);
    let len = bcx.ins().load(
        types::I64,
        MemFlags::trusted(),
        lens_ptr,
        (base_param as i32) * 8,
    );
    let oob = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
    let cont = bail_if(
        bcx,
        oob,
        fallback,
        safepoint_ptr,
        payload_ptr,
        vars,
        next_id,
        deopt,
    );
    bcx.switch_to_block(cont);
    let base_ptr = bcx.use_var(base_var);
    let value = bcx.use_var(value_var);
    let offset = bcx.ins().imul_imm(index, 8);
    let addr = bcx.ins().iadd(base_ptr, offset);
    bcx.ins().store(MemFlags::trusted(), value, addr, 0);
}

/// Host-side deopt bookkeeping threaded through every guard emission. Each
/// [`bail_if`] call records one [`DeoptSite`] into `sites` for the instruction
/// currently being emitted (`ip`), keeping `sites` aligned 1:1 with the minted
/// safepoint ids. Carries no Cranelift state — it shapes no machine code.
struct DeoptCtx<'a> {
    /// Index of the instruction currently being lowered (the resume_ip for any
    /// guard it emits).
    ip: u32,
    /// Definite-assignment sets per instruction (see [`definite_assignment`]).
    assigned_in: &'a [Vec<bool>],
    /// Storage class per register, to type each live register.
    reg_types: &'a [JitValueType],
    /// Accumulated sites, in emission (= id) order.
    sites: &'a mut Vec<DeoptSite>,
    /// Required payload width for this function. Starts at `n_regs`; native-call
    /// deopt sites reserve extra words for copied child payloads.
    payload_words: &'a mut usize,
    /// Test/diagnostic hook: when set, matching safepoints bail unconditionally
    /// (see [`NativeModule::compile_forcing_bail`] and
    /// [`NativeModule::compile_forcing_all_bails`]). `None` on the default
    /// [`compile`](NativeModule::compile) path, where every site is guarded.
    forced: Option<ForcedDeopt>,
    /// OSR-exit (J5.2): when set, the site about to be minted bails unconditionally
    /// (the loop-exit edge always deopts). Independent of `forced` (which is keyed
    /// by a specific id); this applies to whatever id this single `bail_if` mints.
    unconditional: bool,
}

impl DeoptCtx<'_> {
    /// Record the site for the safepoint about to be minted: resume at the current
    /// instruction with its entry-assigned (definitely-live) registers. Returns the
    /// same `live` set so the caller can emit the matching payload-capture stores.
    fn record_site(&mut self, child: Option<DeoptChildSite>) -> Vec<(u32, JitValueType)> {
        let live: Vec<(u32, JitValueType)> = match self.assigned_in.get(self.ip as usize) {
            Some(set) => set
                .iter()
                .enumerate()
                .filter(|&(_, &assigned)| assigned)
                .map(|(r, _)| (r as u32, self.reg_types[r]))
                .collect(),
            None => Vec::new(),
        };
        self.sites.push(DeoptSite {
            resume_ip: self.ip,
            live: live.clone(),
            child,
        });
        live
    }

    fn record(&mut self) -> Vec<(u32, JitValueType)> {
        self.record_site(None)
    }

    fn record_child(
        &mut self,
        callee: &NativeCallee,
    ) -> (Vec<(u32, JitValueType)>, DeoptChildSite) {
        let safepoint_slot = *self.payload_words;
        let payload_slot = safepoint_slot + 1;
        *self.payload_words += 1 + callee.deopt_payload_words;
        let child = DeoptChildSite {
            callee: callee.handle,
            safepoint_slot: safepoint_slot as u32,
            payload_slot: payload_slot as u32,
            payload_words: callee.deopt_payload_words as u32,
        };
        let live = self.record_site(Some(child));
        (live, child)
    }
}

/// Emit a per-site guarded bail and return the `cont` block to continue in.
///
/// `safepoint_ptr` is the host's safepoint-id cell; `next_id` is the running
/// site-id counter (post-incremented to mint this site's stable id, starting from
/// 1; `0` stays reserved). On the bail edge — and *only* there — a dedicated cold
/// `site_block` stores this site's id into `safepoint_ptr` before jumping to the
/// shared `fallback`. The hot fall-through (`cont`) path executes zero extra
/// instructions, so non-bailing iterations are unaffected.
///
/// `deopt` records this site's [`DeoptSite`] (resume_ip + live regs) host-side,
/// pushed in lock-step with `next_id` so `sites[id - 1]` aligns with the id minted
/// here. This recording emits no machine code.
///
/// On the cold edge — and only there — each live register's current value is also
/// *stored* into `payload_ptr[reg]` (`vars[reg]` is its Cranelift variable; an f64
/// var stores its 8-byte bit pattern into the slot). The hot `cont` path emits no
/// capture store, so non-bailing iterations are unaffected.
#[allow(clippy::too_many_arguments)]
fn bail_if(
    bcx: &mut FunctionBuilder,
    cond: Value,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Block {
    let site_id = *next_id;
    *next_id += 1;
    let forced = deopt.forced.is_some_and(|forced| forced.forces(site_id)) || deopt.unconditional;
    let live = deopt.record();
    let site_block = bcx.create_block();
    let cont = bcx.create_block();
    if forced {
        // Forced-bail diagnostic path: deopt at this site unconditionally, ignoring
        // `cond`. `cont` is still created/sealed (and emitted into) below — it just
        // becomes unreachable here, which Cranelift's DCE drops. The site_block body
        // is unchanged, so the forced bail captures the same live set a natural bail
        // would. The default `compile` path never sets `forced`, so it always takes
        // the `brif` branch and emits byte-identical guards.
        bcx.ins().jump(site_block, &[]);
    } else {
        bcx.ins().brif(cond, site_block, &[], cont, &[]);
    }
    // Cold path: record this site's id, capture each live register's value into the
    // payload buffer, then fall through to the shared fallback. None of this is
    // emitted on the hot `cont` edge below.
    bcx.switch_to_block(site_block);
    let id_v = bcx.ins().iconst(types::I64, site_id);
    bcx.ins().store(MemFlags::trusted(), id_v, safepoint_ptr, 0);
    for &(reg, _) in &live {
        let v = bcx.use_var(vars[reg as usize]);
        bcx.ins()
            .store(MemFlags::trusted(), v, payload_ptr, (reg as i32) * 8);
    }
    bcx.ins().jump(fallback, &[]);
    bcx.switch_to_block(cont);
    cont
}

#[allow(clippy::too_many_arguments)]
fn bail_if_child_native_failed(
    bcx: &mut FunctionBuilder,
    cond: Value,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    child_safepoint_ptr: Value,
    child_payload_ptr: Value,
    child_meta: &NativeCallee,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Block {
    let site_id = *next_id;
    *next_id += 1;
    let forced = deopt.forced.is_some_and(|forced| forced.forces(site_id)) || deopt.unconditional;
    let (live, child) = deopt.record_child(child_meta);
    let site_block = bcx.create_block();
    let cont = bcx.create_block();
    if forced {
        bcx.ins().jump(site_block, &[]);
    } else {
        bcx.ins().brif(cond, site_block, &[], cont, &[]);
    }

    bcx.switch_to_block(site_block);
    let id_v = bcx.ins().iconst(types::I64, site_id);
    bcx.ins().store(MemFlags::trusted(), id_v, safepoint_ptr, 0);
    for &(reg, _) in &live {
        let v = bcx.use_var(vars[reg as usize]);
        bcx.ins()
            .store(MemFlags::trusted(), v, payload_ptr, (reg as i32) * 8);
    }

    let child_safepoint = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), child_safepoint_ptr, 0);
    bcx.ins().store(
        MemFlags::trusted(),
        child_safepoint,
        payload_ptr,
        (child.safepoint_slot as i32) * 8,
    );
    for slot in 0..child.payload_words {
        let bits = bcx.ins().load(
            types::I64,
            MemFlags::trusted(),
            child_payload_ptr,
            (slot as i32) * 8,
        );
        bcx.ins().store(
            MemFlags::trusted(),
            bits,
            payload_ptr,
            ((child.payload_slot + slot) as i32) * 8,
        );
    }
    bcx.ins().jump(fallback, &[]);
    bcx.switch_to_block(cont);
    cont
}

/// Load the host-helper bail flag and branch to `fallback` if a preceding heap
/// read flagged failure — checked immediately after each helper call so a bad
/// read never keeps executing. Returns the continuation block.
#[allow(clippy::too_many_arguments)]
fn bail_if_helper_failed(
    bcx: &mut FunctionBuilder,
    bail_ptr: Value,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Block {
    let flag = bcx.ins().load(types::I8, MemFlags::trusted(), bail_ptr, 0);
    bail_if(
        bcx,
        flag,
        fallback,
        safepoint_ptr,
        payload_ptr,
        vars,
        next_id,
        deopt,
    )
}

/// Checked division / remainder matching the interpreter: bail on divide-by-zero
/// and on `i64::MIN / -1` (the only signed-division overflow).
#[allow(clippy::too_many_arguments)]
fn emit_checked_divrem(
    bcx: &mut FunctionBuilder,
    lhs: Variable,
    rhs: Variable,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    is_rem: bool,
) -> Value {
    let a = bcx.use_var(lhs);
    let b = bcx.use_var(rhs);
    let zero = bcx.ins().iconst(types::I64, 0);
    let is_zero = bcx.ins().icmp(IntCC::Equal, b, zero);
    let cont1 = bail_if(
        bcx,
        is_zero,
        fallback,
        safepoint_ptr,
        payload_ptr,
        vars,
        next_id,
        deopt,
    );
    bcx.switch_to_block(cont1);
    let imin = bcx.ins().iconst(types::I64, i64::MIN);
    let neg1 = bcx.ins().iconst(types::I64, -1);
    let a_is_min = bcx.ins().icmp(IntCC::Equal, a, imin);
    let b_is_neg1 = bcx.ins().icmp(IntCC::Equal, b, neg1);
    let overflow = bcx.ins().band(a_is_min, b_is_neg1);
    let cont2 = bail_if(
        bcx,
        overflow,
        fallback,
        safepoint_ptr,
        payload_ptr,
        vars,
        next_id,
        deopt,
    );
    bcx.switch_to_block(cont2);
    if is_rem {
        bcx.ins().srem(a, b)
    } else {
        bcx.ins().sdiv(a, b)
    }
}

/// Checked shift: bail when the shift amount is negative or `>= 64` (so the
/// in-range case matches `wrapping_shl`/`wrapping_shr` exactly).
#[allow(clippy::too_many_arguments)]
fn emit_checked_shift(
    bcx: &mut FunctionBuilder,
    lhs: Variable,
    rhs: Variable,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    is_right: bool,
) -> Value {
    let a = bcx.use_var(lhs);
    let amt = bcx.use_var(rhs);
    let limit = bcx.ins().iconst(types::I64, 64);
    // Unsigned compare folds "negative" (huge unsigned) and ">= 64" into one test.
    let oob = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, amt, limit);
    let cont = bail_if(
        bcx,
        oob,
        fallback,
        safepoint_ptr,
        payload_ptr,
        vars,
        next_id,
        deopt,
    );
    bcx.switch_to_block(cont);
    if is_right {
        bcx.ins().sshr(a, amt)
    } else {
        bcx.ins().ishl(a, amt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test shim: validate as a non-OSR program (the common case for these IR
    /// validation tests). OSR-specific validation is exercised via `compile_osr`.
    fn validate(program: &JitFunction) -> Result<(), JitError> {
        super::validate(program, false)
    }

    #[test]
    fn host_helper_descriptor_table_is_complete_and_unique() {
        let helpers = HostHelper::all();
        assert_eq!(helpers.len(), HostHelper::DESCRIPTORS.len());
        let mut symbols = std::collections::HashSet::new();
        let mut seen_helpers = std::collections::HashSet::new();
        for (&helper, descriptor) in helpers.iter().zip(HostHelper::DESCRIPTORS.iter()) {
            assert_eq!(descriptor.helper, helper);
            assert!(seen_helpers.insert(helper), "duplicate helper: {helper:?}");
            assert!(
                symbols.insert(descriptor.symbol),
                "duplicate host helper symbol: {}",
                descriptor.symbol
            );
            assert_eq!(helper.symbol(), descriptor.symbol);
            assert_eq!(helper.signature().args, descriptor.sig.args);
            assert_eq!(helper.arg_types(), descriptor.sig.args);
            if helper.heap_effect().extends_input_handles() {
                assert!(
                    matches!(
                        helper.signature().result,
                        HostResult::Exact(JitValueType::Handle)
                    ),
                    "{helper:?} extends input handles but does not return a handle"
                );
            }
        }
    }

    #[test]
    fn host_helper_heap_effect_metadata_covers_transactional_helpers() {
        let mut mutates_input = std::collections::HashSet::from([
            HostHelper::ListSetInt,
            HostHelper::ListPushInt,
            HostHelper::ListSortInt,
            HostHelper::MapInsertInt,
            HostHelper::SetInsertInt,
            HostHelper::SortedSetInsertInt,
            HostHelper::SortedMapInsertInt,
            HostHelper::DequePushBackInt,
            HostHelper::DequePushFrontInt,
            HostHelper::DequePopFrontInt,
            HostHelper::DequePopBackInt,
        ]);
        let mut allocates_result = std::collections::HashSet::from([
            HostHelper::ListNewInt,
            HostHelper::StringFromInt,
            HostHelper::StringConcat,
            HostHelper::StringSlice,
            HostHelper::StringPadLeft,
            HostHelper::StringSplit,
            HostHelper::StringLiteral,
            HostHelper::JsonParse,
            HostHelper::JsonField,
            HostHelper::BytesSlice,
        ]);
        let mut extends_input_handles =
            std::collections::HashSet::from([HostHelper::FieldHandle, HostHelper::ListGetHandle]);

        for &helper in HostHelper::all() {
            let expected = if mutates_input.remove(&helper) {
                HostHeapEffect::MutatesInput
            } else if allocates_result.remove(&helper) {
                HostHeapEffect::AllocatesResult
            } else if extends_input_handles.remove(&helper) {
                HostHeapEffect::ExtendsInputHandles
            } else if helper == HostHelper::FieldSetInt {
                HostHeapEffect::ReplacesInput
            } else {
                HostHeapEffect::ReadOnly
            };
            assert_eq!(helper.heap_effect(), expected, "{helper:?}");
        }
        assert!(mutates_input.is_empty());
        assert!(allocates_result.is_empty());
        assert!(extends_input_handles.is_empty());
    }

    /// Test-only convenience over [`NativeModule::call`]: pass a zeroed `lens`
    /// (length-matched to `args`) for tests that use no flat-array params.
    trait CallScalar {
        fn callt(&self, id: CompiledId, args: &[i64]) -> Option<i64>;
    }
    impl CallScalar for NativeModule {
        fn callt(&self, id: CompiledId, args: &[i64]) -> Option<i64> {
            let lens = vec![0i64; args.len()];
            self.call(id, args, &lens).completed()
        }
    }

    /// `NativeOutcome`'s scalar/handle/any-bits accessors must keep a scalar result
    /// and an opaque heap-output-table handle distinct: `completed()` is scalar-only,
    /// `completed_handle()` is handle-only, and `completed_any_bits()` accepts either
    /// completed variant. A `Deopt` is `None` for all three.
    #[test]
    fn native_outcome_completed_accessors_separate_scalar_and_handle() {
        let scalar = NativeOutcome::Completed(10);
        assert_eq!(scalar.clone().completed(), Some(10));
        assert_eq!(scalar.clone().completed_handle(), None);
        assert_eq!(scalar.completed_any_bits(), Some(10));

        let handle = NativeOutcome::CompletedHandle(7);
        // A handle is NOT a scalar result, so `completed()` must hide it.
        assert_eq!(handle.clone().completed(), None);
        assert_eq!(handle.clone().completed_handle(), Some(7));
        assert_eq!(handle.completed_any_bits(), Some(7));

        let deopt = NativeOutcome::Deopt {
            safepoint_id: SafepointId(0),
            live: Vec::new(),
            child: None,
        };
        assert_eq!(deopt.clone().completed(), None);
        assert_eq!(deopt.clone().completed_handle(), None);
        assert_eq!(deopt.completed_any_bits(), None);
    }

    extern "C" fn noop_field_int(_ctx: HostCtx, _handle: i64, _slot: i64) -> i64 {
        0
    }
    extern "C" fn noop_field_set_int(_ctx: HostCtx, _handle: i64, _slot: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_len(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_collection_len(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_is_empty(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_get_int(_ctx: HostCtx, _handle: i64, _index: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_set_int(_ctx: HostCtx, _handle: i64, _index: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_push_int(_ctx: HostCtx, _handle: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_sort_int(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_new_int(_ctx: HostCtx) -> i64 {
        0
    }
    extern "C" fn noop_field_float(_ctx: HostCtx, _handle: i64, _slot: i64) -> f64 {
        0.0
    }
    extern "C" fn noop_list_get_float(_ctx: HostCtx, _handle: i64, _index: i64) -> f64 {
        0.0
    }
    extern "C" fn noop_closure_id(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_closure_capture(_ctx: HostCtx, _handle: i64, _index: i64) -> i64 {
        0
    }
    extern "C" fn noop_field_closure_id(_ctx: HostCtx, _handle: i64, _slot: i64) -> i64 {
        0
    }
    extern "C" fn noop_field_closure_capture(
        _ctx: HostCtx,
        _handle: i64,
        _slot: i64,
        _index: i64,
    ) -> i64 {
        0
    }
    extern "C" fn noop_field_handle(_ctx: HostCtx, _handle: i64, _slot: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_get_handle(_ctx: HostCtx, _handle: i64, _index: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_from_int(_ctx: HostCtx, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_len(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_concat(_ctx: HostCtx, _left: i64, _right: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_slice(_ctx: HostCtx, _value: i64, _start: i64, _len: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_pad_left(_ctx: HostCtx, _value: i64, _width: i64, _fill: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_pad_left_len(
        _ctx: HostCtx,
        _value: i64,
        _width: i64,
        _fill: i64,
    ) -> i64 {
        0
    }
    extern "C" fn noop_string_split(_ctx: HostCtx, _value: i64, _delimiter: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_starts_with(_ctx: HostCtx, _value: i64, _prefix: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_split_count(_ctx: HostCtx, _value: i64, _delimiter: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_literal(_ctx: HostCtx, _literal_id: i64) -> i64 {
        0
    }
    extern "C" fn noop_json_parse(_ctx: HostCtx, _text: i64) -> i64 {
        0
    }
    extern "C" fn noop_json_field(_ctx: HostCtx, _value: i64, _name: i64) -> i64 {
        0
    }
    extern "C" fn noop_json_field_int(_ctx: HostCtx, _value: i64, _name: i64) -> i64 {
        0
    }
    extern "C" fn noop_bytes_slice(_ctx: HostCtx, _value: i64, _start: i64, _len: i64) -> i64 {
        0
    }
    extern "C" fn noop_map_insert_int(_ctx: HostCtx, _map: i64, _key: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_map_get_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        0
    }
    extern "C" fn noop_map_get_match_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        0
    }
    extern "C" fn noop_map_get_match_found(_ctx: HostCtx) -> i64 {
        0
    }
    extern "C" fn noop_map_contains_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        0
    }
    extern "C" fn noop_set_insert_int(_ctx: HostCtx, _set: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_set_insert_int(_ctx: HostCtx, _set: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_set_contains_int(_ctx: HostCtx, _set: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_map_insert_int(
        _ctx: HostCtx,
        _map: i64,
        _key: i64,
        _value: i64,
    ) -> i64 {
        0
    }
    extern "C" fn noop_sorted_map_get_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_map_get_found(_ctx: HostCtx) -> i64 {
        0
    }
    extern "C" fn noop_sorted_map_contains_key_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_map_len(_ctx: HostCtx, _map: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_len(_ctx: HostCtx, _deque: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_push_back_int(_ctx: HostCtx, _deque: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_push_front_int(_ctx: HostCtx, _deque: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_pop_front_int(_ctx: HostCtx, _deque: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_pop_back_int(_ctx: HostCtx, _deque: i64) -> i64 {
        0
    }

    fn host_helpers() -> HostHelpers {
        HostHelpers {
            field_int: noop_field_int,
            field_set_int: noop_field_set_int,
            list_len: noop_list_len,
            list_is_empty: noop_is_empty,
            list_get_int: noop_list_get_int,
            list_set_int: noop_list_set_int,
            list_push_int: noop_list_push_int,
            list_sort_int: noop_list_sort_int,
            list_new_int: noop_list_new_int,
            field_float: noop_field_float,
            list_get_float: noop_list_get_float,
            closure_id: noop_closure_id,
            closure_capture: noop_closure_capture,
            field_closure_id: noop_field_closure_id,
            field_closure_capture: noop_field_closure_capture,
            field_handle: noop_field_handle,
            list_get_handle: noop_list_get_handle,
            string_from_int: noop_string_from_int,
            string_len: noop_string_len,
            string_concat: noop_string_concat,
            string_slice: noop_string_slice,
            string_pad_left: noop_string_pad_left,
            string_pad_left_len: noop_string_pad_left_len,
            string_split: noop_string_split,
            string_starts_with: noop_string_starts_with,
            string_split_count: noop_string_split_count,
            string_literal: noop_string_literal,
            json_parse: noop_json_parse,
            json_field: noop_json_field,
            json_field_int: noop_json_field_int,
            bytes_len: noop_collection_len,
            bytes_slice: noop_bytes_slice,
            map_insert_int: noop_map_insert_int,
            map_get_int: noop_map_get_int,
            map_get_match_int: noop_map_get_match_int,
            map_get_match_found: noop_map_get_match_found,
            map_contains_int: noop_map_contains_int,
            map_len: noop_collection_len,
            map_is_empty: noop_is_empty,
            set_insert_int: noop_set_insert_int,
            set_len: noop_collection_len,
            set_is_empty: noop_is_empty,
            sorted_set_insert_int: noop_sorted_set_insert_int,
            sorted_set_contains_int: noop_sorted_set_contains_int,
            sorted_set_is_empty: noop_is_empty,
            sorted_map_insert_int: noop_sorted_map_insert_int,
            sorted_map_get_int: noop_sorted_map_get_int,
            sorted_map_get_found: noop_sorted_map_get_found,
            sorted_map_contains_key_int: noop_sorted_map_contains_key_int,
            sorted_map_is_empty: noop_is_empty,
            sorted_map_len: noop_sorted_map_len,
            deque_len: noop_deque_len,
            deque_is_empty: noop_is_empty,
            deque_push_back_int: noop_deque_push_back_int,
            deque_push_front_int: noop_deque_push_front_int,
            deque_pop_front_int: noop_deque_pop_front_int,
            deque_pop_back_int: noop_deque_pop_back_int,
        }
    }

    /// A module with no-op host helpers (these tests exercise only scalar ops).
    fn module() -> NativeModule {
        NativeModule::new(host_helpers()).unwrap()
    }

    fn f(n_params: u32, n_regs: u32, code: Vec<JitInstr>) -> JitFunction {
        JitFunction {
            n_params,
            n_regs,
            reg_types: vec![JitValueType::Int; n_regs as usize],
            code,
            cold_blocks: Vec::new(),
        }
    }

    /// Like `f` but with explicit per-register storage classes (for float tests).
    fn ft(n_params: u32, reg_types: Vec<JitValueType>, code: Vec<JitInstr>) -> JitFunction {
        JitFunction {
            n_params,
            n_regs: reg_types.len() as u32,
            reg_types,
            code,
            cold_blocks: Vec::new(),
        }
    }

    #[test]
    fn rejects_memoized_heap_writing_host_helper() {
        use JitValueType::{Handle, Int};

        let err = validate(&ft(
            1,
            vec![Handle, Int, Int, Int, Int],
            vec![
                JitInstr::MemoizedHostCall {
                    helper: HostHelper::ListSetInt,
                    dst: 3,
                    args: vec![HostArg::Reg(0), HostArg::Reg(1), HostArg::Reg(2)],
                    cache: 4,
                    flag: 1,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .expect_err("memoized heap-writing helper should be rejected");

        assert!(
            err.0.contains("heap-writing helpers cannot be memoized"),
            "{err:?}"
        );
    }

    // Heap-result return ABI: a function whose `Return` source is a
    // `Handle` register reports `CompletedHandle` carrying the i64 it returned (an
    // opaque output-table handle), while the scalar path stays `Completed`. This
    // pass-through (`fn(h) -> h`) performs no allocation/mutation; the host
    // materializes from its output table only on this clean completion (§7.2-safe).
    #[test]
    fn handle_returning_function_reports_completed_handle() {
        use JitValueType::Handle;
        let mut m = module();
        // fn(h: Handle) -> Handle { return h }
        let id = m
            .compile(&ft(1, vec![Handle], vec![JitInstr::Return { src: 0 }]))
            .unwrap();
        // The arg is an opaque table index (the host's input-table handle); the call
        // returns it verbatim via the heap-result variant.
        match m.call(id, &[7], &[0]) {
            NativeOutcome::CompletedHandle(h) => assert_eq!(h, 7),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }
        // The scalar-return path is byte-identical: a plain Int return is `Completed`.
        let sid = m
            .compile(&f(1, 1, vec![JitInstr::Return { src: 0 }]))
            .unwrap();
        match m.call(sid, &[42], &[0]) {
            NativeOutcome::Completed(v) => assert_eq!(v, 42),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    static MEMOIZED_SPLIT_COUNT_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    extern "C" fn counting_string_split_count(_ctx: HostCtx, _value: i64, _delimiter: i64) -> i64 {
        MEMOIZED_SPLIT_COUNT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        5
    }

    #[test]
    fn memoized_host_call_reuses_first_scalar_result_in_loop() {
        use JitValueType::{Handle, Int};

        MEMOIZED_SPLIT_COUNT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        let mut helpers = host_helpers();
        helpers.string_split_count = counting_string_split_count;
        let mut m = NativeModule::new(helpers).unwrap();
        let id = m
            .compile(&ft(
                3,
                vec![Handle, Handle, Int, Int, Int, Int, Int, Int, Int],
                vec![
                    JitInstr::LoadInt { dst: 3, value: 0 },
                    JitInstr::LoadInt { dst: 4, value: 0 },
                    JitInstr::JumpIfIntCompare {
                        lhs: 3,
                        rhs: 2,
                        op: JitCompare::Lt,
                        expected: false,
                        target: 8,
                    },
                    JitInstr::MemoizedHostCall {
                        helper: HostHelper::StringSplitCount,
                        dst: 5,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                        cache: 7,
                        flag: 8,
                    },
                    JitInstr::Add {
                        dst: 4,
                        lhs: 4,
                        rhs: 5,
                    },
                    JitInstr::LoadInt { dst: 6, value: 1 },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 3,
                        rhs: 6,
                    },
                    JitInstr::Jump { target: 2 },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();

        match m.call(id, &[10, 11, 4], &[0, 0, 0]) {
            NativeOutcome::Completed(v) => assert_eq!(v, 20),
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(
            MEMOIZED_SPLIT_COUNT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            1,
        );
    }

    // A forced bail of a handle-returning function reports `Deopt`, NOT
    // `CompletedHandle`: the heap result is materialized only on clean completion, so
    // a bailed attempt never reports a heap value (§7.2 no-effect-before-bail).
    #[test]
    fn handle_returning_function_bails_as_deopt_not_handle() {
        use JitValueType::Handle;
        let mut m = module();
        // fn(h: Handle) -> Handle { bail; return h } — force the (only) site to bail.
        let func = ft(
            1,
            vec![Handle],
            vec![JitInstr::Bail, JitInstr::Return { src: 0 }],
        );
        let id = m.compile(&func).unwrap();
        match m.call(id, &[7], &[0]) {
            NativeOutcome::Deopt { .. } => {}
            other => panic!("expected Deopt on bail, got {other:?}"),
        }
    }

    #[test]
    fn string_from_int_returns_heap_output_handle() {
        use JitValueType::{Handle, Int};
        extern "C" fn string_from_int(_ctx: HostCtx, value: i64) -> i64 {
            value + 100
        }
        let mut m = NativeModule::new(HostHelpers {
            string_from_int,
            ..host_helpers()
        })
        .unwrap();
        let id = m
            .compile(&ft(
                1,
                vec![Int, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::StringFromInt,
                        dst: 1,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(id, &[23], &[0]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 123),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }
    }

    #[test]
    fn string_from_int_handle_can_feed_string_len() {
        use JitValueType::{Handle, Int};
        extern "C" fn string_from_int(_ctx: HostCtx, value: i64) -> i64 {
            value + 10
        }
        extern "C" fn string_len(_ctx: HostCtx, handle: i64) -> i64 {
            handle * 2
        }
        let mut m = NativeModule::new(HostHelpers {
            string_from_int,
            string_len,
            ..host_helpers()
        })
        .unwrap();
        let id = m
            .compile(&ft(
                1,
                vec![Int, Handle, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::StringFromInt,
                        dst: 1,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::HostCall {
                        helper: HostHelper::StringLen,
                        dst: 2,
                        args: vec![HostArg::Reg(1)],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        match m.call(id, &[5], &[0]) {
            NativeOutcome::Completed(value) => assert_eq!(value, 30),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn string_concat_returns_heap_output_handle() {
        use JitValueType::Handle;
        extern "C" fn string_concat(_ctx: HostCtx, left: i64, right: i64) -> i64 {
            left * 10 + right
        }
        let mut m = NativeModule::new(HostHelpers {
            string_concat,
            ..host_helpers()
        })
        .unwrap();
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Handle, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::StringConcat,
                        dst: 2,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        match m.call(id, &[4, 7], &[0, 1]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 47),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }
    }

    #[test]
    fn host_call_covers_heap_read_helpers_with_immediates_and_typed_results() {
        use JitValueType::{Float, Handle, Int};
        extern "C" fn field_int(_ctx: HostCtx, handle: i64, slot: i64) -> i64 {
            handle + slot
        }
        extern "C" fn list_len(_ctx: HostCtx, handle: i64) -> i64 {
            handle * 2
        }
        extern "C" fn list_get_int(_ctx: HostCtx, handle: i64, index: i64) -> i64 {
            handle + index * 3
        }
        extern "C" fn field_float(_ctx: HostCtx, handle: i64, slot: i64) -> f64 {
            handle as f64 + slot as f64 + 0.25
        }
        extern "C" fn list_get_float(_ctx: HostCtx, handle: i64, index: i64) -> f64 {
            handle as f64 + index as f64 + 0.5
        }
        extern "C" fn closure_capture(_ctx: HostCtx, _handle: i64, _index: i64) -> i64 {
            1.25_f64.to_bits() as i64
        }
        extern "C" fn field_handle(_ctx: HostCtx, handle: i64, slot: i64) -> i64 {
            handle * 10 + slot
        }
        extern "C" fn list_get_handle(_ctx: HostCtx, handle: i64, index: i64) -> i64 {
            handle * 100 + index
        }
        let mut m = NativeModule::new(HostHelpers {
            field_int,
            list_len,
            list_get_int,
            field_float,
            list_get_float,
            closure_capture,
            field_handle,
            list_get_handle,
            ..host_helpers()
        })
        .unwrap();

        let id = m
            .compile(&ft(
                1,
                vec![Handle, Int, Int, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::FieldInt,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(2)],
                    },
                    JitInstr::HostCall {
                        helper: HostHelper::ListLen,
                        dst: 2,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::HostCall {
                        helper: HostHelper::ListGetInt,
                        dst: 3,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 3,
                        rhs: 2,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        match m.call(id, &[5], &[0]) {
            NativeOutcome::Completed(value) => assert_eq!(value, 5 + (5 + 2) * 3 + 10),
            other => panic!("expected Completed, got {other:?}"),
        }

        let float_id = m
            .compile(&ft(
                1,
                vec![Handle, Float],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::FieldFloat,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(3)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(float_id, &[4], &[0]) {
            NativeOutcome::Completed(bits) => assert_eq!(f64::from_bits(bits as u64), 7.25),
            other => panic!("expected Completed float bits, got {other:?}"),
        }

        let list_float_id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Float],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ListGetFloat,
                        dst: 2,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        match m.call(list_float_id, &[4, 3], &[0, 0]) {
            NativeOutcome::Completed(bits) => assert_eq!(f64::from_bits(bits as u64), 7.5),
            other => panic!("expected Completed list float bits, got {other:?}"),
        }

        let capture_id = m
            .compile(&ft(
                1,
                vec![Handle, Float],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ClosureCapture,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(0)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(capture_id, &[9], &[0]) {
            NativeOutcome::Completed(bits) => assert_eq!(f64::from_bits(bits as u64), 1.25),
            other => panic!("expected Completed capture float bits, got {other:?}"),
        }

        let handle_id = m
            .compile(&ft(
                1,
                vec![Handle, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::FieldHandle,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(6)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(handle_id, &[8], &[0]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 86),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }

        let list_handle_id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ListGetHandle,
                        dst: 2,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        match m.call(list_handle_id, &[8, 6], &[0, 0]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 806),
            other => panic!("expected list CompletedHandle, got {other:?}"),
        }
    }

    static MAP_GET_MATCH_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static MAP_CONTAINS_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static MAP_GET_MATCH_FOUND: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

    extern "C" fn counting_map_get_match_int(_ctx: HostCtx, _map: i64, key: i64) -> i64 {
        MAP_GET_MATCH_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match key {
            7 => {
                MAP_GET_MATCH_FOUND.store(1, std::sync::atomic::Ordering::SeqCst);
                0
            }
            8 => {
                MAP_GET_MATCH_FOUND.store(1, std::sync::atomic::Ordering::SeqCst);
                123
            }
            _ => {
                MAP_GET_MATCH_FOUND.store(0, std::sync::atomic::Ordering::SeqCst);
                0
            }
        }
    }

    extern "C" fn counting_map_get_match_found(_ctx: HostCtx) -> i64 {
        MAP_GET_MATCH_FOUND.load(std::sync::atomic::Ordering::SeqCst)
    }

    extern "C" fn counting_map_contains_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        MAP_CONTAINS_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        1
    }

    #[test]
    fn match_map_get_uses_single_lookup_and_found_flag() {
        use JitValueType::{Handle, Int};

        MAP_GET_MATCH_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        MAP_CONTAINS_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        MAP_GET_MATCH_FOUND.store(0, std::sync::atomic::Ordering::SeqCst);

        let mut helpers = host_helpers();
        helpers.map_get_match_int = counting_map_get_match_int;
        helpers.map_get_match_found = counting_map_get_match_found;
        helpers.map_contains_int = counting_map_contains_int;
        let mut m = NativeModule::new(helpers).unwrap();
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Int],
                vec![
                    JitInstr::MatchMapGetInt {
                        map: 0,
                        key: 1,
                        value_dst: 2,
                        some_ip: 1,
                        none_ip: 3,
                    },
                    JitInstr::Return { src: 2 },
                    JitInstr::Nop,
                    JitInstr::LoadInt { dst: 2, value: -1 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();

        assert_eq!(m.call(id, &[42, 7], &[0, 0]).completed(), Some(0));
        assert_eq!(m.call(id, &[42, 8], &[0, 0]).completed(), Some(123));
        assert_eq!(m.call(id, &[42, 9], &[0, 0]).completed(), Some(-1));
        assert_eq!(
            MAP_GET_MATCH_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        assert_eq!(
            MAP_CONTAINS_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[test]
    fn host_call_closure_id_uses_non_bailing_failure_mode() {
        assert_eq!(
            HostHelper::ClosureId.signature().failure,
            HostFailureMode::CannotFail,
            "closure_id is total and must not inspect the shared bail flag",
        );
        use JitValueType::{Handle, Int};
        extern "C" fn closure_id(_ctx: HostCtx, handle: i64) -> i64 {
            handle + 4
        }
        let mut m = NativeModule::new(HostHelpers {
            closure_id,
            ..host_helpers()
        })
        .unwrap();
        let id = m
            .compile(&ft(
                1,
                vec![Handle, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ClosureId,
                        dst: 1,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(id, &[9], &[0]) {
            NativeOutcome::Completed(value) => assert_eq!(value, 13),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn compiles_and_runs_float_arith() {
        use JitValueType::{Float, Int};
        let mut m = module();
        // fn(a: f64, b: f64) -> f64 { return a * b - a }  regs 0=a,1=b,2=t
        let id = m
            .compile(&ft(
                2,
                vec![Float, Float, Float],
                vec![
                    JitInstr::Mul {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Sub {
                        dst: 2,
                        lhs: 2,
                        rhs: 0,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let call = |a: f64, b: f64| {
            f64::from_bits(
                m.callt(id, &[a.to_bits() as i64, b.to_bits() as i64])
                    .unwrap() as u64,
            )
        };
        assert_eq!(call(2.5, 4.0), 2.5 * 4.0 - 2.5);
        assert_eq!(call(3.0, 0.0), -3.0);
        let _ = Int; // silence unused in case
    }

    #[test]
    fn native_scalar_call_invokes_compiled_int_leaf() {
        let mut m = module();
        // callee add2(a, b) = a + b
        let callee = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        // caller(x) = add2(x, 7) * 2
        let caller = m
            .compile(&f(
                1,
                5,
                vec![
                    JitInstr::LoadInt { dst: 1, value: 7 },
                    JitInstr::CallNative {
                        callee,
                        dst: 2,
                        args: vec![0, 1],
                    },
                    JitInstr::LoadInt { dst: 3, value: 2 },
                    JitInstr::Mul {
                        dst: 4,
                        lhs: 2,
                        rhs: 3,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();

        assert_eq!(m.callt(caller, &[5]), Some(24));
        assert_eq!(m.callt(caller, &[-10]), Some(-6));
    }

    #[test]
    fn compile_records_machine_code_size() {
        let mut m = module();
        let id = m
            .compile(&f(
                1,
                2,
                vec![
                    JitInstr::LoadInt { dst: 1, value: 1 },
                    JitInstr::Add {
                        dst: 1,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        assert!(
            m.code_size_bytes(id).is_some_and(|bytes| bytes > 0),
            "compiled functions should expose nonzero emitted machine-code size"
        );

        assert_eq!(
            m.code_size_bytes(CompiledId {
                module_id: id.module_id.wrapping_add(1),
                index: id.index,
            }),
            None,
            "compiled ids from another module must not expose local code metadata"
        );
    }

    #[test]
    fn native_scalar_call_invokes_compiled_call_chain() {
        let mut m = module();
        let inc = m
            .compile(&f(
                1,
                3,
                vec![
                    JitInstr::LoadInt { dst: 1, value: 1 },
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.native_call_depth(inc), Some(0));
        let twice = m
            .compile(&f(
                1,
                4,
                vec![
                    JitInstr::CallNative {
                        callee: inc,
                        dst: 1,
                        args: vec![0],
                    },
                    JitInstr::CallNative {
                        callee: inc,
                        dst: 2,
                        args: vec![0],
                    },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        assert_eq!(m.native_call_depth(twice), Some(1));
        let top = m
            .compile(&f(
                1,
                4,
                vec![
                    JitInstr::CallNative {
                        callee: twice,
                        dst: 1,
                        args: vec![0],
                    },
                    JitInstr::CallNative {
                        callee: twice,
                        dst: 2,
                        args: vec![0],
                    },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();

        assert_eq!(m.native_call_depth(top), Some(2));
        assert_eq!(m.callt(top, &[10]), Some(44));
    }

    #[test]
    fn native_scalar_call_invokes_compiled_float_leaf() {
        use JitValueType::{Float, Int};
        let mut m = module();
        // callee(a: Float, b: Float) = a * b
        let callee = m
            .compile(&ft(
                2,
                vec![Float, Float, Float],
                vec![
                    JitInstr::Mul {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        // caller(x: Float) = callee(x, 4.0) - x
        let caller = m
            .compile(&ft(
                1,
                vec![Float, Float, Float, Float],
                vec![
                    JitInstr::LoadFloat { dst: 1, value: 4.0 },
                    JitInstr::CallNative {
                        callee,
                        dst: 2,
                        args: vec![0, 1],
                    },
                    JitInstr::Sub {
                        dst: 3,
                        lhs: 2,
                        rhs: 0,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        let got = m
            .callt(caller, &[2.5f64.to_bits() as i64])
            .map(|bits| f64::from_bits(bits as u64));
        assert_eq!(got, Some(7.5));
        let _ = Int;
    }

    #[test]
    fn native_call_can_return_child_heap_handle_to_parent() {
        use JitValueType::{Handle, Int};
        extern "C" fn string_from_int(_ctx: HostCtx, value: i64) -> i64 {
            value + 10
        }
        let mut m = NativeModule::new(HostHelpers {
            string_from_int,
            ..host_helpers()
        })
        .unwrap();

        let child = m
            .compile(&ft(
                1,
                vec![Int, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::StringFromInt,
                        dst: 1,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        let parent = m
            .compile(&ft(
                1,
                vec![Int, Handle],
                vec![
                    JitInstr::CallNative {
                        callee: child,
                        dst: 1,
                        args: vec![0],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();

        match m.call(parent, &[37], &[0]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 47),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }
    }

    #[test]
    fn native_call_child_heap_handle_can_feed_parent_host_helper() {
        use JitValueType::{Handle, Int};
        extern "C" fn string_from_int(_ctx: HostCtx, value: i64) -> i64 {
            value + 10
        }
        extern "C" fn string_len(_ctx: HostCtx, handle: i64) -> i64 {
            handle * 2
        }
        let mut m = NativeModule::new(HostHelpers {
            string_from_int,
            string_len,
            ..host_helpers()
        })
        .unwrap();

        let child = m
            .compile(&ft(
                1,
                vec![Int, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::StringFromInt,
                        dst: 1,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        let parent = m
            .compile(&ft(
                1,
                vec![Int, Handle, Int],
                vec![
                    JitInstr::CallNative {
                        callee: child,
                        dst: 1,
                        args: vec![0],
                    },
                    JitInstr::HostCall {
                        helper: HostHelper::StringLen,
                        dst: 2,
                        args: vec![HostArg::Reg(1)],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();

        match m.call(parent, &[5], &[0]) {
            NativeOutcome::Completed(value) => assert_eq!(value, 30),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn native_scalar_call_deopts_at_caller_when_callee_deopts() {
        let mut m = module();
        // callee(a, b) = a + b; overflows for (MAX, 1).
        let callee = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let caller = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::CallNative {
                        callee,
                        dst: 2,
                        args: vec![0, 1],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();

        assert_eq!(m.callt(caller, &[10, 20]), Some(30));
        assert!(matches!(
            m.call(caller, &[i64::MAX, 1], &[0, 0]),
            NativeOutcome::Deopt {
                safepoint_id: SafepointId(1),
                child: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn native_scalar_call_chains_child_deopt_payload() {
        let mut m = module();
        // callee(a, b) = a + b; overflows for (MAX, 1). The child Add guard lives
        // at callee safepoint 1 with params a/b live.
        let callee = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        // caller(x, y) { c = 9; return callee(x, y) + c }. On child deopt, the
        // parent deopts at the CallNative site and preserves its own live regs plus
        // a child frame decoded with the callee's deopt map.
        let caller = m
            .compile(&f(
                2,
                5,
                vec![
                    JitInstr::LoadInt { dst: 2, value: 9 },
                    JitInstr::CallNative {
                        callee,
                        dst: 3,
                        args: vec![0, 1],
                    },
                    JitInstr::Add {
                        dst: 4,
                        lhs: 3,
                        rhs: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();

        match m.call(caller, &[i64::MAX, 1], &[0, 0]) {
            NativeOutcome::Deopt {
                safepoint_id,
                live,
                child: Some(child),
            } => {
                assert_eq!(safepoint_id, SafepointId(1));
                assert_eq!(child.function, callee);
                assert_eq!(child.safepoint_id, SafepointId(1));
                assert_eq!(child.child, None);
                assert_eq!(
                    live.iter().find(|r| r.reg == 2).map(|r| r.value),
                    Some(DeoptValue::Int(9)),
                    "parent payload should still capture live caller regs"
                );
                assert_eq!(
                    child.live.iter().find(|r| r.reg == 0).map(|r| r.value),
                    Some(DeoptValue::Int(i64::MAX))
                );
                assert_eq!(
                    child.live.iter().find(|r| r.reg == 1).map(|r| r.value),
                    Some(DeoptValue::Int(1))
                );
            }
            other => panic!("expected parent deopt with child frame, got {other:?}"),
        }
    }

    #[test]
    fn native_scalar_call_chains_nested_child_deopt_payloads() {
        let mut m = module();
        let leaf = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let middle = m
            .compile(&f(
                2,
                4,
                vec![
                    JitInstr::LoadInt { dst: 2, value: 11 },
                    JitInstr::CallNative {
                        callee: leaf,
                        dst: 3,
                        args: vec![0, 1],
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        let top = m
            .compile(&f(
                2,
                4,
                vec![
                    JitInstr::LoadInt { dst: 2, value: 22 },
                    JitInstr::CallNative {
                        callee: middle,
                        dst: 3,
                        args: vec![0, 1],
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();

        match m.call(top, &[i64::MAX, 1], &[0, 0]) {
            NativeOutcome::Deopt {
                safepoint_id,
                child: Some(middle_frame),
                ..
            } => {
                assert_eq!(safepoint_id, SafepointId(1));
                assert_eq!(middle_frame.function, middle);
                assert_eq!(middle_frame.safepoint_id, SafepointId(1));
                assert_eq!(
                    middle_frame
                        .live
                        .iter()
                        .find(|r| r.reg == 2)
                        .map(|r| r.value),
                    Some(DeoptValue::Int(11))
                );
                let leaf_frame = middle_frame.child.as_ref().expect("leaf child frame");
                assert_eq!(leaf_frame.function, leaf);
                assert_eq!(leaf_frame.safepoint_id, SafepointId(1));
                assert_eq!(
                    leaf_frame.live.iter().find(|r| r.reg == 0).map(|r| r.value),
                    Some(DeoptValue::Int(i64::MAX))
                );
                assert_eq!(
                    leaf_frame.live.iter().find(|r| r.reg == 1).map(|r| r.value),
                    Some(DeoptValue::Int(1))
                );
                assert_eq!(leaf_frame.child, None);
            }
            other => panic!("expected nested native deopt chain, got {other:?}"),
        }
    }

    #[test]
    fn native_call_can_pass_handle_arg_to_readonly_callee() {
        use JitValueType::{Handle, Int};
        extern "C" fn string_len(_ctx: HostCtx, handle: i64) -> i64 {
            handle * 2
        }
        let mut m = NativeModule::new(HostHelpers {
            string_len,
            ..host_helpers()
        })
        .unwrap();
        let callee = m
            .compile(&ft(
                1,
                vec![Handle, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::StringLen,
                        dst: 1,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        let caller = m
            .compile(&ft(
                1,
                vec![Handle, Int],
                vec![
                    JitInstr::CallNative {
                        callee,
                        dst: 1,
                        args: vec![0],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();

        assert_eq!(m.callt(caller, &[21]), Some(42));
    }

    #[test]
    fn native_call_can_invoke_heap_writing_handle_callee() {
        use JitValueType::{Handle, Int};
        extern "C" fn list_set_int(_ctx: HostCtx, handle: i64, index: i64, value: i64) -> i64 {
            handle + index * 10 + value * 100
        }
        let mut m = NativeModule::new(HostHelpers {
            list_set_int,
            ..host_helpers()
        })
        .unwrap();
        let callee = m
            .compile(&ft(
                3,
                vec![Handle, Int, Int, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ListSetInt,
                        dst: 3,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1), HostArg::Reg(2)],
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        let caller = m
            .compile(&ft(
                3,
                vec![Handle, Int, Int, Int],
                vec![
                    JitInstr::CallNative {
                        callee,
                        dst: 3,
                        args: vec![0, 1, 2],
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();

        assert_eq!(m.callt(caller, &[5, 2, 7]), Some(725));
    }

    #[test]
    fn native_call_can_pass_flat_int_arg_to_readonly_callee() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();
        let flat_callee = m
            .compile(&ft(
                1,
                vec![FlatInt, Int],
                vec![
                    JitInstr::ListLenDirect { dst: 1, base: 0 },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        let caller = m
            .compile(&ft(
                1,
                vec![FlatInt, Int],
                vec![
                    JitInstr::CallNative {
                        callee: flat_callee,
                        dst: 1,
                        args: vec![0],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();

        let data = [10i64, 20, 30];
        assert_eq!(
            m.call(caller, &[data.as_ptr() as i64], &[data.len() as i64])
                .completed(),
            Some(3)
        );
    }

    #[test]
    fn native_call_can_pass_flat_int_arg_to_mutating_callee() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();
        let flat_callee = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int],
                vec![
                    JitInstr::ListSetIntDirect {
                        dst: 3,
                        base: 0,
                        index: 1,
                        value: 2,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        let caller = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::CallNative {
                        callee: flat_callee,
                        dst: 3,
                        args: vec![0, 1, 2],
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();

        let mut data = [10i64, 20, 30];
        assert_eq!(
            m.call(
                caller,
                &[data.as_mut_ptr() as i64, 1, 99],
                &[data.len() as i64, 0, 0],
            )
            .completed(),
            Some(99)
        );
        assert_eq!(data, [10, 99, 30]);
    }

    #[test]
    fn native_call_can_pass_flat_float_arg_to_readonly_callee() {
        use JitValueType::{FlatFloat, Float, Int};
        let mut m = module();
        let flat_callee = m
            .compile(&ft(
                2,
                vec![FlatFloat, Int, Float],
                vec![
                    JitInstr::ListGetFloatDirect {
                        dst: 2,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let caller = m
            .compile(&ft(
                2,
                vec![FlatFloat, Int, Float],
                vec![
                    JitInstr::CallNative {
                        callee: flat_callee,
                        dst: 2,
                        args: vec![0, 1],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();

        let data = [1.25f64, 2.5, 3.75];
        let outcome = m.call(caller, &[data.as_ptr() as i64, 1], &[data.len() as i64, 0]);
        match outcome {
            NativeOutcome::Completed(bits) => {
                assert_eq!(f64::from_bits(bits as u64), 2.5);
            }
            other => panic!("expected flat-float native call to complete, got {other:?}"),
        }
    }

    #[test]
    fn float_read_helpers_compile_and_bail() {
        use JitValueType::{Float, Handle, Int};
        // A module whose float helpers return a fixed value or bail by parity of
        // the slot/index, so we can exercise both the success and bail channels.
        extern "C" fn field_float(_ctx: HostCtx, _handle: i64, slot: i64) -> f64 {
            if slot == 0 {
                2.5
            } else {
                signal_bail();
                0.0
            }
        }
        extern "C" fn list_get_float(_ctx: HostCtx, _handle: i64, index: i64) -> f64 {
            if index >= 0 {
                index as f64 + 0.5
            } else {
                signal_bail();
                0.0
            }
        }
        let mut m = NativeModule::new(HostHelpers {
            field_float,
            list_get_float,
            ..host_helpers()
        })
        .unwrap();
        // fn(h: Handle, idx: Int) -> Float { return list[idx] }  regs 0=h,1=idx,2=res
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Float],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ListGetFloat,
                        dst: 2,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        // Handle arg is opaque (helper ignores it); index 3 → 3.5.
        let got = m.callt(id, &[0, 3]).unwrap();
        assert_eq!(f64::from_bits(got as u64), 3.5);
        // Negative index → helper signals bail → None.
        assert_eq!(m.callt(id, &[0, -1]), None);

        // fn(h: Handle) -> Float { return field[1] }  → bails (slot != 0).
        let id2 = m
            .compile(&ft(
                1,
                vec![Handle, Float],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::FieldFloat,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(1)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id2, &[0]), None);
        let _ = Int;
    }

    #[test]
    fn guard_closure_id_passes_or_bails() {
        use JitValueType::{Handle, Int};
        // `closure_id` is the identity on the handle arg, so the handle value IS the
        // observed function id — letting the test drive the guard directly.
        extern "C" fn closure_id(_ctx: HostCtx, handle: i64) -> i64 {
            handle
        }
        let mut m = NativeModule::new(HostHelpers {
            closure_id,
            ..host_helpers()
        })
        .unwrap();
        // fn(h: Handle, x: Int) -> Int { guard id(h) == 7; return x + 100 }
        // regs 0=h, 1=x, 2=hundred, 3=res
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Int, Int],
                vec![
                    JitInstr::GuardClosureId {
                        base: 0,
                        expected: 7,
                    },
                    JitInstr::LoadInt { dst: 2, value: 100 },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        // Matching callee (handle == expected 7): native completes.
        assert_eq!(m.callt(id, &[7, 5]), Some(105));
        // Mismatched callee (handle != 7): guard bails to the interpreter (None).
        assert_eq!(m.callt(id, &[8, 5]), None);
        assert_eq!(m.callt(id, &[0, 5]), None);
    }

    #[test]
    fn closure_id_dispatch_selects_arm_or_bails() {
        use JitValueType::{Handle, Int};
        // `closure_id` is the identity on the handle arg, so the handle value IS the
        // observed function id — the test drives the polymorphic dispatch directly,
        // mirroring the producer's lowering (read id once via ClosureId, then
        // LoadInt + Equal + JumpIfBool per arm, with a no-match Bail).
        extern "C" fn closure_id(_ctx: HostCtx, handle: i64) -> i64 {
            handle
        }
        let mut m = NativeModule::new(HostHelpers {
            closure_id,
            ..host_helpers()
        })
        .unwrap();
        // Polymorphic inline cache over two callees {3, 5}:
        //   0: id  = closure_id(h)
        //   1: key = 3
        //   2: eq  = (id == key)
        //   3: if eq -> arm3 (8)
        //   4: key = 5
        //   5: eq  = (id == key)
        //   6: if eq -> arm5 (11)
        //   7: Bail                    ; no match
        //   8: c30 = 30  9: res = x+30  10: return res   (arm3)
        //  11: c50 = 50 12: res = x+50  13: return res   (arm5)
        // regs: 0=h(Handle) 1=x 2=id 3=key 4=eq 5=c30 6=res3 7=c50 8=res5
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Int, Int, Int, Int, Int, Int, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ClosureId,
                        dst: 2,
                        args: vec![HostArg::Reg(0)],
                    }, // 0
                    JitInstr::LoadInt { dst: 3, value: 3 }, // 1
                    JitInstr::Equal {
                        dst: 4,
                        lhs: 2,
                        rhs: 3,
                    }, // 2
                    JitInstr::JumpIfBool {
                        cond: 4,
                        expected: true,
                        target: 8,
                    }, // 3
                    JitInstr::LoadInt { dst: 3, value: 5 }, // 4
                    JitInstr::Equal {
                        dst: 4,
                        lhs: 2,
                        rhs: 3,
                    }, // 5
                    JitInstr::JumpIfBool {
                        cond: 4,
                        expected: true,
                        target: 11,
                    }, // 6
                    JitInstr::Bail,                         // 7 (no match)
                    JitInstr::LoadInt { dst: 5, value: 30 }, // 8 arm3
                    JitInstr::Add {
                        dst: 6,
                        lhs: 1,
                        rhs: 5,
                    }, // 9
                    JitInstr::Return { src: 6 },            // 10
                    JitInstr::LoadInt { dst: 7, value: 50 }, // 11 arm5
                    JitInstr::Add {
                        dst: 8,
                        lhs: 1,
                        rhs: 7,
                    }, // 12
                    JitInstr::Return { src: 8 },            // 13
                ],
            ))
            .unwrap();
        // Arm 3 selected (h == 3): x + 30.
        assert_eq!(m.callt(id, &[3, 5]), Some(35));
        // Arm 5 selected (h == 5): x + 50.
        assert_eq!(m.callt(id, &[5, 5]), Some(55));
        // No arm matches (h == 9): the cache bails to the interpreter (None).
        assert_eq!(m.callt(id, &[9, 5]), None);
        assert_eq!(m.callt(id, &[0, 5]), None);
    }

    #[test]
    fn direct_flat_reads_index_in_register() {
        use JitValueType::{FlatFloat, FlatInt, Float, Int};
        let mut m = module();

        // fn(a: FlatInt, i: Int) -> Int { return a[i] }  regs 0=a,1=i,2=res
        let id_int = m
            .compile(&ft(
                2,
                vec![FlatInt, Int, Int],
                vec![
                    JitInstr::ListGetIntDirect {
                        dst: 2,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let ints: Vec<i64> = vec![10, 20, 30];
        let ints_ptr = ints.as_ptr() as i64;
        let ilen = ints.len() as i64;
        // In-bounds reads index directly out of the flat buffer.
        assert_eq!(
            m.call(id_int, &[ints_ptr, 0], &[ilen, 0]).completed(),
            Some(10)
        );
        assert_eq!(
            m.call(id_int, &[ints_ptr, 2], &[ilen, 0]).completed(),
            Some(30)
        );
        // OOB (>= len and < 0) → fallback (None), like the helper's bail.
        assert_eq!(m.call(id_int, &[ints_ptr, 3], &[ilen, 0]).completed(), None);
        assert_eq!(
            m.call(id_int, &[ints_ptr, -1], &[ilen, 0]).completed(),
            None
        );

        // fn(a: FlatFloat, i: Int) -> Float { return a[i] }
        let id_f = m
            .compile(&ft(
                2,
                vec![FlatFloat, Int, Float],
                vec![
                    JitInstr::ListGetFloatDirect {
                        dst: 2,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let floats: Vec<f64> = vec![1.5, 2.5, 3.5];
        let fptr = floats.as_ptr() as i64;
        let flen = floats.len() as i64;
        let read = |i: i64| {
            m.call(id_f, &[fptr, i], &[flen, 0])
                .completed()
                .map(|b| f64::from_bits(b as u64))
        };
        assert_eq!(read(1), Some(2.5));
        assert_eq!(read(0), Some(1.5));
        assert_eq!(read(3), None);

        // fn(a: FlatInt) -> Int { return len(a) }  via ListLenDirect
        let id_len = m
            .compile(&ft(
                1,
                vec![FlatInt, Int],
                vec![
                    JitInstr::ListLenDirect { dst: 1, base: 0 },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        assert_eq!(m.call(id_len, &[ints_ptr], &[ilen]).completed(), Some(3));
    }

    #[test]
    fn distinct_bail_sites_get_stable_safepoint_ids() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();

        // fn(a: FlatInt, x: Int, i: Int) -> Int { t = x + x; return a[i] }
        // Two distinct bail sites: the `Add` overflow guard (site 1) precedes the
        // `ListGetIntDirect` OOB guard (site 2). regs 0=a,1=x,2=i,3=t,4=res.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let ints: Vec<i64> = vec![10, 20, 30];
        let ints_ptr = ints.as_ptr() as i64;
        let ilen = ints.len() as i64;

        // Bail at the FIRST site: x + x overflows, so the `Add` guard fires (id 1)
        // before the list read is ever reached.
        assert!(matches!(
            m.call(id, &[ints_ptr, i64::MAX, 0], &[ilen, 0, 0]),
            NativeOutcome::Deopt {
                safepoint_id: SafepointId(1),
                ..
            }
        ));
        // Pass the first guard (small x, no overflow) but bail at the SECOND site:
        // index 5 is out of bounds, so the direct-read OOB guard fires (id 2).
        assert!(matches!(
            m.call(id, &[ints_ptr, 1, 5], &[ilen, 0, 0]),
            NativeOutcome::Deopt {
                safepoint_id: SafepointId(2),
                ..
            }
        ));
        // Both guards pass → completes (id stays 0 = no bail recorded).
        assert!(matches!(
            m.call(id, &[ints_ptr, 1, 2], &[ilen, 0, 0]),
            NativeOutcome::Completed(_)
        ));
    }

    // --- J0.1a: deopt state-map (must-analysis) -------------------------------

    #[test]
    fn deopt_map_straightline_single_guard() {
        // fn(a, b) { t = a + b; return t }  regs 0=a,1=b,2=t. The `Add` (ip 0) has
        // one overflow guard (site id 1) → one site, resuming at ip 0 with the two
        // params live (t is not yet assigned on entry to its own instruction).
        let mut m = module();
        let id = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let map = m.deopt_map(id).expect("map for valid id");
        assert_eq!(map.sites.len(), 1);
        assert_eq!(map.sites[0].resume_ip, 0);
        assert_eq!(
            map.sites[0].live,
            vec![(0, JitValueType::Int), (1, JitValueType::Int)]
        );
    }

    #[test]
    fn profiled_branch_deopts_on_cold_edge() {
        let mut m = module();
        let id = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::ProfiledJumpIfIntCompare {
                        lhs: 0,
                        rhs: 1,
                        op: JitCompare::Lt,
                        expected: true,
                        target: 3,
                        hot_target: false,
                    },
                    JitInstr::LoadInt { dst: 2, value: 10 },
                    JitInstr::Return { src: 2 },
                    JitInstr::LoadInt { dst: 2, value: 99 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();

        assert_eq!(m.call(id, &[5, 3], &[0, 0]).completed(), Some(10));
        match m.call(id, &[1, 3], &[0, 0]) {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                assert_eq!(safepoint_id, SafepointId(1));
                assert_eq!(
                    live.iter().find(|reg| reg.reg == 0).map(|reg| reg.value),
                    Some(DeoptValue::Int(1))
                );
                assert_eq!(
                    live.iter().find(|reg| reg.reg == 1).map(|reg| reg.value),
                    Some(DeoptValue::Int(3))
                );
            }
            other => panic!("expected profiled cold edge to deopt, got {other:?}"),
        }
    }

    #[test]
    fn deopt_map_two_distinct_sites_track_prior_defs() {
        use JitValueType::{FlatInt, Int};
        // fn(a: FlatInt, x, i) { t = x + x; return a[i] }  regs 0=a,1=x,2=i,3=t,4=res.
        // Site 1: the `Add` overflow guard at ip 0 (t not yet live). Site 2: the
        // `ListGetIntDirect` OOB guard at ip 1 — by then `t` (reg 3) is definitely
        // assigned, so it appears in site 2's live set but not site 1's. (Mirrors
        // `distinct_bail_sites_get_stable_safepoint_ids`.)
        let mut m = module();
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let map = m.deopt_map(id).expect("map for valid id");
        assert_eq!(map.sites.len(), 2);

        // Site 1 (id 1): resume at the Add (ip 0); params live, t (reg 3) is NOT.
        assert_eq!(map.sites[0].resume_ip, 0);
        assert!(!map.sites[0].live.iter().any(|(r, _)| *r == 3));
        // Params 0..3 are live (a is FlatInt, x/i are Int).
        assert_eq!(
            map.sites[0].live,
            vec![
                (0, JitValueType::FlatInt),
                (1, JitValueType::Int),
                (2, JitValueType::Int)
            ]
        );

        // Site 2 (id 2): resume at the direct read (ip 1); t (reg 3) is now live.
        assert_eq!(map.sites[1].resume_ip, 1);
        assert!(map.sites[1].live.contains(&(3, JitValueType::Int)));
    }

    #[test]
    fn deopt_map_must_analysis_excludes_one_armed_def() {
        // A register assigned on only ONE arm before a join with a guard must NOT be
        // in the join's live set (intersection / must-analysis).
        //
        //   0: if cond(reg1) goto 3            (cond is param reg 1)
        //   1:   t(reg3) = a(reg0) + a(reg0)   only the fall-through arm assigns t
        //   2:   goto 4
        //   3:   nop                           the taken arm leaves t unassigned
        //   4:   u(reg4) = a + a               guard here joins both arms
        //   5:   return u
        // regs: 0=a, 1=cond, 2=(unused scratch), 3=t, 4=u.
        use JitValueType::Int;
        let mut m = module();
        let id = m
            .compile(&ft(
                2,
                vec![Int, Int, Int, Int, Int],
                vec![
                    JitInstr::JumpIfBool {
                        cond: 1,
                        expected: true,
                        target: 3,
                    },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 0,
                        rhs: 0,
                    },
                    JitInstr::Jump { target: 4 },
                    JitInstr::Nop,
                    JitInstr::Add {
                        dst: 4,
                        lhs: 0,
                        rhs: 0,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let map = m.deopt_map(id).expect("map for valid id");
        // Two Add guards: site 1 at ip 1, site 2 at the post-join ip 4.
        assert_eq!(map.sites.len(), 2);
        assert_eq!(map.sites[0].resume_ip, 1);
        assert_eq!(map.sites[1].resume_ip, 4);
        // The key assertion: at the post-join guard (ip 4), `t` (reg 3) is assigned
        // on only one arm, so intersection excludes it from the live set.
        assert!(
            !map.sites[1].live.iter().any(|(r, _)| *r == 3),
            "reg 3 assigned on only one arm must not be live at the join: {:?}",
            map.sites[1].live
        );
        // The params (regs 0 and 1) are assigned on every path → still live.
        assert!(map.sites[1].live.contains(&(0, JitValueType::Int)));
        assert!(map.sites[1].live.contains(&(1, JitValueType::Int)));
        // On the fall-through arm's own guard (ip 1) t is also not-yet live.
        assert!(!map.sites[0].live.iter().any(|(r, _)| *r == 3));
    }

    #[test]
    fn deopt_map_rejects_foreign_id() {
        // A foreign / out-of-range id yields no map, mirroring `call`'s validation.
        let mut m1 = module();
        let mut m2 = module();
        let id1 = m1.compile(&two_param_add()).unwrap();
        let _id2 = m2.compile(&two_param_add()).unwrap();
        assert!(m1.deopt_map(id1).is_some());
        assert!(m2.deopt_map(id1).is_none());
    }

    #[test]
    fn compiles_and_runs_add() {
        let mut m = module();
        // fn(a, b) { return a + b }   regs: 0=a,1=b,2=tmp
        let id = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id, &[3, 4]), Some(7));
        assert_eq!(m.callt(id, &[-10, 4]), Some(-6));
        // overflow bails:
        assert_eq!(m.callt(id, &[i64::MAX, 1]), None);
    }

    #[test]
    fn loop_sum_to_n() {
        // fn(n) { total=0; i=1; while i<=n { total+=i; i+=1 } return total }
        // regs: 0=n, 1=total, 2=i, 3=one
        let mut m = module();
        let code = vec![
            JitInstr::LoadInt { dst: 1, value: 0 }, // 0 total=0
            JitInstr::LoadInt { dst: 2, value: 1 }, // 1 i=1
            JitInstr::LoadInt { dst: 3, value: 1 }, // 2 one=1
            // 3: loop head: if !(i<=n) goto end(8)
            JitInstr::JumpIfIntCompare {
                lhs: 2,
                rhs: 0,
                op: JitCompare::Le,
                expected: false,
                target: 8,
            },
            JitInstr::Add {
                dst: 1,
                lhs: 1,
                rhs: 2,
            }, // 4 total+=i
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 3,
            }, // 5 i+=1
            JitInstr::Jump { target: 3 }, // 6 loop
            JitInstr::Nop,                // 7 (padding leader)
            JitInstr::Return { src: 1 },  // 8 end
        ];
        let id = m.compile(&f(1, 4, code)).unwrap();
        assert_eq!(m.callt(id, &[10]), Some(55));
        assert_eq!(m.callt(id, &[0]), Some(0));
        assert_eq!(m.callt(id, &[100]), Some(5050));
    }

    #[test]
    fn osr_entry_runs_loop_and_exits_with_live_out() {
        // Same loop as `loop_sum_to_n`, but compiled as an OSR-entry at the loop
        // header (ip 3) with the post-loop `Return` replaced by `OsrExit`. The
        // entry loads the live-in window (regs definitely-assigned at ip 3: total,
        // i, one, n) and jumps into the loop; the loop runs natively; on exit it
        // deopts at ip 8 with the live-out window (`total`).
        let mut m = module();
        let code = vec![
            JitInstr::LoadInt { dst: 1, value: 0 }, // 0 total=0 (pre-loop; not run under OSR)
            JitInstr::LoadInt { dst: 2, value: 1 }, // 1 i=1
            JitInstr::LoadInt { dst: 3, value: 1 }, // 2 one=1
            // 3: loop head: if !(i<=n) goto end(8)
            JitInstr::JumpIfIntCompare {
                lhs: 2,
                rhs: 0,
                op: JitCompare::Le,
                expected: false,
                target: 8,
            },
            JitInstr::Add {
                dst: 1,
                lhs: 1,
                rhs: 2,
            }, // 4 total+=i
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 3,
            }, // 5 i+=1
            JitInstr::Jump { target: 3 }, // 6 loop
            JitInstr::Nop,                // 7 (padding leader)
            JitInstr::OsrExit,            // 8 OSR-exit (was Return)
        ];
        let prog = f(1, 4, code);
        let id = m.compile_osr(&prog, 3).unwrap();
        // The window is `n_regs`-wide, indexed by register. Seed the loop live-in:
        // total=0 (reg1), i=1 (reg2), one=1 (reg3), n (reg0). lens parallel & unused.
        let run = |n: i64| -> NativeOutcome {
            let window = [n, 0, 1, 1]; // reg0=n, reg1=total, reg2=i, reg3=one
            let lens = [0i64; 4];
            m.call(id, &window, &lens)
        };
        for &(n, expected) in &[(10i64, 55i64), (0, 0), (100, 5050)] {
            match run(n) {
                NativeOutcome::Deopt {
                    safepoint_id, live, ..
                } => {
                    let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                    assert_eq!(site.resume_ip, 8, "OSR-exit resumes at the post-loop ip");
                    let total = live
                        .iter()
                        .find(|r| r.reg == 1)
                        .map(|r| match r.value {
                            DeoptValue::Int(v) => v,
                            DeoptValue::Float(_) => panic!("total is Int"),
                        })
                        .expect("total is live-out");
                    assert_eq!(total, expected, "live-out total for n={n}");
                }
                NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => {
                    panic!("OSR loop must deopt at exit, not complete")
                }
            }
        }
    }

    #[test]
    fn osr_window_flat_list_direct_read_and_len() {
        // OSR loop summing a flat List<Int> that lives in a NON-param window slot
        // (reg index >= n_params). Models a loop-invariant typed list marshalled into
        // the OSR live-in window as a flat buffer: in-loop `List.get` lowers to
        // `ListGetIntDirect` and `List.len` to `ListLenDirect`, both basing off the
        // window register. This exercises the relaxed flat-base gate (admits an
        // OSR-window register, not only a top-level param).
        //
        // regs: 0=xs(FlatInt, NON-param window slot), 1=len, 2=i, 3=acc, 4=one, 5=elem
        // n_params=0. The flat list and every loop-carried value enter via the live-in
        // window (definite-assignment includes a register read-but-never-written in the
        // loop, exactly as `translate_osr_loop` produces for a pre-loop-built list whose
        // pre-header init instructions become `Bail` — no linear pred excludes it). The
        // pre-loop region is `Bail` (never run under OSR), so the header's only preds are
        // the loop backedge, keeping `xs`/`len`/`i`/`acc`/`one` definitely-assigned there.
        use JitValueType::{FlatInt, Int};
        let mut m = module();
        let code = vec![
            JitInstr::Bail, // 0 pre-loop (never run under OSR; no successor)
            JitInstr::Bail, // 1
            JitInstr::Bail, // 2
            JitInstr::Bail, // 3
            // 4: loop head (OSR header): if !(i < len) goto end(9)
            JitInstr::JumpIfIntCompare {
                lhs: 2,
                rhs: 1,
                op: JitCompare::Lt,
                expected: false,
                target: 9,
            },
            JitInstr::ListGetIntDirect {
                dst: 5,
                base: 0,
                index: 2,
            }, // 5 elem = xs[i]
            JitInstr::Add {
                dst: 3,
                lhs: 3,
                rhs: 5,
            }, // 6 acc += elem
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 4,
            }, // 7 i += 1
            JitInstr::Jump { target: 4 }, // 8 loop
            JitInstr::OsrExit,            // 9 exit (live-out: acc)
        ];
        let prog = ft(0, vec![FlatInt, Int, Int, Int, Int, Int], code);
        // OSR header at the loop head (ip 4). regs 0,1,2,3,4 are definitely assigned
        // there (read-only live-ins, never written in-loop).
        let id = m.compile_osr(&prog, 4).unwrap();

        let data: Vec<i64> = vec![10, 20, 30, 40];
        // The window is n_regs-wide; reg0's args slot holds the raw data pointer and
        // reg0's lens slot holds the element count (the flat-buffer ABI, by register).
        // Seed the loop live-in: len=4 (reg1), i=0 (reg2), acc=0 (reg3), one=1 (reg4).
        let mut window = [0i64; 6];
        window[0] = data.as_ptr() as i64;
        window[1] = data.len() as i64; // len (hoisted, live-in)
        window[2] = 0; // i
        window[3] = 0; // acc
        window[4] = 1; // one
        let mut lens = [0i64; 6];
        lens[0] = data.len() as i64;
        match m.call(id, &window, &lens) {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                assert_eq!(site.resume_ip, 9, "exits at post-loop ip");
                let acc = live
                    .iter()
                    .find(|r| r.reg == 3)
                    .map(|r| match r.value {
                        DeoptValue::Int(v) => v,
                        DeoptValue::Float(_) => panic!("acc is Int"),
                    })
                    .expect("acc is live-out");
                assert_eq!(acc, 100, "sum of [10,20,30,40] via direct reads");
            }
            other => panic!("OSR loop must deopt at exit, got {other:?}"),
        }
        // OOB safety: a window whose lens claims more elements than the buffer has
        // would read OOB — but every direct read is bounds-checked against lens, so a
        // shorter real loop bound (len) keeps every index in range. To prove the OOB
        // guard itself bails, force an index past the buffer by lying about len upward
        // is unsound for the test buffer; instead, an empty buffer (len 0) must make
        // the very first `i < len` false and exit immediately with acc=0.
        let mut empty_window = [0i64; 6];
        let empty: Vec<i64> = vec![];
        empty_window[0] = empty.as_ptr() as i64;
        empty_window[1] = 0; // len = 0
        empty_window[4] = 1; // one
        let mut empty_lens = [0i64; 6];
        empty_lens[0] = 0;
        match m.call(id, &empty_window, &empty_lens) {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                assert_eq!(site.resume_ip, 9);
                let acc = live.iter().find(|r| r.reg == 3).map(|r| match r.value {
                    DeoptValue::Int(v) => v,
                    DeoptValue::Float(_) => panic!(),
                });
                assert_eq!(acc, Some(0), "empty list sums to 0");
            }
            other => panic!("empty OSR loop must deopt at exit, got {other:?}"),
        }

        // ListLenDirect in-loop off a NON-param flat window register + OOB direct read.
        // regs: 0=xs(FlatInt), 1=i, 2=acc, 3=one, 4=len, 5=elem. The loop reads len
        // directly each iteration (ListLenDirect base=0) and indexes xs[i]; an `i`
        // pushed past `len` (here we drive the loop bound from a SEPARATE register `b`
        // larger than the buffer) makes the direct read OOB ⇒ a bounds-check bail/deopt
        // (NOT UB), matching the host helper's OOB bail.
        // regs: 0=xs, 1=i, 2=acc, 3=one, 4=len(unused-here), 5=elem, 6=bound
        let code2 = vec![
            JitInstr::Bail, // 0 pre-loop
            JitInstr::Bail, // 1
            JitInstr::Bail, // 2
            JitInstr::Bail, // 3
            // 4: header: if !(i < bound) goto end(10)
            JitInstr::JumpIfIntCompare {
                lhs: 1,
                rhs: 6,
                op: JitCompare::Lt,
                expected: false,
                target: 10,
            },
            JitInstr::ListLenDirect { dst: 4, base: 0 }, // 5 len = len(xs)  (direct)
            JitInstr::ListGetIntDirect {
                dst: 5,
                base: 0,
                index: 1,
            }, // 6 elem = xs[i] (OOB once i>=len)
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 5,
            }, // 7 acc += elem
            JitInstr::Add {
                dst: 1,
                lhs: 1,
                rhs: 3,
            }, // 8 i += 1
            JitInstr::Jump { target: 4 },                // 9 loop
            JitInstr::OsrExit,                           // 10 exit
        ];
        let prog2 = ft(
            0,
            vec![JitValueType::FlatInt, Int, Int, Int, Int, Int, Int],
            code2,
        );
        let id2 = m.compile_osr(&prog2, 4).unwrap();
        // Drive bound=8 but the buffer only has 4 elements ⇒ at i==4 the direct read is
        // OOB and must deopt (a bounds bail), never reading past the buffer.
        let mut w2 = [0i64; 7];
        w2[0] = data.as_ptr() as i64; // xs
        w2[1] = 0; // i
        w2[2] = 0; // acc
        w2[3] = 1; // one
        w2[6] = 8; // bound (> len 4)
        let mut l2 = [0i64; 7];
        l2[0] = data.len() as i64; // lens[xs] = 4 — the bounds-check source
        match m.call(id2, &w2, &l2) {
            NativeOutcome::Deopt { safepoint_id, .. } => {
                let site = m.deopt_map(id2).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                // The OOB read bails at the ListGetIntDirect ip (6), NOT the exit (10):
                // a precise mid-loop deopt, so the interpreter re-runs and raises the
                // real out-of-bounds behavior itself.
                assert_eq!(
                    site.resume_ip, 6,
                    "OOB direct read bails at its own ip (not UB)"
                );
            }
            other => panic!("OOB direct read must deopt, got {other:?}"),
        }
    }

    #[test]
    fn osr_flat_base_gate_rejects_nonwindow_non_osr() {
        // The relaxed flat-base gate: under OSR a flat base may be any register in the
        // n_regs-wide window (index >= n_params); under a NORMAL compile it must still
        // be a packed param (index < n_params). A non-OSR program with a flat base at a
        // non-param register is rejected by validation.
        use JitValueType::{FlatInt, Int};
        // n_params=1 (reg0 the only param), reg2 is a FlatInt non-param ⇒ illegal base
        // for a normal compile.
        let prog = ft(
            1,
            vec![Int, Int, FlatInt, Int],
            vec![
                JitInstr::ListGetIntDirect {
                    dst: 1,
                    base: 2,
                    index: 0,
                },
                JitInstr::Return { src: 1 },
            ],
        );
        assert!(
            super::validate(&prog, false).is_err(),
            "non-param flat base must be rejected by a normal compile"
        );
        // Under OSR the same shape validates (the window is n_regs-wide).
        assert!(
            super::validate(&prog, true).is_ok(),
            "an OSR-window flat base (index >= n_params) must validate"
        );
    }

    #[test]
    fn osr_rejects_non_leader_header() {
        // An OSR header ip that is not a leader / jump-target block is rejected
        // cleanly (no panic, no miscompile).
        let mut m = module();
        let prog = f(
            1,
            2,
            vec![
                JitInstr::Add {
                    dst: 1,
                    lhs: 0,
                    rhs: 0,
                },
                JitInstr::Return { src: 1 },
            ],
        );
        // ip 1 (the Return) is a leader only if a jump targets it; here none does,
        // and it is not ip 0, so it has no block.
        assert!(m.compile_osr(&prog, 1).is_err());
    }

    #[test]
    fn div_by_zero_bails() {
        let mut m = module();
        let id = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Div {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id, &[20, 5]), Some(4));
        assert_eq!(m.callt(id, &[20, 0]), None);
        assert_eq!(m.callt(id, &[i64::MIN, -1]), None);
    }

    // --- J0.1b: live-register value capture at deopt --------------------------

    /// Find the captured value of register `reg` in a deopt outcome's `live` set.
    fn live_value(outcome: &NativeOutcome, reg: u32) -> Option<DeoptValue> {
        match outcome {
            NativeOutcome::Deopt { live, .. } => {
                live.iter().find(|r| r.reg == reg).map(|r| r.value)
            }
            NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => None,
        }
    }

    #[test]
    fn deopt_capture_records_live_register_values() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();
        // fn(xs: FlatInt, a: Int, b: Int) -> Int { t = a + b; return xs[t] }
        // regs 0=xs, 1=a, 2=b, 3=t. The `ListGetIntDirect` OOB guard (ip 1) resumes
        // with xs(0)/a(1)/b(2)/t(3) all definitely assigned on entry.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 3,
                        base: 0,
                        index: 3,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        let xs: Vec<i64> = vec![10, 20, 30, 40, 50];
        let xs_ptr = xs.as_ptr() as i64;
        let xlen = xs.len() as i64;

        // In range: t = 1 + 2 = 3 → xs[3] = 40.
        assert_eq!(
            m.call(id, &[xs_ptr, 1, 2], &[xlen, 0, 0]),
            NativeOutcome::Completed(40)
        );

        // Out of range: t = 3 + 4 = 7 >= len 5 → the direct-read OOB guard bails.
        let out = m.call(id, &[xs_ptr, 3, 4], &[xlen, 0, 0]);
        assert!(matches!(out, NativeOutcome::Deopt { .. }));
        // t (reg 3) was computed before the guard fired and is captured.
        assert_eq!(live_value(&out, 3), Some(DeoptValue::Int(7)));
        // The params a (reg 1) and b (reg 2) are captured with their passed values.
        assert_eq!(live_value(&out, 1), Some(DeoptValue::Int(3)));
        assert_eq!(live_value(&out, 2), Some(DeoptValue::Int(4)));
    }

    #[test]
    fn deopt_capture_records_float_register_value() {
        use JitValueType::{FlatInt, Float, Int};
        let mut m = module();
        // fn(xs: FlatInt, i: Int, f: Float) -> Int { g = f + f; return xs[i] }
        // regs 0=xs, 1=i, 2=f, 3=g. The float `g` is definitely assigned before the
        // `ListGetIntDirect` OOB guard (ip 2), so it is captured as an exact f64.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Float, Float],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 2,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 1,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        let xs: Vec<i64> = vec![7];
        let xs_ptr = xs.as_ptr() as i64;
        let xlen = xs.len() as i64;
        let f = 1.5_f64;

        // Out of range index 9 → bail; the float g = f + f = 3.0 round-trips exactly.
        let out = m.call(id, &[xs_ptr, 9, f.to_bits() as i64], &[xlen, 0, 0]);
        assert!(matches!(out, NativeOutcome::Deopt { .. }));
        assert_eq!(live_value(&out, 3), Some(DeoptValue::Float(f + f)));
        // The float param f itself is captured exactly too.
        assert_eq!(live_value(&out, 2), Some(DeoptValue::Float(f)));
    }

    // --- J0.3: deopt-at-every-safepoint stress test (master correctness) ------

    /// Force a bail at EVERY safepoint of a few representative functions and verify
    /// the captured safepoint id + live register values are correct at each — even at
    /// safepoints that never fire under the (deliberately in-range, non-overflowing)
    /// inputs. This exercises the J0 capture/map machinery exhaustively: for every
    /// site `k`, `compile_forcing_bail(f, k)` makes only site `k` bail, and we assert
    /// the outcome is `Deopt { SafepointId(k) }` whose `live` set is exactly the one
    /// `deopt_map().sites[k-1]` advertises, each register carrying the value the
    /// function computes for it. Late sites must capture earlier intermediates.
    #[test]
    fn force_bail_at_every_safepoint_captures_correct_state() {
        use JitValueType::{FlatInt, Float, Int};

        // A representative case: a `JitFunction`, the in-range inputs (args/lens) that
        // make NO natural bail fire, and a closure giving the value the function
        // computes for each register at any safepoint, so we can check every capture.
        struct Case {
            name: &'static str,
            func: JitFunction,
            args: Vec<i64>,
            lens: Vec<i64>,
            // reg -> captured DeoptValue, for any reg appearing in a site's live set.
            expect: Box<dyn Fn(u32) -> DeoptValue>,
        }

        let ints: Vec<i64> = vec![10, 20, 30, 40, 50];
        let ints_ptr = ints.as_ptr() as i64;
        let ilen = ints.len() as i64;

        // Case A: fn(a: FlatInt, x: Int, i: Int) { t = x + x; return a[i] }
        // Sites: 1 = Add overflow guard (ip 0), 2 = ListGetIntDirect OOB (ip 1).
        // Site 2 is LATE and captures the earlier-computed t = x + x.
        let a_ptr = ints_ptr;
        let case_a = Case {
            name: "add-then-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ),
            // a = ptr, x = 7, i = 2 (in range). t = x + x = 14.
            args: vec![a_ptr, 7, 2],
            lens: vec![ilen, 0, 0],
            expect: Box::new(|reg| match reg {
                0 => DeoptValue::Int(0), // a: ptr value is asserted separately
                1 => DeoptValue::Int(7),
                2 => DeoptValue::Int(2),
                3 => DeoptValue::Int(14),
                _ => DeoptValue::Int(0),
            }),
        };

        // Case B: fn(xs: FlatInt, i: Int, f: Float) { g = f + f; return xs[i] }
        // The float Add has no guard, so the only site is the ListGetIntDirect OOB
        // (ip 1). Its live set includes the float register g — checked as exact f64.
        let xs_ptr = ints_ptr;
        let fv = 1.25_f64;
        let case_b = Case {
            name: "float-reg-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Float, Float],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 2,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 1,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 1 },
                ],
            ),
            // xs = ptr, i = 1 (in range), f = 1.25. g = f + f = 2.5.
            args: vec![xs_ptr, 1, fv.to_bits() as i64],
            lens: vec![ilen, 0, 0],
            expect: Box::new(move |reg| match reg {
                1 => DeoptValue::Int(1),
                2 => DeoptValue::Float(fv),
                3 => DeoptValue::Float(fv + fv),
                _ => DeoptValue::Int(0),
            }),
        };

        // Case C: fn(a: FlatInt, x: Int, y: Int) { p = x + y; q = p * x; return a[y] }
        // Three sites: 1 = Add (ip 0), 2 = Mul (ip 1), 3 = ListGetIntDirect OOB (ip 2).
        // The LATE site 3 captures both earlier intermediates p and q.
        let case_c = Case {
            name: "add-mul-then-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::Mul {
                        dst: 4,
                        lhs: 3,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 5,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 5 },
                ],
            ),
            // a = ptr, x = 3, y = 4 (in range). p = 3 + 4 = 7, q = p * x = 21.
            args: vec![ints_ptr, 3, 4],
            lens: vec![ilen, 0, 0],
            expect: Box::new(|reg| match reg {
                1 => DeoptValue::Int(3),
                2 => DeoptValue::Int(4),
                3 => DeoptValue::Int(7),  // p
                4 => DeoptValue::Int(21), // q
                _ => DeoptValue::Int(0),
            }),
        };

        let cases = [case_a, case_b, case_c];
        let mut combinations = 0usize;
        let mut late_intermediate_checks = 0usize;

        for case in &cases {
            let mut m = module();
            // Site count from the natural (un-forced) compilation.
            let base_id = m.compile(&case.func).unwrap();
            let n = m.deopt_map(base_id).expect("map for valid id").sites.len();
            assert!(n >= 1, "{}: expected at least one safepoint", case.name);

            for k in 1..=n as u32 {
                // Force ONLY site k to bail; inputs are chosen so no natural bail fires.
                let id = m.compile_forcing_bail(&case.func, k).unwrap();
                let site = m.deopt_map(id).expect("map").sites[(k - 1) as usize].clone();
                let out = m.call(id, &case.args, &case.lens);

                // The forced site must bail with exactly its id.
                let live = match &out {
                    NativeOutcome::Deopt {
                        safepoint_id, live, ..
                    } => {
                        assert_eq!(
                            *safepoint_id,
                            SafepointId(k),
                            "{}: forced site {} reported wrong safepoint id",
                            case.name,
                            k
                        );
                        live
                    }
                    NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => {
                        panic!("{}: forced site {} did not bail", case.name, k)
                    }
                };

                // The captured live registers must be exactly the map's live set...
                let mut captured: Vec<u32> = live.iter().map(|r| r.reg).collect();
                captured.sort_unstable();
                let mut expected_regs: Vec<u32> = site.live.iter().map(|(r, _)| *r).collect();
                expected_regs.sort_unstable();
                assert_eq!(
                    captured, expected_regs,
                    "{}: site {} live-reg set mismatch (map vs capture)",
                    case.name, k
                );

                // ...and each captured value must match what the function computes,
                // including FlatInt pointer registers (checked as the raw arg word).
                for &(reg, _ty) in &site.live {
                    let got = live_value(&out, reg).expect("captured reg present");
                    if case.func.reg_types[reg as usize] == FlatInt {
                        assert_eq!(
                            got,
                            DeoptValue::Int(case.args[reg as usize]),
                            "{}: site {} reg {} (flat ptr) value mismatch",
                            case.name,
                            k,
                            reg
                        );
                    } else {
                        assert_eq!(
                            got,
                            (case.expect)(reg),
                            "{}: site {} reg {} value mismatch",
                            case.name,
                            k,
                            reg
                        );
                    }
                }

                combinations += 1;
            }

            // Explicit late-site check: the LAST site of a multi-site function must
            // capture an earlier-computed intermediate with its correct value.
            if n >= 2 {
                let id = m.compile_forcing_bail(&case.func, n as u32).unwrap();
                let out = m.call(id, &case.args, &case.lens);
                // reg 3 is the first arithmetic result in cases A and C; it is computed
                // at an earlier site yet must be captured at the final site.
                assert_eq!(
                    live_value(&out, 3),
                    Some((case.expect)(3)),
                    "{}: late site {} failed to capture earlier intermediate reg 3",
                    case.name,
                    n
                );
                late_intermediate_checks += 1;
            }
        }

        // Sanity: we actually exercised every site of every case plus late checks.
        assert_eq!(combinations, 2 + 1 + 3, "unexpected (case, site) coverage");
        assert!(late_intermediate_checks >= 2, "expected late-site checks");
    }

    #[test]
    fn compile_forcing_all_bails_deopts_at_first_executed_safepoint() {
        use JitValueType::{FlatInt, Int};

        let values = vec![10, 20, 30];
        let ptr = values.as_ptr() as i64;
        let func = ft(
            3,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 1,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 2,
                },
                JitInstr::Return { src: 4 },
            ],
        );

        let mut m = module();
        let id = m.compile_forcing_all_bails(&func).unwrap();
        assert_eq!(
            m.deopt_map(id).expect("map").sites.len(),
            2,
            "test function should have both add-overflow and direct-list guards",
        );
        let out = m.call(id, &[ptr, 7, 1], &[values.len() as i64, 0, 0]);
        match out {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                assert_eq!(safepoint_id, SafepointId(1));
                assert_eq!(
                    live.iter().find(|reg| reg.reg == 1).map(|reg| reg.value),
                    Some(DeoptValue::Int(7))
                );
            }
            other => panic!("expected forced all-sites deopt, got {other:?}"),
        }
    }

    // --- IR validation: malformed public IR must fail cleanly, not panic ---

    #[test]
    fn rejects_out_of_range_register() {
        // `Add` reads register 5 in a 3-register function.
        let err = validate(&f(
            1,
            3,
            vec![JitInstr::Add {
                dst: 0,
                lhs: 5,
                rhs: 1,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("out of range"), "{}", err.0);
    }

    #[test]
    fn rejects_out_of_range_jump_target() {
        let err = validate(&f(1, 1, vec![JitInstr::Jump { target: 9 }])).unwrap_err();
        assert!(err.0.contains("target 9"), "{}", err.0);
    }

    #[test]
    fn rejects_out_of_range_cold_block_hint() {
        let mut prog = f(1, 1, vec![JitInstr::Return { src: 0 }]);
        prog.cold_blocks.push(3);
        let err = validate(&prog).unwrap_err();
        assert!(err.0.contains("cold block instruction 3"), "{}", err.0);
    }

    #[test]
    fn compiles_valid_cold_block_hint() {
        let mut m = module();
        let mut prog = f(
            1,
            1,
            vec![
                JitInstr::JumpIfBool {
                    cond: 0,
                    expected: true,
                    target: 2,
                },
                JitInstr::Return { src: 0 },
                JitInstr::Return { src: 0 },
            ],
        );
        prog.cold_blocks.push(2);
        m.compile(&prog)
            .expect("cold block metadata must be a layout-only hint");
    }

    #[test]
    fn rejects_conditional_branch_without_fallthrough() {
        // A trailing conditional branch has no `i + 1` to fall through to.
        let err = validate(&f(
            1,
            1,
            vec![JitInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 0,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("fall-through"), "{}", err.0);
    }

    #[test]
    fn rejects_reg_types_length_mismatch() {
        let bad = JitFunction {
            n_params: 0,
            n_regs: 3,
            reg_types: vec![JitValueType::Int; 2],
            code: vec![],
            cold_blocks: Vec::new(),
        };
        let err = validate(&bad).unwrap_err();
        assert!(err.0.contains("reg_types length"), "{}", err.0);
    }

    #[test]
    fn rejects_params_exceeding_regs() {
        let err = validate(&f(4, 2, vec![])).unwrap_err();
        assert!(err.0.contains("n_params"), "{}", err.0);
    }

    #[test]
    fn rejects_int_op_on_float_register() {
        use JitValueType::{Float, Int};
        // `Mod` (integer-only) applied to float registers.
        let err = validate(&ft(
            2,
            vec![Float, Float, Int],
            vec![JitInstr::Mod {
                dst: 2,
                lhs: 0,
                rhs: 1,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("must be Int"), "{}", err.0);
    }

    #[test]
    fn rejects_mismatched_arith_classes() {
        use JitValueType::{Float, Int};
        // `Add` with one int and one float operand.
        let err = validate(&ft(
            2,
            vec![Int, Float, Int],
            vec![JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("classes differ"), "{}", err.0);
    }

    #[test]
    fn rejects_handle_outside_heap_read_base() {
        use JitValueType::{Handle, Int};
        // A `Handle` register used as an arithmetic operand.
        let err = validate(&ft(
            2,
            vec![Handle, Int, Int],
            vec![JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("Handle"), "{}", err.0);
    }

    #[test]
    fn rejects_non_handle_heap_read_base() {
        // `FieldInt` host helper base must be a `Handle`, not an `Int`.
        let err = validate(&f(
            1,
            2,
            vec![JitInstr::HostCall {
                helper: HostHelper::FieldInt,
                dst: 1,
                args: vec![HostArg::Reg(0), HostArg::ImmI64(0)],
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("expected Handle"), "{}", err.0);
    }

    #[test]
    fn accepts_well_formed_heap_read() {
        use JitValueType::{Handle, Int};
        validate(&ft(
            1,
            vec![Handle, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::ListLen,
                    dst: 1,
                    args: vec![HostArg::Reg(0)],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .expect("well-formed heap read should validate");
    }

    // fn(a, b) { return a + b } — a 2-param function for the call-guard tests.
    fn two_param_add() -> JitFunction {
        f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        )
    }

    #[test]
    fn call_rejects_wrong_arg_count() {
        // The generated entry block reads exactly `n_params` words from `args_ptr`,
        // so a short slice must be rejected by `call` (otherwise: out-of-bounds
        // read). Both too-few and too-many fall back rather than misread memory.
        let mut m = module();
        let id = m.compile(&two_param_add()).unwrap();
        assert_eq!(m.callt(id, &[2, 3]), Some(5));
        assert_eq!(m.callt(id, &[2]), None); // too few — must not read past the slice
        assert_eq!(m.callt(id, &[]), None);
        assert_eq!(m.callt(id, &[2, 3, 4]), None); // too many
    }

    #[test]
    fn call_rejects_id_from_another_module() {
        // A `CompiledId` minted by one module indexes that module's table; using it
        // against another module must be rejected, not silently mis-dispatched.
        let mut m1 = module();
        let mut m2 = module();
        let id1 = m1.compile(&two_param_add()).unwrap();
        let _id2 = m2.compile(&two_param_add()).unwrap();
        assert_eq!(m1.callt(id1, &[2, 3]), Some(5));
        assert_eq!(m2.callt(id1, &[2, 3]), None); // foreign id → fallback, no panic
    }

    // --- Structured fuzz: validate/compile robustness (execution spec §7) ------
    //
    // The contract is that `compile` is *total* over arbitrary `JitFunction`
    // values: a producer bug (out-of-range register, type-mismatched operand,
    // wild jump target, truncated stream) MUST surface as a clean `JitError` —
    // never a panic, never undefined behaviour, never silently-wrong machine code.
    // These tests drive thousands of random and mutation-derived programs through
    // `compile` (which runs `validate` then Cranelift codegen) and assert it
    // always returns (`Ok` or `Err`). Miscompile detection is the differential
    // suite's job (compile-vs-interpreter on real programs); here we only pin
    // robustness. Randomness is a fixed-seed xorshift so failures are reproducible
    // without an external rng/proptest dependency.

    /// Deterministic xorshift64* PRNG — reproducible, no external dep.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u32) -> u32 {
            if n == 0 {
                0
            } else {
                (self.next() % n as u64) as u32
            }
        }
        /// A register index: usually in `0..n_regs`, occasionally out of range so
        /// `validate`'s bounds checks are exercised.
        fn reg(&mut self, n_regs: u32) -> u32 {
            if self.next() & 7 == 0 {
                self.below(n_regs.saturating_mul(2).saturating_add(3))
            } else {
                self.below(n_regs.max(1))
            }
        }
        fn vty(&mut self) -> JitValueType {
            match self.below(5) {
                0 => JitValueType::Int,
                1 => JitValueType::Float,
                2 => JitValueType::FlatInt,
                3 => JitValueType::FlatFloat,
                _ => JitValueType::Handle,
            }
        }
    }

    /// One random instruction. `n` is the code length (for jump targets), which
    /// may be exceeded so out-of-range targets are tested too.
    fn random_instr(rng: &mut Rng, n_regs: u32, n: u32) -> JitInstr {
        let r = |rng: &mut Rng| rng.reg(n_regs);
        let t = |rng: &mut Rng| rng.below(n.saturating_add(2));
        match rng.below(31) {
            22 => JitInstr::HostCall {
                helper: HostHelper::FieldFloat,
                dst: r(rng),
                args: vec![
                    HostArg::Reg(r(rng)),
                    HostArg::ImmI64(i64::from(rng.below(8))),
                ],
            },
            23 => JitInstr::HostCall {
                helper: HostHelper::ListGetFloat,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng)), HostArg::Reg(r(rng))],
            },
            24 => JitInstr::ListGetIntDirect {
                dst: r(rng),
                base: r(rng),
                index: r(rng),
            },
            25 => JitInstr::ListGetFloatDirect {
                dst: r(rng),
                base: r(rng),
                index: r(rng),
            },
            26 => JitInstr::ListLenDirect {
                dst: r(rng),
                base: r(rng),
            },
            0 => JitInstr::Nop,
            1 => JitInstr::Bail,
            2 => JitInstr::LoadInt {
                dst: r(rng),
                value: rng.next() as i64,
            },
            3 => JitInstr::LoadFloat {
                dst: r(rng),
                value: f64::from_bits(rng.next()),
            },
            4 => JitInstr::LoadBool {
                dst: r(rng),
                value: rng.next() & 1 == 0,
            },
            5 => JitInstr::Move {
                dst: r(rng),
                src: r(rng),
            },
            6 => JitInstr::Add {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            7 => JitInstr::Sub {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            8 => JitInstr::Mul {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            9 => JitInstr::Div {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            10 => JitInstr::Mod {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            11 => JitInstr::BitAnd {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            12 => JitInstr::Shl {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            13 => JitInstr::Shr {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            14 => JitInstr::Equal {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            15 => JitInstr::Compare {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
                op: match rng.below(4) {
                    0 => JitCompare::Lt,
                    1 => JitCompare::Le,
                    2 => JitCompare::Gt,
                    _ => JitCompare::Ge,
                },
            },
            16 => JitInstr::Jump { target: t(rng) },
            17 => JitInstr::JumpIfBool {
                cond: r(rng),
                expected: rng.next() & 1 == 0,
                target: t(rng),
            },
            18 => JitInstr::Return { src: r(rng) },
            19 => JitInstr::HostCall {
                helper: HostHelper::FieldInt,
                dst: r(rng),
                args: vec![
                    HostArg::Reg(r(rng)),
                    HostArg::ImmI64(i64::from(rng.below(8))),
                ],
            },
            20 => JitInstr::HostCall {
                helper: HostHelper::ListLen,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng))],
            },
            21 => JitInstr::HostCall {
                helper: HostHelper::ListGetInt,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng)), HostArg::Reg(r(rng))],
            },
            27 => JitInstr::HostCall {
                helper: HostHelper::ClosureId,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng))],
            },
            28 => JitInstr::HostCall {
                helper: HostHelper::ClosureCapture,
                dst: r(rng),
                args: vec![
                    HostArg::Reg(r(rng)),
                    HostArg::ImmI64(i64::from(rng.below(8))),
                ],
            },
            29 => JitInstr::HostCall {
                helper: HostHelper::FieldHandle,
                dst: r(rng),
                args: vec![
                    HostArg::Reg(r(rng)),
                    HostArg::ImmI64(i64::from(rng.below(8))),
                ],
            },
            _ => JitInstr::HostCall {
                helper: HostHelper::ListGetHandle,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng)), HostArg::Reg(r(rng))],
            },
        }
    }

    fn random_program(rng: &mut Rng) -> JitFunction {
        let n_regs = rng.below(6); // 0..=5, includes the empty-window edge case
        let n_params = if n_regs == 0 {
            0
        } else {
            rng.below(n_regs + 1)
        };
        let len = rng.below(14);
        let reg_types = (0..n_regs).map(|_| rng.vty()).collect();
        let code = (0..len).map(|_| random_instr(rng, n_regs, len)).collect();
        JitFunction {
            n_params,
            n_regs,
            reg_types,
            code,
            cold_blocks: Vec::new(),
        }
    }

    #[test]
    fn fuzz_compile_is_total_over_arbitrary_ir() {
        let mut m = module();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..6000 {
            let prog = random_program(&mut rng);
            // The whole contract: never panic. Both arms are acceptable.
            match m.compile(&prog) {
                Ok(_) | Err(_) => {}
            }
        }
    }

    #[test]
    fn fuzz_compile_is_total_over_mutated_valid_ir() {
        // Seed: `fn(a, b) { t = a + b; return t }` — a known-valid program. Each
        // round perturbs one field (opcode swap, register bump, target bump,
        // truncation) and re-compiles; a mutation that invalidates the IR must be
        // caught as a clean error, not a panic.
        let mut m = module();
        let mut rng = Rng(0x1234_5678_9ABC_DEF0);
        let base = f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        for _ in 0..4000 {
            let mut prog = base.clone();
            match rng.below(5) {
                0 => prog.n_regs = rng.below(6),
                1 => prog.n_params = rng.below(6),
                2 => {
                    if !prog.code.is_empty() {
                        let idx = rng.below(prog.code.len() as u32) as usize;
                        prog.code[idx] =
                            random_instr(&mut rng, prog.n_regs.max(1), prog.code.len() as u32);
                    }
                }
                3 => prog
                    .code
                    .truncate(rng.below(prog.code.len() as u32 + 1) as usize),
                _ => {
                    if !prog.reg_types.is_empty() {
                        let idx = rng.below(prog.reg_types.len() as u32) as usize;
                        prog.reg_types[idx] = rng.vty();
                    }
                }
            }
            match m.compile(&prog) {
                Ok(_) | Err(_) => {}
            }
        }
    }

    /// Execution robustness + host-helper handle fuzz: drive *loop-free* (forward-
    /// jump-only, so guaranteed-terminating) validated programs through `call` with
    /// random argument bit patterns — including `Handle` args fed to the no-op
    /// `field_int`/`list_len`/`list_get_int` helpers at random slots/indices. The
    /// compiled code must always return cleanly (`Some`/`None` — a value or a bail),
    /// never UB or a hang. Loop-free generation is what keeps this from spinning on
    /// the native tier, which (by design, §6.2) has no internal step limit.
    #[test]
    fn fuzz_straightline_execution_never_traps_host() {
        let mut m = module();
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..3000 {
            let n_regs = rng.below(5).max(1);
            let n_params = rng.below(n_regs + 1);
            let len = rng.below(8);
            let reg_types: Vec<JitValueType> = (0..n_regs).map(|_| rng.vty()).collect();
            let mut code = Vec::new();
            for i in 0..len {
                // Forward-only jumps (target strictly after this index, up to `len`),
                // so control flow always makes progress to the end.
                let forward = i + 1 + rng.below(len.saturating_sub(i).max(1));
                let instr = match rng.below(12) {
                    0 => JitInstr::Jump { target: forward },
                    1 => JitInstr::JumpIfBool {
                        cond: rng.below(n_regs),
                        expected: rng.next() & 1 == 0,
                        target: forward,
                    },
                    other => random_instr(&mut rng, n_regs, len).pipe_nonjump(other),
                };
                code.push(instr);
            }
            // Guarantee a terminating tail so a validated function returns.
            code.push(JitInstr::Return {
                src: rng.below(n_regs),
            });
            let prog = JitFunction {
                n_params,
                n_regs,
                reg_types,
                code,
                cold_blocks: Vec::new(),
            };
            if let Ok(id) = m.compile(&prog) {
                let args: Vec<i64> = (0..n_params).map(|_| rng.next() as i64).collect();
                // Must return without UB/hang; value or bail are both fine.
                let _ = m.callt(id, &args);
            }
        }
    }

    // Small helper: keep a non-jump instruction as-is (jumps are generated with
    // forward targets separately above).
    impl JitInstr {
        fn pipe_nonjump(self, _tag: u32) -> JitInstr {
            match self {
                // Re-point any stray jump the generator produced to a Nop so this
                // path stays loop-free; all other instructions pass through.
                JitInstr::Jump { .. } | JitInstr::JumpIfBool { .. } => JitInstr::Nop,
                other => other,
            }
        }
    }

    // --- J4.3: conservative range proof for eliding overflow checks -----------

    /// LoadInt c ⇒ [c, c]; an Add of two known constants whose sum fits i64 is
    /// proven non-overflowing (and the proof line is computed in i128).
    #[test]
    fn interval_load_and_add_constants() {
        let prog = f(
            0,
            3,
            vec![
                JitInstr::LoadInt { dst: 0, value: 5 },
                JitInstr::LoadInt { dst: 1, value: 3 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        // On entry to the Add (ip 2), reg0=[5,5], reg1=[3,3].
        assert_eq!(iv[2][0], Interval { lo: 5, hi: 5 });
        assert_eq!(iv[2][1], Interval { lo: 3, hi: 3 });
        // The Add is proven non-overflowing ([8,8] ⊂ i64).
        assert!(arith_cannot_overflow(&iv[2], &prog.code[2]));
        // The result [8,8] flows to the Return's in-set (ip 3).
        assert_eq!(iv[3][2], Interval { lo: 8, hi: 8 });
    }

    /// A proven-unchecked add of two large constants whose sum is STILL within i64
    /// produces the exact (non-wrapping) sum — the unchecked op is byte-identical to
    /// the checked one here because the proof guarantees no overflow.
    #[test]
    fn proven_large_constant_add_is_correct() {
        let mut m = module();
        // c = (i64::MAX - 10) + 10 = i64::MAX, proven safe ⇒ unchecked, exact.
        let id = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt {
                        dst: 0,
                        value: i64::MAX - 10,
                    },
                    JitInstr::LoadInt { dst: 1, value: 10 },
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id, &[]), Some(i64::MAX));
    }

    /// Boundary: operand ranges summing to EXACTLY i64::MAX are proven safe; summing
    /// to i64::MAX + 1 are NOT (the analysis draws the line at the i64 boundary).
    #[test]
    fn boundary_exact_max_proven_overflow_unproven() {
        // Proven: (i64::MAX - 1) + 1 = i64::MAX, fits ⇒ unchecked.
        let safe = f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX - 1,
                },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&safe);
        assert_eq!(
            iv[2][0],
            Interval {
                lo: i64::MAX as i128 - 1,
                hi: i64::MAX as i128 - 1
            }
        );
        assert!(arith_cannot_overflow(&iv[2], &safe.code[2]));

        // Just over: i64::MAX + 1 overflows ⇒ NOT proven, stays checked.
        let over = f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX,
                },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv2 = interval_analysis(&over);
        assert!(!arith_cannot_overflow(&iv2[2], &over.code[2]));
    }

    /// The proven-boundary add runs unchecked and yields exactly i64::MAX (no bail);
    /// the over-boundary constant add — which the proof leaves CHECKED — bails on its
    /// actual overflow. Same analysis, opposite emission, both correct.
    #[test]
    fn boundary_proven_runs_overflow_constant_bails() {
        let mut m = module();
        let safe = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt {
                        dst: 0,
                        value: i64::MAX - 1,
                    },
                    JitInstr::LoadInt { dst: 1, value: 1 },
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(safe, &[]), Some(i64::MAX));

        let over = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt {
                        dst: 0,
                        value: i64::MAX,
                    },
                    JitInstr::LoadInt { dst: 1, value: 1 },
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        // Constant operands are tracked, [i64::MAX]+[1] doesn't fit ⇒ stays checked ⇒
        // the sadd_overflow guard fires on the real overflow ⇒ Deopt (None).
        assert_eq!(m.callt(over, &[]), None);
    }

    /// Params are untracked (TOP). `a + b` over params stays CHECKED and bails on a
    /// real overflow (i64::MAX + 1) exactly as before — proving checks are NOT
    /// over-eagerly stripped.
    #[test]
    fn unknown_params_stay_checked_and_bail() {
        let mut m = module();
        // fn(a, b) -> Int { return a + b }, params untracked ⇒ TOP ⇒ checked.
        let prog = f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        // Both operands TOP ⇒ result range is TOP ⇒ not proven.
        assert!(!arith_cannot_overflow(&iv[0], &prog.code[0]));
        let id = m.compile(&prog).unwrap();
        // In-range add returns the exact sum.
        assert_eq!(m.callt(id, &[2, 3]), Some(5));
        // i64::MAX + 1 overflows ⇒ the retained check bails.
        assert_eq!(m.callt(id, &[i64::MAX, 1]), None);
    }

    /// A register fed by `ListLen` is `[0, i64::MAX]` — non-negative but with NO
    /// tighter upper bound. So `len + len` can reach 2*i64::MAX, does NOT fit i64,
    /// and stays CHECKED (we did not assume a smaller length bound).
    #[test]
    fn list_len_is_nonneg_unbounded_above() {
        use JitValueType::{Handle, Int};
        let prog = ft(
            1,
            vec![Handle, Int, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::ListLen,
                    dst: 1,
                    args: vec![HostArg::Reg(0)],
                },
                // len + len
                JitInstr::Add {
                    dst: 2,
                    lhs: 1,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        // ListLen result is exactly [0, i64::MAX] on entry to the Add (ip 1).
        assert_eq!(
            iv[1][1],
            Interval {
                lo: 0,
                hi: i64::MAX as i128
            }
        );
        // [0,MAX]+[0,MAX] = [0, 2*MAX] does NOT fit i64 ⇒ stays checked.
        assert!(!arith_cannot_overflow(&iv[1], &prog.code[1]));
    }

    /// ListLenDirect is treated identically to ListLen: [0, i64::MAX].
    #[test]
    fn list_len_direct_is_nonneg() {
        use JitValueType::{FlatInt, Int};
        let prog = ft(
            1,
            vec![FlatInt, Int],
            vec![
                JitInstr::ListLenDirect { dst: 1, base: 0 },
                JitInstr::Return { src: 1 },
            ],
        );
        let iv = interval_analysis(&prog);
        assert_eq!(
            iv[1][1],
            Interval {
                lo: 0,
                hi: i64::MAX as i128
            }
        );
    }

    /// Sub and Mul interval transfer functions, plus their proven/unproven lines.
    #[test]
    fn interval_sub_and_mul_transfer() {
        // (10 - 3) = 7, proven; Move copies a range.
        let prog = f(
            0,
            4,
            vec![
                JitInstr::LoadInt { dst: 0, value: 10 },
                JitInstr::LoadInt { dst: 1, value: 3 },
                JitInstr::Sub {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Mul {
                    dst: 3,
                    lhs: 2,
                    rhs: 1,
                },
                JitInstr::Return { src: 3 },
            ],
        );
        let iv = interval_analysis(&prog);
        assert!(arith_cannot_overflow(&iv[2], &prog.code[2])); // [10,10]-[3,3]=[7,7]
        assert_eq!(iv[3][2], Interval { lo: 7, hi: 7 });
        assert!(arith_cannot_overflow(&iv[3], &prog.code[3])); // [7,7]*[3,3]=[21,21]

        // A Mul of two large constants whose product overflows is NOT proven.
        let big = f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX,
                },
                JitInstr::LoadInt { dst: 1, value: 2 },
                JitInstr::Mul {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv2 = interval_analysis(&big);
        assert!(!arith_cannot_overflow(&iv2[2], &big.code[2]));
    }

    /// A register incremented across a loop back-edge widens to TOP (we infer no loop
    /// bound), so an unbounded accumulator's add stays CHECKED — and the fixpoint
    /// terminates.
    #[test]
    fn loop_accumulator_widens_to_top() {
        // i = 0; loop { i = i + 1; } (no exit) — i grows unbounded.
        // regs: 0=i, 1=one
        let prog = f(
            0,
            2,
            vec![
                JitInstr::LoadInt { dst: 0, value: 0 }, // 0
                JitInstr::LoadInt { dst: 1, value: 1 }, // 1
                JitInstr::Add {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                }, // 2: i = i + 1
                JitInstr::Jump { target: 2 },           // 3: back-edge to the Add
            ],
        );
        let iv = interval_analysis(&prog);
        // After widening, `i` on entry to the Add is TOP ⇒ the increment stays checked.
        assert_eq!(iv[2][0], Interval::TOP);
        assert!(!arith_cannot_overflow(&iv[2], &prog.code[2]));
    }

    // --- J4.3+: branch-conditioned range refinement for loop counters ----------

    /// Build the counted loop
    ///   fn f(limit){ i=0; total=0; while i<limit { total += step; i += incr }; total }
    /// in JIT IR. regs: 0=limit(param), 1=i, 2=total, 3=incr(const), 4=step(const 1).
    /// The guard is `JumpIfIntCompare i < limit, expected=false, target=exit`, so the
    /// FALL-THROUGH edge into the body asserts `i < limit` (the refinement site).
    fn counted_loop(incr: i64, op: JitCompare) -> JitFunction {
        f(
            1,
            5,
            vec![
                JitInstr::LoadInt { dst: 1, value: 0 }, // 0: i = 0
                JitInstr::LoadInt { dst: 2, value: 0 }, // 1: total = 0
                JitInstr::LoadInt {
                    dst: 3,
                    value: incr,
                }, // 2: incr
                JitInstr::LoadInt { dst: 4, value: 1 }, // 3: step = 1
                // 4: header guard — if !(i <op> limit) goto exit(8)
                JitInstr::JumpIfIntCompare {
                    lhs: 1,
                    rhs: 0,
                    op,
                    expected: false,
                    target: 8,
                },
                JitInstr::Add {
                    dst: 2,
                    lhs: 2,
                    rhs: 4,
                }, // 5: total = total + 1 (unbounded)
                JitInstr::Add {
                    dst: 1,
                    lhs: 1,
                    rhs: 3,
                }, // 6: i = i + incr (loop counter)
                JitInstr::Jump { target: 4 }, // 7: back-edge
                JitInstr::Return { src: 2 },  // 8: exit
            ],
        )
    }

    /// (i) Under the guard `i < limit`, the counter increment `i = i + 1` is proven
    /// UNCHECKED: on the loop-body edge `i <= limit - 1 <= i64::MAX - 1`, so
    /// `i + 1 <= i64::MAX` provably fits. The unbounded accumulator `total = total + 1`
    /// stays CHECKED (total widens to TOP — no bounding guard).
    #[test]
    fn loop_counter_lt_increment_proven_accumulator_checked() {
        let prog = counted_loop(1, JitCompare::Lt);
        let iv = interval_analysis(&prog);
        // Body in-set (ip 5/6): the refined `i` is bounded above by limit.hi - 1.
        // limit is TOP ([MIN, MAX]) ⇒ i.hi = MAX - 1.
        assert_eq!(iv[6][1].hi, i64::MAX as i128 - 1);
        // i = i + 1 (ip 6): [.., MAX-1] + [1,1] = [.., MAX] ⇒ fits ⇒ UNCHECKED.
        assert!(arith_cannot_overflow(&iv[6], &prog.code[6]));
        // total = total + 1 (ip 5): total is TOP ⇒ stays CHECKED.
        assert!(!arith_cannot_overflow(&iv[5], &prog.code[5]));
    }

    /// The same loop, compiled and run: the result equals the loop trip count for a
    /// large limit, confirming the UNCHECKED counter increment is correct at scale
    /// (no spurious bail, no wrong wrap). total stays small so its checked add is fine.
    #[test]
    fn loop_counter_lt_runs_correct_at_scale() {
        let mut m = module();
        let id = m.compile(&counted_loop(1, JitCompare::Lt)).unwrap();
        assert_eq!(m.callt(id, &[0]), Some(0));
        assert_eq!(m.callt(id, &[1]), Some(1));
        assert_eq!(m.callt(id, &[1_000_000]), Some(1_000_000));
    }

    /// (ii-a) `i = i + 2` must stay CHECKED: under `i < limit` we only know
    /// `i <= i64::MAX - 1`, so `i + 2` can reach `i64::MAX + 1` ⇒ does NOT fit.
    #[test]
    fn loop_counter_plus_two_stays_checked() {
        let prog = counted_loop(2, JitCompare::Lt);
        let iv = interval_analysis(&prog);
        // i still refined to [.., MAX-1], but [.., MAX-1] + [2,2] = [.., MAX+1] ⇒
        // does NOT fit i64 ⇒ CHECKED.
        assert_eq!(iv[6][1].hi, i64::MAX as i128 - 1);
        assert!(!arith_cannot_overflow(&iv[6], &prog.code[6]));
    }

    /// (ii-b) `while i <= limit` must keep `i = i + 1` CHECKED: the `Le` taken-edge
    /// only proves `i <= limit <= i64::MAX`, so `i` may BE `i64::MAX` and `i + 1`
    /// overflows. This locks the Lt-vs-Le off-by-one.
    #[test]
    fn loop_counter_le_increment_stays_checked() {
        let prog = counted_loop(1, JitCompare::Le);
        let iv = interval_analysis(&prog);
        // Under `i <= limit` (limit TOP), i.hi = min(MAX, limit.hi) = MAX, NOT MAX-1.
        assert_eq!(iv[6][1].hi, i64::MAX as i128);
        // [.., MAX] + [1,1] = [.., MAX+1] ⇒ does NOT fit ⇒ CHECKED.
        assert!(!arith_cannot_overflow(&iv[6], &prog.code[6]));
    }

    /// (iii) A bare `a + b` with unconstrained (TOP) operands and no governing guard
    /// stays CHECKED and bails on a real overflow — refinement never strips a check
    /// when there is no comparison fact to refine by. (Mirrors the param test, kept
    /// here so the J4.3+ slice asserts the negative directly.)
    #[test]
    fn unguarded_add_stays_checked_and_bails() {
        let mut m = module();
        let prog = f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        assert!(!arith_cannot_overflow(&iv[0], &prog.code[0]));
        let id = m.compile(&prog).unwrap();
        assert_eq!(m.callt(id, &[i64::MAX, 1]), None); // checked overflow ⇒ bail
    }

    /// The refinement is edge-SENSITIVE: the SAME register is bounded on the taken
    /// edge but TOP at the post-join loop header. A direct two-block check that the
    /// guard's taken (`<`) edge tightens lhs while the header stays TOP, and that an
    /// unreachable refinement (`x < x`) is handled soundly (no malformed interval).
    #[test]
    fn refinement_edge_sensitive_and_unreachable_safe() {
        // if i < limit { return i + 1 } else { return i }   regs 0=limit,1=i,2=t
        let prog = f(
            2,
            3,
            vec![
                // 0: if !(i < limit) goto 3 (else-branch)
                JitInstr::JumpIfIntCompare {
                    lhs: 1,
                    rhs: 0,
                    op: JitCompare::Lt,
                    expected: false,
                    target: 3,
                },
                JitInstr::Add {
                    dst: 2,
                    lhs: 1,
                    rhs: 1,
                }, // 1: t = i + i (on i<limit edge)
                JitInstr::Return { src: 2 }, // 2
                JitInstr::Return { src: 1 }, // 3: else
            ],
        );
        let iv = interval_analysis(&prog);
        // On the body edge i is refined to [MIN, limit.hi-1] = [MIN, MAX-1]; the
        // header in-set (ip 0) still sees i as TOP (param, untracked).
        assert_eq!(iv[0][1], Interval::TOP);
        assert_eq!(iv[1][1].hi, i64::MAX as i128 - 1);

        // Unreachable edge: `if i < i` — the taken edge asserts the empty fact i < i.
        // The two per-operand narrowings (i.hi <= i.hi-1, i.lo >= i.lo+1) each apply
        // to the SAME register but never invert it in a single `apply`, so the result
        // is a sound (over-approximating) but WELL-FORMED interval. The contract this
        // test locks is soundness's structural half: NO malformed interval (lo > hi)
        // ever escapes the refinement, even on an unreachable edge.
        let bad = f(
            1,
            2,
            vec![
                JitInstr::JumpIfIntCompare {
                    lhs: 0,
                    rhs: 0,
                    op: JitCompare::Lt,
                    expected: true,
                    target: 2,
                },
                JitInstr::Return { src: 0 }, // 1: fall-through (i >= i, always)
                JitInstr::Return { src: 0 }, // 2: taken (i < i, unreachable)
            ],
        );
        let iv2 = interval_analysis(&bad);
        // Every register interval at every ip is well-formed (lo <= hi).
        for row in &iv2 {
            for v in row {
                assert!(v.lo <= v.hi, "malformed interval {v:?}");
            }
        }
    }
}
