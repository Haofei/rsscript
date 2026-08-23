/// Opaque VM-owned native helper context. The JIT never interprets this value; it
/// just forwards it from [`NativeModule::call`] to every imported host helper.
pub type HostCtx = i64;

pub const JIT_CALL_ABI_VERSION: u32 = 3;

/// Single versioned argument passed to generated functions. Keeping the machine
/// ABI to one pointer prevents caller/codegen parameter-order drift as execution
/// controls evolve.
#[repr(C)]
pub struct JitCallFrame {
    pub abi_version: u32,
    /// Size of the caller-provided frame. Generated code validates this prefix
    /// before loading any pointer-bearing field.
    pub frame_size: u32,
    pub flags: u64,
    pub args: *const i64,
    pub lens: *const i64,
    pub arg_count: usize,
    pub host_ctx: HostCtx,
    pub limits: *const i64,
    pub result: *mut i64,
    pub bail: *mut u8,
    pub safepoint: *mut i64,
    pub deopt: *mut i64,
    pub native_depth: usize,
    pub logical_depth: usize,
    pub logical_depth_limit: usize,
}

pub(crate) const CALL_FRAME_SIZE: u32 = std::mem::size_of::<JitCallFrame>() as u32;
pub(crate) const FRAME_ABI_VERSION: i32 = std::mem::offset_of!(JitCallFrame, abi_version) as i32;
pub(crate) const FRAME_SIZE: i32 = std::mem::offset_of!(JitCallFrame, frame_size) as i32;
pub(crate) const FRAME_FLAGS: i32 = std::mem::offset_of!(JitCallFrame, flags) as i32;
pub(crate) const FRAME_ARGS: i32 = std::mem::offset_of!(JitCallFrame, args) as i32;
pub(crate) const FRAME_LENS: i32 = std::mem::offset_of!(JitCallFrame, lens) as i32;
pub(crate) const FRAME_ARG_COUNT: i32 = std::mem::offset_of!(JitCallFrame, arg_count) as i32;
pub(crate) const FRAME_HOST_CTX: i32 = std::mem::offset_of!(JitCallFrame, host_ctx) as i32;
pub(crate) const FRAME_LIMITS: i32 = std::mem::offset_of!(JitCallFrame, limits) as i32;
pub(crate) const FRAME_RESULT: i32 = std::mem::offset_of!(JitCallFrame, result) as i32;
pub(crate) const FRAME_BAIL: i32 = std::mem::offset_of!(JitCallFrame, bail) as i32;
pub(crate) const FRAME_SAFEPOINT: i32 = std::mem::offset_of!(JitCallFrame, safepoint) as i32;
pub(crate) const FRAME_DEOPT: i32 = std::mem::offset_of!(JitCallFrame, deopt) as i32;
pub(crate) const FRAME_NATIVE_DEPTH: i32 = std::mem::offset_of!(JitCallFrame, native_depth) as i32;
pub(crate) const FRAME_LOGICAL_DEPTH: i32 =
    std::mem::offset_of!(JitCallFrame, logical_depth) as i32;
pub(crate) const FRAME_LOGICAL_DEPTH_LIMIT: i32 =
    std::mem::offset_of!(JitCallFrame, logical_depth_limit) as i32;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitStatus {
    // Generated code returns this discriminant directly. Rust code never needs to
    // construct the variant, but it must remain in the FFI enum so decoding `0`
    // is well-defined.
    #[allow(dead_code)]
    Deopt = 0,
    Completed = 1,
    AbiMismatch = 2,
    Yielded = 3,
}

pub(crate) const DEFAULT_STANDALONE_JIT_ARENA_BYTES: u64 = 64 * 1024 * 1024;

/// Borrow proof for one flat buffer passed to generated code. Immutable proofs may
/// be reused; a mutable proof validates exactly one mutable ABI entry and cannot
/// also satisfy a read-only entry. Safe callers cannot construct an arbitrary
/// address: `NativeModule` validates every pointer and length immediately before
/// the call.
pub enum FlatBufferArg<'a> {
    Int(&'a [i64]),
    IntMut(&'a mut [i64]),
    Float(&'a [f64]),
    FloatMut(&'a mut [f64]),
}

/// `(struct_handle, slot) -> i64`: the struct's `slot`-th field as an `Int`.
pub type FieldIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(struct_handle, slot, value) -> i64`: copy-on-write set of an `Int` field,
/// returning a VM-owned output-table handle for the updated struct/variant.
pub type FieldSetIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(struct_handle, slot, value_handle) -> i64`: set a struct/variant field to a **heap**
/// value (e.g. a `String`/nested collection), returning the new (COW) struct's
/// output-table handle. The value handle is resolved to its heap value; a wrong-type/
/// out-of-range field signals a bail out-of-band.
pub type FieldSetHandleFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(struct_handle, slot, value: f64) -> i64`: copy-on-write set of a `Float`
/// field (the write-side counterpart of [`FieldFloatFn`]), returning a VM-owned
/// output-table handle for the updated struct/variant. A wrong-type/out-of-range
/// field signals a bail out-of-band.
pub type FieldSetFloatFn = extern "C" fn(HostCtx, i64, i64, f64) -> i64;
/// `(list_handle) -> i64`: list length.
pub type ListLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(handle) -> i64`: return `1` when a collection is empty, else `0`.
pub type IsEmptyFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(list_handle, index) -> i64`: the list element at `index` as an `Int`.
pub type ListGetIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(list_handle, index, value) -> i64`: set an `Int` list element. Returns `0`
/// on success; a wrong-type/out-of-bounds write signals a bail out-of-band.
pub type ListSetIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(list_handle, index, value: f64) -> i64`: set a `Float` list element (write-side
/// counterpart of [`ListGetFloatFn`]). Returns `0` on success; a wrong-type/out-of-
/// bounds write signals a bail out-of-band.
pub type ListSetFloatFn = extern "C" fn(HostCtx, i64, i64, f64) -> i64;
/// `(list_handle, value) -> i64`: push an `Int` list element. Returns `0` on
/// success; a wrong-type/invalid handle signals a bail out-of-band.
pub type ListPushIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(list_handle, value_handle: i64) -> i64`: push a **heap** element (e.g. a `String`
/// or nested collection) onto a `List<HeapType>`. The value handle is resolved to its
/// heap value (host-owned) and appended; the write is journaled (the transaction rollback contract). A
/// wrong-type/invalid handle signals a bail out-of-band.
pub type ListPushHandleFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(list_handle, value: f64) -> i64`: push a `Float` list element (the write-side
/// counterpart of [`ListGetFloatFn`]). Returns `0` on success; a wrong-type/invalid
/// handle signals a bail out-of-band.
pub type ListPushFloatFn = extern "C" fn(HostCtx, i64, f64) -> i64;
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
/// `(map_handle, key_handle, value: i64) -> i64`: insert an `Int` value under a
/// **heap key** (e.g. a `String`) into a `Map<HeapKey, Int>`. The key handle is
/// resolved to its heap value and hashed by the host's own canonical map-key (never
/// re-implemented in native). A wrong container/key shape signals a bail.
pub type MapInsertHandleKeyIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(map_handle, key, value: f64) -> i64`: insert into an Int-keyed `Map<_, Float>`.
pub type MapInsertFloatFn = extern "C" fn(HostCtx, i64, i64, f64) -> i64;
/// `(map_handle, key) -> i64`: return an Int payload for an existing map key.
/// Missing keys or non-Int payloads signal a bail.
pub type MapGetIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(map_handle, key, found_out) -> i64`: return an Int payload for a map key, or
/// 0 for a missing key, and write 1/0 to `found_out` in the same host call. Wrong
/// shape/non-Int payloads signal a bail.
pub type MapGetMatchIntFn = extern "C" fn(HostCtx, i64, i64, &mut i64) -> i64;
/// `(map_handle, key, found_out) -> f64`: Float mirror of [`MapGetMatchIntFn`].
pub type MapGetMatchFloatFn = extern "C" fn(HostCtx, i64, i64, &mut i64) -> f64;
/// `(map_handle, key) -> i64`: return `1` when the Int key exists, else `0`.
/// A wrong container/key shape signals a bail.
pub type MapContainsIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(handle) -> i64`: collection length. A wrong container shape signals a bail.
pub type CollectionLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(set_handle, value) -> i64`: insert an Int into a set, returning whether it
/// was newly inserted. A wrong container/value shape signals a bail.
pub type SetInsertIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(set_handle, value_handle) -> i64`: insert a **heap** value (e.g. a `String`) into a
/// `Set<HeapType>`, returning whether it was newly inserted. The value handle is
/// resolved and hashed by the host's own canonical key (never re-hashed in native). A
/// wrong container/value shape signals a bail.
pub type SetInsertHandleFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(set_handle, value) -> i64`: insert an Int into a sorted set, returning whether
/// it was newly inserted. A wrong container/value shape signals a bail.
pub type SortedSetInsertIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(set_handle, value_handle) -> i64`: insert a **heap** value (e.g. `String`) into a
/// sorted set, returning whether newly inserted. Ordering/equality is the host's own.
pub type SortedSetInsertHandleFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(set_handle, value) -> i64`: return whether an Int exists in a sorted set.
/// A wrong container/value shape signals a bail.
pub type SortedSetContainsIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(map_handle, key, value) -> i64`: insert/update an Int-keyed, Int-valued
/// sorted map. A wrong container/key/value shape signals a bail.
pub type SortedMapInsertIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(map_handle, key_handle, value: i64) -> i64`: insert an `Int` value under a **heap**
/// key (e.g. `String`) into a sorted map. Ordering/equality is the host's own.
pub type SortedMapInsertHandleKeyIntFn = extern "C" fn(HostCtx, i64, i64, i64) -> i64;
/// `(map_handle, key, found_out) -> i64`: return an Int payload for an existing
/// sorted-map key, or 0 for a missing key, and write 1/0 to `found_out`.
pub type SortedMapGetIntFn = extern "C" fn(HostCtx, i64, i64, &mut i64) -> i64;
/// `(map_handle, key, found_out) -> f64`: Float mirror of [`SortedMapGetIntFn`].
pub type SortedMapGetFloatFn = extern "C" fn(HostCtx, i64, i64, &mut i64) -> f64;
/// `(map_handle, key) -> i64`: return `1` when the Int key exists, else `0`.
/// A wrong container/key shape signals a bail.
pub type SortedMapContainsKeyIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(map_handle) -> i64`: sorted map length. A wrong container shape signals a bail.
pub type SortedMapLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(deque_handle) -> i64`: deque length. A wrong container shape signals a bail.
pub type DequeLenFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(deque_handle, value) -> i64`: push an Int to the back of a deque.
pub type DequePushBackIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(deque_handle, value_handle) -> i64`: push a **heap** value onto the back of a
/// `Deque<HeapType>`. The value handle is resolved to its heap value. Bails on bad shape.
pub type DequePushBackHandleFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(deque_handle, value: f64) -> i64`: push a `Float` onto the back of a `Deque<Float>`.
pub type DequePushBackFloatFn = extern "C" fn(HostCtx, i64, f64) -> i64;
/// `(deque_handle, value: f64) -> i64`: push a `Float` onto the front of a `Deque<Float>`.
pub type DequePushFrontFloatFn = extern "C" fn(HostCtx, i64, f64) -> i64;
/// `(deque_handle, value) -> i64`: push an Int to the front of a deque.
pub type DequePushFrontIntFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(deque_handle, value_handle) -> i64`: push a **heap** value onto the front of a
/// `Deque<HeapType>`. The value handle is resolved to its heap value. Bails on bad shape.
pub type DequePushFrontHandleFn = extern "C" fn(HostCtx, i64, i64) -> i64;
/// `(deque_handle) -> i64`: pop an Int from the front of a deque. Empty or non-Int
/// payloads signal a bail; RSScript's interpreter then executes the `None` path.
pub type DequePopFrontIntFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(deque_handle) -> i64`: pop an Int from the back of a deque. Empty or non-Int
/// payloads signal a bail; RSScript's interpreter then executes the `None` path.
pub type DequePopBackIntFn = extern "C" fn(HostCtx, i64) -> i64;
/// `(deque_handle) -> f64`: pop a `Float` from the front of a `Deque<Float>` (Float
/// value-side mirror of [`DequePopFrontIntFn`]). Empty or non-Float payloads signal a
/// bail; the interpreter then executes the `None` path.
pub type DequePopFrontFloatFn = extern "C" fn(HostCtx, i64) -> f64;
/// `(deque_handle) -> f64`: pop a `Float` from the back of a `Deque<Float>`.
pub type DequePopBackFloatFn = extern "C" fn(HostCtx, i64) -> f64;

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
    pub field_set_handle: FieldSetHandleFn,
    pub field_set_float: FieldSetFloatFn,
    pub list_len: ListLenFn,
    pub list_is_empty: IsEmptyFn,
    pub list_get_int: ListGetIntFn,
    pub list_set_int: ListSetIntFn,
    pub list_set_float: ListSetFloatFn,
    pub list_push_int: ListPushIntFn,
    pub list_push_handle: ListPushHandleFn,
    pub list_push_float: ListPushFloatFn,
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
    pub map_insert_handle_key_int: MapInsertHandleKeyIntFn,
    pub map_insert_float: MapInsertFloatFn,
    pub map_get_int: MapGetIntFn,
    pub map_get_match_int: MapGetMatchIntFn,
    pub map_get_match_float: MapGetMatchFloatFn,
    pub map_contains_int: MapContainsIntFn,
    pub map_len: CollectionLenFn,
    pub map_is_empty: IsEmptyFn,
    pub set_insert_int: SetInsertIntFn,
    pub set_insert_handle: SetInsertHandleFn,
    pub set_len: CollectionLenFn,
    pub set_is_empty: IsEmptyFn,
    pub sorted_set_insert_int: SortedSetInsertIntFn,
    pub sorted_set_insert_handle: SortedSetInsertHandleFn,
    pub sorted_set_contains_int: SortedSetContainsIntFn,
    pub sorted_set_is_empty: IsEmptyFn,
    pub sorted_map_insert_int: SortedMapInsertIntFn,
    pub sorted_map_insert_handle_key_int: SortedMapInsertHandleKeyIntFn,
    pub sorted_map_get_int: SortedMapGetIntFn,
    pub sorted_map_get_float: SortedMapGetFloatFn,
    pub sorted_map_contains_key_int: SortedMapContainsKeyIntFn,
    pub sorted_map_is_empty: IsEmptyFn,
    pub sorted_map_len: SortedMapLenFn,
    pub deque_len: DequeLenFn,
    pub deque_is_empty: IsEmptyFn,
    pub deque_push_back_int: DequePushBackIntFn,
    pub deque_push_back_handle: DequePushBackHandleFn,
    pub deque_push_back_float: DequePushBackFloatFn,
    pub deque_push_front_int: DequePushFrontIntFn,
    pub deque_push_front_handle: DequePushFrontHandleFn,
    pub deque_push_front_float: DequePushFrontFloatFn,
    pub deque_pop_front_int: DequePopFrontIntFn,
    pub deque_pop_back_int: DequePopBackIntFn,
    pub deque_pop_front_float: DequePopFrontFloatFn,
    pub deque_pop_back_float: DequePopBackFloatFn,
}

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

/// Heap substructure observed or modified by a host helper. Native optimization
/// passes use these projections to distinguish shape-preserving element writes
/// from operations that can change collection metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostHeapProjection {
    CollectionLen,
    Elements,
    KeySet,
    Fields,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostHeapAccess {
    pub arg: u8,
    pub projection: HostHeapProjection,
}

impl HostHeapAccess {
    pub const fn new(arg: u8, projection: HostHeapProjection) -> Self {
        Self { arg, projection }
    }
}

const HOST_READ_COLLECTION_LEN: [HostHeapAccess; 1] =
    [HostHeapAccess::new(0, HostHeapProjection::CollectionLen)];
const HOST_WRITE_ELEMENTS: [HostHeapAccess; 1] =
    [HostHeapAccess::new(0, HostHeapProjection::Elements)];
const HOST_WRITE_COLLECTION_SHAPE: [HostHeapAccess; 2] = [
    HostHeapAccess::new(0, HostHeapProjection::CollectionLen),
    HostHeapAccess::new(0, HostHeapProjection::Elements),
];
const HOST_WRITE_MAP_SHAPE: [HostHeapAccess; 3] = [
    HostHeapAccess::new(0, HostHeapProjection::CollectionLen),
    HostHeapAccess::new(0, HostHeapProjection::KeySet),
    HostHeapAccess::new(0, HostHeapProjection::Elements),
];
const HOST_WRITE_SET_SHAPE: [HostHeapAccess; 2] = [
    HostHeapAccess::new(0, HostHeapProjection::CollectionLen),
    HostHeapAccess::new(0, HostHeapProjection::KeySet),
];
const HOST_WRITE_FIELDS: [HostHeapAccess; 1] = [HostHeapAccess::new(0, HostHeapProjection::Fields)];
const HOST_WRITE_UNKNOWN: [HostHeapAccess; 1] =
    [HostHeapAccess::new(0, HostHeapProjection::Unknown)];

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
pub(crate) struct HostHelperSig {
    pub(crate) args: &'static [JitValueType],
    pub(crate) found_out: bool,
    pub(crate) result: HostResult,
    pub(crate) failure: HostFailureMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostResult {
    Exact(JitValueType),
    IntOrFloatBits,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct HostHelperDescriptor {
    pub(crate) helper: HostHelper,
    pub(crate) symbol: &'static str,
    pub(crate) sig: HostHelperSig,
    pub(crate) heap_effect: HostHeapEffect,
    pub(crate) heap_reads: &'static [HostHeapAccess],
    pub(crate) heap_writes: &'static [HostHeapAccess],
}

macro_rules! host_heap_effect {
    () => {
        HostHeapEffect::ReadOnly
    };
    ($effect:expr) => {
        $effect
    };
}

macro_rules! host_found_out {
    () => {
        false
    };
    ($found_out:expr) => {
        $found_out
    };
}

macro_rules! host_heap_accesses {
    () => {
        &[]
    };
    ($accesses:expr) => {
        $accesses
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
            $(found_out: $found_out:expr,)?
            $(heap_effect: $heap_effect:expr,)?
            $(reads: $reads:expr,)?
            $(writes: $writes:expr,)?
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
            pub(crate) fn addr(self, helper: HostHelper) -> *const u8 {
                match helper {
                    $(HostHelper::$helper => self.$field as *const u8),+
                }
            }
        }

        impl HostHelper {
            const ALL: &'static [HostHelper] = &[
                $(HostHelper::$helper),+
            ];

            pub(crate) const DESCRIPTORS: &'static [HostHelperDescriptor] = &[
            $(HostHelperDescriptor {
                    helper: HostHelper::$helper,
                    symbol: $symbol,
                    sig: HostHelperSig {
                        args: &[$($arg),*],
                        found_out: host_found_out!($($found_out)?),
                        result: $result,
                        failure: $failure,
                    },
                    heap_effect: host_heap_effect!($($heap_effect)?),
                    heap_reads: host_heap_accesses!($($reads)?),
                    heap_writes: host_heap_accesses!($($writes)?),
                }),+
            ];

            pub(crate) fn all() -> &'static [HostHelper] {
                Self::ALL
            }

            fn descriptor(self) -> &'static HostHelperDescriptor {
                Self::DESCRIPTORS
                    .iter()
                    .find(|descriptor| descriptor.helper == self)
                    .expect("host helper descriptor missing")
            }

            pub(crate) fn symbol(self) -> &'static str {
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

            pub fn heap_reads(self) -> &'static [HostHeapAccess] {
                self.descriptor().heap_reads
            }

            pub fn heap_writes(self) -> &'static [HostHeapAccess] {
                let descriptor = self.descriptor();
                if descriptor.heap_writes.is_empty()
                    && descriptor.heap_effect.writes_existing_heap()
                {
                    &HOST_WRITE_UNKNOWN
                } else {
                    descriptor.heap_writes
                }
            }

            pub(crate) fn signature(self) -> HostHelperSig {
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
        writes: &HOST_WRITE_FIELDS,
    },
    FieldSetHandle => {
        field: field_set_handle,
        symbol: "rss_jit_field_set_handle",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::ReplacesInput,
        writes: &HOST_WRITE_FIELDS,
    },
    FieldSetFloat => {
        field: field_set_float,
        symbol: "rss_jit_field_set_float",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Float],
        result: HostResult::Exact(JitValueType::Handle),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::ReplacesInput,
        writes: &HOST_WRITE_FIELDS,
    },
    ListLen => {
        field: list_len,
        symbol: "rss_jit_list_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    ListIsEmpty => {
        field: list_is_empty,
        symbol: "rss_jit_list_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
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
        writes: &HOST_WRITE_ELEMENTS,
    },
    ListSetFloat => {
        field: list_set_float,
        symbol: "rss_jit_list_set_float",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Float],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_ELEMENTS,
    },
    ListPushInt => {
        field: list_push_int,
        symbol: "rss_jit_list_push_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    ListPushHandle => {
        field: list_push_handle,
        symbol: "rss_jit_list_push_handle",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    ListPushFloat => {
        field: list_push_float,
        symbol: "rss_jit_list_push_float",
        args: [JitValueType::Handle, JitValueType::Float],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    ListSortInt => {
        field: list_sort_int,
        symbol: "rss_jit_list_sort_int",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_ELEMENTS,
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
        result: HostResult::Exact(JitValueType::Bool),
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
        writes: &HOST_WRITE_MAP_SHAPE,
    },
    MapInsertHandleKeyInt => {
        field: map_insert_handle_key_int,
        symbol: "rss_jit_map_insert_handle_key_int",
        args: [JitValueType::Handle, JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_MAP_SHAPE,
    },
    MapInsertFloat => {
        field: map_insert_float,
        symbol: "rss_jit_map_insert_float",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Float],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_MAP_SHAPE,
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
        found_out: true,
    },
    MapGetMatchFloat => {
        field: map_get_match_float,
        symbol: "rss_jit_map_get_match_float",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Float),
        failure: HostFailureMode::BailFlag,
        found_out: true,
    },
    MapContainsInt => {
        field: map_contains_int,
        symbol: "rss_jit_map_contains_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
    },
    MapLen => {
        field: map_len,
        symbol: "rss_jit_map_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    MapIsEmpty => {
        field: map_is_empty,
        symbol: "rss_jit_map_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    SetInsertInt => {
        field: set_insert_int,
        symbol: "rss_jit_set_insert_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_SET_SHAPE,
    },
    SetInsertHandle => {
        field: set_insert_handle,
        symbol: "rss_jit_set_insert_handle",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_SET_SHAPE,
    },
    SetLen => {
        field: set_len,
        symbol: "rss_jit_set_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    SetIsEmpty => {
        field: set_is_empty,
        symbol: "rss_jit_set_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    SortedSetInsertInt => {
        field: sorted_set_insert_int,
        symbol: "rss_jit_sorted_set_insert_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_SET_SHAPE,
    },
    SortedSetInsertHandle => {
        field: sorted_set_insert_handle,
        symbol: "rss_jit_sorted_set_insert_handle",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_SET_SHAPE,
    },
    SortedSetContainsInt => {
        field: sorted_set_contains_int,
        symbol: "rss_jit_sorted_set_contains_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
    },
    SortedSetIsEmpty => {
        field: sorted_set_is_empty,
        symbol: "rss_jit_sorted_set_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    SortedMapInsertInt => {
        field: sorted_map_insert_int,
        symbol: "rss_jit_sorted_map_insert_int",
        args: [JitValueType::Handle, JitValueType::Int, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_MAP_SHAPE,
    },
    SortedMapInsertHandleKeyInt => {
        field: sorted_map_insert_handle_key_int,
        symbol: "rss_jit_sorted_map_insert_handle_key_int",
        args: [JitValueType::Handle, JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_MAP_SHAPE,
    },
    SortedMapGetInt => {
        field: sorted_map_get_int,
        symbol: "rss_jit_sorted_map_get_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        found_out: true,
    },
    SortedMapGetFloat => {
        field: sorted_map_get_float,
        symbol: "rss_jit_sorted_map_get_float",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Float),
        failure: HostFailureMode::BailFlag,
        found_out: true,
    },
    SortedMapContainsKeyInt => {
        field: sorted_map_contains_key_int,
        symbol: "rss_jit_sorted_map_contains_key_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
    },
    SortedMapIsEmpty => {
        field: sorted_map_is_empty,
        symbol: "rss_jit_sorted_map_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    SortedMapLen => {
        field: sorted_map_len,
        symbol: "rss_jit_sorted_map_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    DequeLen => {
        field: deque_len,
        symbol: "rss_jit_deque_len",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    DequeIsEmpty => {
        field: deque_is_empty,
        symbol: "rss_jit_deque_is_empty",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Bool),
        failure: HostFailureMode::BailFlag,
        reads: &HOST_READ_COLLECTION_LEN,
    },
    DequePushBackInt => {
        field: deque_push_back_int,
        symbol: "rss_jit_deque_push_back_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePushBackHandle => {
        field: deque_push_back_handle,
        symbol: "rss_jit_deque_push_back_handle",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePushBackFloat => {
        field: deque_push_back_float,
        symbol: "rss_jit_deque_push_back_float",
        args: [JitValueType::Handle, JitValueType::Float],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePushFrontInt => {
        field: deque_push_front_int,
        symbol: "rss_jit_deque_push_front_int",
        args: [JitValueType::Handle, JitValueType::Int],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePushFrontHandle => {
        field: deque_push_front_handle,
        symbol: "rss_jit_deque_push_front_handle",
        args: [JitValueType::Handle, JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePushFrontFloat => {
        field: deque_push_front_float,
        symbol: "rss_jit_deque_push_front_float",
        args: [JitValueType::Handle, JitValueType::Float],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePopFrontInt => {
        field: deque_pop_front_int,
        symbol: "rss_jit_deque_pop_front_int",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePopFrontFloat => {
        field: deque_pop_front_float,
        symbol: "rss_jit_deque_pop_front_float",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Float),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePopBackFloat => {
        field: deque_pop_back_float,
        symbol: "rss_jit_deque_pop_back_float",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Float),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
    DequePopBackInt => {
        field: deque_pop_back_int,
        symbol: "rss_jit_deque_pop_back_int",
        args: [JitValueType::Handle],
        result: HostResult::Exact(JitValueType::Int),
        failure: HostFailureMode::BailFlag,
        heap_effect: HostHeapEffect::MutatesInput,
        writes: &HOST_WRITE_COLLECTION_SHAPE,
    },
}
use super::*;
