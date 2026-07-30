use super::super::*;

/// Process-once gate for compile-time `DeepCopy` elision (`RSS_VM_ELIDE_DEEPCOPY`).
///
/// Default ON (Phase 2 v2): the lowerer neutralizes the prologue `DeepCopy` of any non-`mut`
/// heap parameter it can PROVE is never mutated through an alias and never escapes the frame —
/// sharing the caller's `Rc` is then observationally identical to copying it (the analysis
/// keeps the copy for everything not proven safe; native sees an unchanged `DeepCopyElided`
/// marker). Verified parity-preserving over runtime 455/0 + differential 33/0 @ 2000 generative
/// cases + soak + cost-model, with a ~14x win on the deep-copy-heavy kernel. Set
/// `RSS_VM_ELIDE_DEEPCOPY=0` (or `off`/`false`/`no`) to restore the byte-identical eager-copy
/// lowering for a fast rollback. Read once (like `RSS_JIT_COST_MODEL`) so the verdict is stable
/// across every function lowering in the process.
pub(super) fn elide_deepcopy_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("RSS_VM_ELIDE_DEEPCOPY")
                .ok()
                .as_deref()
                .map(str::trim),
            Some("0") | Some("false") | Some("off") | Some("no")
        )
    })
}

/// Test-only view of the process-once elision gate, so the elision regression guard
/// (`deepcopy_elision_fires_for_read_only_heap_param`) can branch on the exact verdict
/// the lowerer used rather than re-reading the env (and matching the memoized value).
#[cfg(test)]
pub(crate) fn elide_deepcopy_enabled_for_test() -> bool {
    elide_deepcopy_enabled()
}

/// The register mutated IN PLACE by an in-scope container mutator, if any — a NON-`native-jit`
/// mirror of `native::passes::native_heap_mutation_receiver` (which is `cfg`-gated). Mutating
/// one of these on a tainted receiver would write through the shared `Rc` the interpreter would
/// have left untouched, so it forces the copy to be kept. (The broader set of interpreter-only
/// mutators — `MapRemove`, `SetClear`, `ListSortBy`, … — is caught by the conservative default
/// arm of [`deepcopy_instr_forces_keep`], which keeps the copy for any unclassified instruction
/// that references a tainted register.)
fn deepcopy_heap_mutation_receiver(instr: &RegInstr) -> Option<Reg> {
    match instr {
        RegInstr::ListSet { list, .. }
        | RegInstr::ListPush { list, .. }
        | RegInstr::ListAppend { list, .. }
        | RegInstr::ListClear { list, .. }
        | RegInstr::ListPop { list, .. }
        | RegInstr::ListSort { list, .. }
        | RegInstr::ListRemoveAt { list, .. } => Some(*list),
        RegInstr::MapInsert { map, .. } | RegInstr::SortedMapInsert { map, .. } => Some(*map),
        RegInstr::SetInsert { set, .. } | RegInstr::SortedSetInsert { set, .. } => Some(*set),
        RegInstr::DequePushBack { deque, .. }
        | RegInstr::DequePushFront { deque, .. }
        | RegInstr::DequePopFront { deque, .. }
        | RegInstr::DequePopBack { deque, .. } => Some(*deque),
        _ => None,
    }
}

/// Push every register referenced (read OR written) by `instr` into `out`. EXHAUSTIVE by
/// design (no wildcard arm) so a future `RegInstr` variant is a COMPILE error rather than a
/// silent hole — the conservative default arm of [`deepcopy_instr_forces_keep`] relies on this
/// to keep the copy whenever a tainted register is touched by an instruction it has not
/// explicitly classified as read-only-safe.
fn deepcopy_collect_regs(instr: &RegInstr, out: &mut Vec<Reg>) {
    match instr {
        RegInstr::LoadUnit { dst }
        | RegInstr::LoadInt { dst, .. }
        | RegInstr::LoadFloat { dst, .. }
        | RegInstr::LoadBool { dst, .. }
        | RegInstr::LoadString { dst, .. }
        | RegInstr::LoadChar { dst, .. }
        | RegInstr::LoadNone { dst } => out.push(*dst),
        RegInstr::Move { dst, src }
        | RegInstr::Manage { dst, src }
        | RegInstr::GetField { dst, base: src, .. }
        | RegInstr::GetFieldSlot { dst, base: src, .. }
        | RegInstr::UnwrapSome { dst, src }
        | RegInstr::UnwrapVariantValue { dst, src, .. }
        | RegInstr::AwaitJoin { dst, src }
        | RegInstr::MakeSome { dst, value: src }
        | RegInstr::NativeClosureId { dst, closure: src }
        | RegInstr::NativeClosureCapture {
            dst, closure: src, ..
        }
        | RegInstr::NativeFieldClosureId { dst, base: src, .. }
        | RegInstr::NativeFieldClosureCapture { dst, base: src, .. } => {
            out.push(*dst);
            out.push(*src);
        }
        RegInstr::DeepCopy { reg }
        | RegInstr::DeepCopyElided { reg }
        | RegInstr::ResourceDrop { resource: reg } => out.push(*reg),
        RegInstr::NativeGuardClosureId { closure, .. } => out.push(*closure),
        RegInstr::SetFieldSlot {
            dst, base, value, ..
        }
        | RegInstr::SetField {
            dst, base, value, ..
        } => {
            out.push(*dst);
            out.push(*base);
            out.push(*value);
        }
        RegInstr::MakeStruct { dst, fields, .. }
        | RegInstr::MakeVariant { dst, fields, .. }
        | RegInstr::MakeObject { dst, fields } => {
            out.push(*dst);
            out.extend(fields.iter().map(|(_, r)| *r));
        }
        RegInstr::MakeList { dst, items } => {
            out.push(*dst);
            out.extend(items.iter().copied());
        }
        RegInstr::MakeMap { dst, entries } => {
            out.push(*dst);
            for (k, v) in entries {
                out.push(*k);
                out.push(*v);
            }
        }
        RegInstr::AddInt { dst, lhs, rhs }
        | RegInstr::SubInt { dst, lhs, rhs }
        | RegInstr::MulInt { dst, lhs, rhs }
        | RegInstr::DivInt { dst, lhs, rhs }
        | RegInstr::ModInt { dst, lhs, rhs }
        | RegInstr::BitAndInt { dst, lhs, rhs }
        | RegInstr::BitOrInt { dst, lhs, rhs }
        | RegInstr::BitXorInt { dst, lhs, rhs }
        | RegInstr::ShiftLeftInt { dst, lhs, rhs }
        | RegInstr::ShiftRightInt { dst, lhs, rhs }
        | RegInstr::LessInt { dst, lhs, rhs }
        | RegInstr::LessEqualInt { dst, lhs, rhs }
        | RegInstr::GreaterInt { dst, lhs, rhs }
        | RegInstr::GreaterEqualInt { dst, lhs, rhs }
        | RegInstr::Equal { dst, lhs, rhs }
        | RegInstr::NotEqual { dst, lhs, rhs }
        | RegInstr::StringConcat {
            dst,
            left: lhs,
            right: rhs,
        } => {
            out.push(*dst);
            out.push(*lhs);
            out.push(*rhs);
        }
        RegInstr::TailCallGuard | RegInstr::Jump { .. } | RegInstr::RuntimeError { .. } => {}
        RegInstr::JumpIfBool { cond, .. } => out.push(*cond),
        RegInstr::JumpIfIntCompare { lhs, rhs, .. } => {
            out.push(*lhs);
            out.push(*rhs);
        }
        RegInstr::MatchOption { src, .. }
        | RegInstr::MatchResult { src, .. }
        | RegInstr::MatchVariant { src, .. }
        | RegInstr::Return { src } => out.push(*src),
        RegInstr::MatchMapGet {
            map,
            key,
            value_dst,
            ..
        }
        | RegInstr::MatchSortedMapGet {
            map,
            key,
            value_dst,
            ..
        } => {
            out.push(*map);
            out.push(*key);
            out.push(*value_dst);
        }
        RegInstr::MakeClosure { dst, captures, .. } => {
            out.push(*dst);
            out.extend(captures.iter().copied());
        }
        RegInstr::CallKnown { dst, args, .. }
        | RegInstr::CallDynamic { dst, args, .. }
        | RegInstr::SpawnTask { dst, args, .. }
        | RegInstr::CallNative { dst, args, .. }
        | RegInstr::CallIntrinsic { dst, args, .. }
        | RegInstr::CallTypedIntrinsic { dst, args, .. } => {
            out.push(*dst);
            out.extend(args.iter().copied());
        }
        RegInstr::CallClosure {
            dst, closure, args, ..
        } => {
            out.push(*dst);
            out.push(*closure);
            out.extend(args.iter().copied());
        }
        RegInstr::SelectWait {
            handles,
            winner,
            value,
        } => {
            out.extend(handles.iter().copied());
            out.push(*winner);
            out.push(*value);
        }
        RegInstr::ListFilter {
            dst,
            list,
            predicate,
        } => {
            out.push(*dst);
            out.push(*list);
            out.push(*predicate);
        }
        RegInstr::ListFold {
            dst,
            list,
            state,
            folder,
        } => {
            out.push(*dst);
            out.push(*list);
            out.push(*state);
            out.push(*folder);
        }
        RegInstr::ListGet {
            dst,
            list: base,
            index: extra,
        }
        | RegInstr::ListRemoveAt {
            dst,
            list: base,
            index: extra,
        }
        | RegInstr::ListMap {
            dst,
            list: base,
            mapper: extra,
        }
        | RegInstr::ListAppend {
            dst,
            list: base,
            values: extra,
        }
        | RegInstr::ListPush {
            dst,
            list: base,
            value: extra,
        }
        | RegInstr::ListSortWith {
            dst,
            list: base,
            compare: extra,
        }
        | RegInstr::DequePushBack {
            dst,
            deque: base,
            value: extra,
        }
        | RegInstr::DequePushFront {
            dst,
            deque: base,
            value: extra,
        }
        | RegInstr::SetInsert {
            dst,
            set: base,
            value: extra,
        }
        | RegInstr::SetRemove {
            dst,
            set: base,
            value: extra,
        }
        | RegInstr::SortedSetInsert {
            dst,
            set: base,
            value: extra,
        }
        | RegInstr::SortedSetRemove {
            dst,
            set: base,
            value: extra,
        }
        | RegInstr::SortedMapRemove {
            dst,
            map: base,
            key: extra,
        }
        | RegInstr::MapGet {
            dst,
            map: base,
            key: extra,
        }
        | RegInstr::MapRemove {
            dst,
            map: base,
            key: extra,
        }
        | RegInstr::CounterAdd {
            dst,
            counter: base,
            amount: extra,
        }
        | RegInstr::ConfigStoreReplace {
            dst,
            store: base,
            value: extra,
        }
        | RegInstr::GlobalConfigReplace {
            dst,
            global: base,
            value: extra,
        }
        | RegInstr::StringBuilderPush {
            dst,
            builder: base,
            value: extra,
        } => {
            out.push(*dst);
            out.push(*base);
            out.push(*extra);
        }
        RegInstr::StringBuilderFinish { dst, builder: base } => {
            out.push(*dst);
            out.push(*base);
        }
        RegInstr::ListLen { dst, list: base }
        | RegInstr::ListClear { dst, list: base }
        | RegInstr::ListPop { dst, list: base }
        | RegInstr::ListSort { dst, list: base }
        | RegInstr::DequeClear { dst, deque: base }
        | RegInstr::DequePopBack { dst, deque: base }
        | RegInstr::DequePopFront { dst, deque: base }
        | RegInstr::SetClear { dst, set: base }
        | RegInstr::SortedSetClear { dst, set: base }
        | RegInstr::SortedMapClear { dst, map: base }
        | RegInstr::MapClear { dst, map: base }
        | RegInstr::BufferClear { dst, buffer: base } => {
            out.push(*dst);
            out.push(*base);
        }
        RegInstr::SetForEach {
            dst,
            set: base,
            callback: extra,
        } => {
            out.push(*dst);
            out.push(*base);
            out.push(*extra);
        }
        RegInstr::ListSet {
            dst,
            list,
            index,
            value,
        }
        | RegInstr::MapInsert {
            dst,
            map: list,
            key: index,
            value,
        }
        | RegInstr::MapInsertOld {
            dst,
            map: list,
            key: index,
            value,
        }
        | RegInstr::SortedMapInsert {
            dst,
            map: list,
            key: index,
            value,
        } => {
            out.push(*dst);
            out.push(*list);
            out.push(*index);
            out.push(*value);
        }
        RegInstr::ListSortBy {
            dst,
            list,
            key,
            compare,
        } => {
            out.push(*dst);
            out.push(*list);
            out.push(*key);
            out.push(*compare);
        }
        RegInstr::TryResult { dst, src, cleanup } => {
            out.push(*dst);
            out.push(*src);
            out.extend(cleanup.iter().copied());
        }
    }
}

/// Taint classification of a collection-read `RegIntrinsic` for `DeepCopy` elision. This is a
/// WHITELIST: only intrinsics VERIFIED (against their `intrinsics/{list,map,set,deque}.rs` impl)
/// to (a) never `borrow_mut` an arg and (b) never store an arg into `self.streams`/`self.channels`
/// or resource state are classified; everything else is [`IntrinsicTaintClass::Keep`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum IntrinsicTaintClass {
    /// Reads its collection args and returns a FRESH scalar/bool/collection-of-scalars whose
    /// contents never alias an arg's inner `Rc`. Safe: the call keeps no copy and needs no taint
    /// propagation (the result can never be a live alias of the param).
    PureFreshReader,
    /// Reads its args, but the RESULT shares an arg's inner `Rc` (an element, a subview, or a
    /// fresh collection holding cloned heap elements). The ARGS are read-only-safe, but taint MUST
    /// propagate arg→dst (see `deepcopy_elidable_param_regs`) so a later mutation/store/return of
    /// the aliased result correctly pins the copy.
    AliasReturner,
    /// Not proven read-only-and-non-storing: conservatively force-keep if it touches a tainted
    /// register. Covers Tier-3 (channels/streams/IO/resource-pool/json/string/…) and every
    /// variant not explicitly whitelisted below. This is the DEFAULT arm — soundness rests on it.
    Keep,
}

/// Classify a `RegIntrinsic` for `DeepCopy`-elision taint tracking. EXPLICIT match with a
/// conservative `Keep` default: an intrinsic is trusted only when its impl has been verified
/// non-mutating and non-storing. Collection READS that lower to dedicated `RegInstr`s
/// (`ListGet`/`ListLen`/`MapGet`) are NOT here — they are handled directly in
/// `deepcopy_instr_forces_keep`/`deepcopy_elidable_param_regs`.
fn deepcopy_intrinsic_class(intrinsic: RegIntrinsic) -> IntrinsicTaintClass {
    use IntrinsicTaintClass::{AliasReturner, PureFreshReader};
    // Three-way split, all POSITIVE (an intrinsic is trusted only when named):
    //   1. PureFreshReader  → READ-ONLY-SAFE, result is fresh (no arg aliasing).
    //   2. AliasReturner    → READ-ONLY-SAFE args, but result aliases an arg's `Rc`
    //                          (taint propagates arg→dst in `deepcopy_elidable_param_regs`).
    //   3. everything else  → UNCLASSIFIED ⇒ `Keep` (the fail-safe default arm; soundness
    //                          rests on it — Tier-3 IO/channels/streams/json/… and any
    //                          not-yet-audited variant conservatively pins the copy).
    match intrinsic {
        // ---- (1) PureFreshReader: fresh scalar/bool/collection-of-scalars, no arg aliasing. ----
        // List
        RegIntrinsic::ListIsEmpty
        | RegIntrinsic::ListContains
        | RegIntrinsic::ListAny
        | RegIntrinsic::ListContainsValue
        | RegIntrinsic::ListCountWhere
        | RegIntrinsic::ListSum
        | RegIntrinsic::ListMin
        | RegIntrinsic::ListMax
        | RegIntrinsic::ListJoin
        | RegIntrinsic::ListConsume
        | RegIntrinsic::ListNew
        // Map
        | RegIntrinsic::MapContainsKey
        | RegIntrinsic::MapLen
        | RegIntrinsic::MapIsEmpty
        | RegIntrinsic::MapForEach
        | RegIntrinsic::MapNew
        // Set / SortedSet / SortedMap
        | RegIntrinsic::SetContains
        | RegIntrinsic::SetIsEmpty
        | RegIntrinsic::SetLen
        | RegIntrinsic::SetIsSubset
        | RegIntrinsic::SetNew
        | RegIntrinsic::SortedSetContains
        | RegIntrinsic::SortedSetIsEmpty
        | RegIntrinsic::SortedSetLen
        | RegIntrinsic::SortedSetNew
        | RegIntrinsic::SortedMapContainsKey
        | RegIntrinsic::SortedMapIsEmpty
        | RegIntrinsic::SortedMapLen
        | RegIntrinsic::SortedMapNew
        // Deque
        | RegIntrinsic::DequeIsEmpty
        | RegIntrinsic::DequeLen
        | RegIntrinsic::DequeNew
        // Char — pure scalar readers: each takes `Char`/`Int` by value and returns
        // a fresh `Int`/`Bool`/`Char`/`String`; none borrow_mut, store into
        // streams/channels/resources, or alias an arg (verified in
        // `intrinsics/char.rs`). Classifying them PureFreshReader lets the elision
        // pass drop a redundant `read List<Char>` prologue DeepCopy when the only
        // keep-forcing use of a `ListGet`-extracted `Char` is one of these — the
        // SH-022 O(n^2) fix (a `read List<Char>` param was deep-copied per call).
        | RegIntrinsic::CharToCode
        | RegIntrinsic::CharFromCode
        | RegIntrinsic::CharToString
        | RegIntrinsic::CharToLower
        | RegIntrinsic::CharToUpper
        | RegIntrinsic::CharIsDigit
        | RegIntrinsic::CharIsAlpha
        | RegIntrinsic::CharIsAlphanumeric
        | RegIntrinsic::CharIsLower
        | RegIntrinsic::CharIsUpper
        | RegIntrinsic::CharIsWhitespace
        | RegIntrinsic::CharCompare
        // String — pure readers (Slice 3). Each takes its receiver by `&`
        // (`expect_string_ref`, never `borrow_mut`), never stores an arg into
        // `self.streams`/`self.channels`/resource state, and RETURNS a FRESH value:
        // a scalar (`Int`/`Bool`/`Char`), a brand-new `Rc<String>` (every
        // `VmValue::string(..)` is `Rc::new(into())`, so even `String.copy`/`slice`/
        // `trim`/`replace` allocate — the result NEVER aliases the arg's `Rc`), a fresh
        // `List` (`chars`/`lines`/`split`), or an `Option`/`Result` wrapping one of those.
        // Verified against `intrinsics/string.rs`: none mutate/store/alias an arg. This
        // lets the elision pass drop a redundant `read String` prologue DeepCopy whose
        // only keep-forcing use is one of these read-only string ops (borrow-by-default).
        | RegIntrinsic::StringAfter
        | RegIntrinsic::StringBefore
        | RegIntrinsic::StringBuilderNew
        | RegIntrinsic::StringCharAt
        | RegIntrinsic::StringChars
        | RegIntrinsic::StringContains
        | RegIntrinsic::StringCount
        | RegIntrinsic::StringCopy
        | RegIntrinsic::StringEndsWith
        | RegIntrinsic::StringFormat
        | RegIntrinsic::StringFromBool
        | RegIntrinsic::StringFromFloat
        | RegIntrinsic::StringFromInt
        | RegIntrinsic::StringIndexOf
        | RegIntrinsic::StringIsEmpty
        | RegIntrinsic::StringJoin
        | RegIntrinsic::StringLines
        | RegIntrinsic::StringLen
        | RegIntrinsic::StringPadLeft
        | RegIntrinsic::StringPadRight
        | RegIntrinsic::StringParseFloat
        | RegIntrinsic::StringParseInt
        | RegIntrinsic::StringRepeat
        | RegIntrinsic::StringReplace
        | RegIntrinsic::StringReplaceFirst
        | RegIntrinsic::StringReverse
        | RegIntrinsic::StringSlice
        | RegIntrinsic::StringSplit
        | RegIntrinsic::StringStartsWith
        | RegIntrinsic::StringStripPrefix
        | RegIntrinsic::StringToLowercase
        | RegIntrinsic::StringToUppercase
        | RegIntrinsic::StringTrim
        | RegIntrinsic::StringTrimEnd
        | RegIntrinsic::StringTrimStart
        // Bytes — pure readers (Slice 3). Each takes its receiver by `&`
        // (`expect_bytes_ref`, never `borrow_mut`), never stores an arg, and returns a
        // FRESH value: a scalar (`BytesLen`→`Int`, `BytesIsEmpty`/`BytesViewStartsWith`→
        // `Bool`), a freshly-allocated `Rc<Vec<u8>>` (`concat`/`slice`/`from_string`/
        // `from_uints`/`view_to_bytes` all `Rc::new` a new `Vec`), a fresh `Rc<String>`
        // (`to_string`), or a fresh `List` (`to_uints`). Verified against
        // `intrinsics/bytes.rs`: none mutate/store/alias an arg (`BytesConsume` merely
        // reads and returns `Unit`).
        | RegIntrinsic::BytesConcat
        | RegIntrinsic::BytesConsume
        | RegIntrinsic::BytesFromString
        | RegIntrinsic::BytesFromUints
        | RegIntrinsic::BytesIsEmpty
        | RegIntrinsic::BytesLen
        | RegIntrinsic::BytesSlice
        | RegIntrinsic::BytesToString
        | RegIntrinsic::BytesToUints
        | RegIntrinsic::BytesViewStartsWith
        | RegIntrinsic::BytesViewToBytes => PureFreshReader,

        // ---- AliasReturner: result shares an arg's inner `Rc`; propagate taint arg→dst. ----
        // List
        RegIntrinsic::ListFirst
        | RegIntrinsic::ListLast
        | RegIntrinsic::ListFind
        | RegIntrinsic::ListSlice
        | RegIntrinsic::ListTake
        | RegIntrinsic::ListSkip
        | RegIntrinsic::ListReverse
        | RegIntrinsic::ListEnumerate
        | RegIntrinsic::ListPartition
        | RegIntrinsic::ListZip
        | RegIntrinsic::ListFlatten
        | RegIntrinsic::ListFlatMap
        | RegIntrinsic::ListDedup
        | RegIntrinsic::ListGroupBy
        | RegIntrinsic::ListTryFold
        // Map
        | RegIntrinsic::MapGetOrDefault
        | RegIntrinsic::MapValues
        | RegIntrinsic::MapFilter
        | RegIntrinsic::MapFold
        | RegIntrinsic::MapMapValues
        | RegIntrinsic::MapMerge
        | RegIntrinsic::MapTryFold
        // Map / Set / SortedSet / SortedMap key & element extractors: the result
        // is a `List` of the map/set's KEYS, which for heap keys (`Set<List<Int>>`,
        // `Map<List<Int>, _>`) share the container's inner `Rc`. Taint must flow
        // arg→dst so the source is not wrongly elided (matches `SortedSetToList`).
        | RegIntrinsic::MapKeys
        | RegIntrinsic::SetToList
        | RegIntrinsic::SortedMapKeys
        | RegIntrinsic::SetDifference
        | RegIntrinsic::SetIntersection
        | RegIntrinsic::SetUnion
        | RegIntrinsic::SortedSetToList
        | RegIntrinsic::SortedMapGet
        | RegIntrinsic::SortedMapValues
        // Deque
        | RegIntrinsic::DequeToList => AliasReturner,

        // ---- (3) UNCLASSIFIED → KEEP (fail-safe default; Tier-3 + any un-audited variant). ----
        _ => IntrinsicTaintClass::Keep,
    }
}

/// WHITELIST verdict for one instruction: does it force the DeepCopy'd param's copy to be
/// KEPT (⇒ NOT elidable)? `tainted[r]` marks every register that aliases the param's inner
/// `Rc`. This is the INTERPRETER-safe (whitelist) counterpart of the native blacklist
/// `native_deepcopy_param_unsoundly_mutated`: the interpreter runs the FULL instruction set,
/// so anything not PROVEN read-only defaults to keep-copy.
///
/// Classification:
/// * In-place mutation of a tainted heap receiver → keep (mirrors native's receiver set;
///   the broader interpreter-only mutators fall through to the conservative default).
/// * Alias-PROPAGATION reads (`Move`/`*Get`/`GetField*`/`UnwrapSome`/`UnwrapVariantValue`/`DequePop*`) →
///   safe: `dst` is already tainted by the closure, and the op does not itself mutate/escape.
/// * Calls (`CallKnown`/`CallDynamic`/`CallNative`/`CallClosure`) → keep ONLY if a tainted
///   value sits in a `mut_args` position; `read` args (and the `closure` receiver) are safe
///   because the callee isolates its own returns under this same scheme.
/// * A small set of pure, non-aliasing SCALAR/fresh producers (integer arithmetic/compare,
///   `ListLen`, `StringConcat`, discriminant `Match*`, `Jump*`, `Load*`) → safe.
/// * Collection-read intrinsics (`CallIntrinsic`/`CallTypedIntrinsic`), per
///   `deepcopy_intrinsic_class`: `PureFreshReader`/`AliasReturner` → safe (args read-only;
///   result-aliasing is handled by taint propagation in `deepcopy_elidable_param_regs`);
///   `Keep` (Tier-3 / unclassified) → keep if a tainted register is touched.
/// * EVERYTHING ELSE (stores into aggregates, `Return`, `MakeClosure` capture, `SpawnTask`,
///   `Manage`, every unlisted mutator, …) → keep if it references ANY tainted register. This
///   default is what makes the analysis a whitelist: an instruction is only trusted when
///   explicitly classified read-only-safe above.
fn deepcopy_instr_forces_keep(instr: &RegInstr, tainted: &[bool], n_regs: usize) -> bool {
    let is_t = |r: Reg| r < n_regs && tainted[r];
    // ---- POSITIVE ESCAPE (keep): in-place mutation of a tainted heap receiver — the direct
    // leak. This is the one escape we detect structurally rather than via the default arm. ----
    if let Some(recv) = deepcopy_heap_mutation_receiver(instr) {
        if is_t(recv) {
            return true;
        }
    }
    // ---- POSITIVE READ-ONLY-SAFE (elide): the arms below return `false` (or key only off a
    // tainted `mut`-arg / classified-`Keep` intrinsic). Anything not matched here falls through
    // to the UNCLASSIFIED → KEEP fail-safe default at the bottom. ----
    match instr {
        // Alias propagation: `dst` aliases `src`'s inner `Rc` (already tainted by the closure);
        // the op only READS `src`, so it never leaks by itself.
        RegInstr::Move { .. }
        | RegInstr::ListGet { .. }
        | RegInstr::MapGet { .. }
        | RegInstr::GetField { .. }
        | RegInstr::GetFieldSlot { .. }
        | RegInstr::UnwrapSome { .. }
        | RegInstr::UnwrapVariantValue { .. }
        | RegInstr::DequePopFront { .. }
        | RegInstr::DequePopBack { .. } => false,

        // A `DeepCopy` (or an already-elided one) is the taint SEED, not a use: it reads its
        // register and yields a FRESH copy, never mutating through nor escaping the arg's `Rc`.
        // It must never be the reason to keep a copy — in particular the prologue `DeepCopy` of
        // the root under analysis must not veto its own elision.
        RegInstr::DeepCopy { .. } | RegInstr::DeepCopyElided { .. } => false,

        // Collection-read intrinsics, classified by `deepcopy_intrinsic_class`:
        //   * PureFreshReader / AliasReturner → args are read-only; a PureFreshReader result is
        //     fresh, and an AliasReturner result's aliasing is handled by taint propagation in
        //     `deepcopy_elidable_param_regs` (so a later mutation/escape of the result pins the
        //     copy there). Either way this CALL neither mutates nor escapes a tainted arg → safe.
        //   * Keep (Tier-3 / unclassified) → conservatively keep if it touches a tainted register
        //     (dst OR any arg), matching the former default-arm behavior.
        RegInstr::CallIntrinsic {
            dst,
            intrinsic,
            args,
        }
        | RegInstr::CallTypedIntrinsic {
            dst,
            intrinsic,
            args,
            ..
        } => match deepcopy_intrinsic_class(*intrinsic) {
            IntrinsicTaintClass::PureFreshReader | IntrinsicTaintClass::AliasReturner => false,
            IntrinsicTaintClass::Keep => is_t(*dst) || args.iter().any(|&r| is_t(r)),
        },

        // Calls: a tainted `mut` arg is mutated by-reference and leaks to our caller. `read`
        // args are safe (the callee deep-copies or has itself proven the param read-only), and
        // the `closure` receiver is only invoked, not mutated.
        RegInstr::CallKnown { args, mut_args, .. }
        | RegInstr::CallDynamic { args, mut_args, .. }
        | RegInstr::CallNative { args, mut_args, .. }
        | RegInstr::CallClosure { args, mut_args, .. } => mut_args
            .iter()
            .any(|&p| args.get(p).is_some_and(|r| is_t(*r))),

        // Pure, non-aliasing producers: read scalars/heap and yield a FRESH scalar or String,
        // or merely branch. None can mutate/store/alias a tainted value.
        RegInstr::AddInt { .. }
        | RegInstr::SubInt { .. }
        | RegInstr::MulInt { .. }
        | RegInstr::DivInt { .. }
        | RegInstr::ModInt { .. }
        | RegInstr::BitAndInt { .. }
        | RegInstr::BitOrInt { .. }
        | RegInstr::BitXorInt { .. }
        | RegInstr::ShiftLeftInt { .. }
        | RegInstr::ShiftRightInt { .. }
        | RegInstr::LessInt { .. }
        | RegInstr::LessEqualInt { .. }
        | RegInstr::GreaterInt { .. }
        | RegInstr::GreaterEqualInt { .. }
        | RegInstr::Equal { .. }
        | RegInstr::NotEqual { .. }
        | RegInstr::StringConcat { .. }
        | RegInstr::ListLen { .. }
        | RegInstr::Jump { .. }
        | RegInstr::JumpIfBool { .. }
        | RegInstr::JumpIfIntCompare { .. }
        | RegInstr::MatchOption { .. }
        | RegInstr::MatchResult { .. }
        | RegInstr::MatchVariant { .. }
        | RegInstr::LoadUnit { .. }
        | RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::LoadChar { .. }
        | RegInstr::LoadNone { .. }
        | RegInstr::RuntimeError { .. }
        | RegInstr::TailCallGuard => false,

        // ---- UNCLASSIFIED → KEEP (fail-safe default; SOUNDNESS BACKBONE — DO NOT WEAKEN). ----
        // Everything else (stores, returns, captures, spawns, `Manage`, `Match*MapGet` extractions,
        // and every unlisted mutator): conservatively keep the copy if a tainted register is
        // involved. `deepcopy_collect_regs` is exhaustive (no wildcard) so a new `RegInstr` variant
        // lands here by default rather than silently escaping the analysis.
        other => {
            let mut regs = Vec::new();
            deepcopy_collect_regs(other, &mut regs);
            regs.iter().any(|&r| is_t(r))
        }
    }
}

/// The set of DeepCopy'd parameter registers whose prologue `DeepCopy` is PROVABLY redundant
/// and may be elided. For each such root, seed a taint set and forward-propagate the alias
/// closure (mirroring `native_deepcopy_param_unsoundly_mutated`'s propagation ops, but with NO
/// type gate — over-tainting only keeps MORE copies, which is always sound). The param is
/// elidable iff NO instruction forces the copy to be kept (see [`deepcopy_instr_forces_keep`]).
///
/// Correctness-critical: when in doubt the analysis keeps the copy. It runs one independent
/// pass per root so a keep verdict is attributed to exactly the responsible parameter.
pub(super) fn deepcopy_elidable_param_regs(
    code: &[RegInstr],
    n_regs: usize,
    scalar_regs: &std::collections::HashSet<Reg>,
) -> std::collections::HashSet<Reg> {
    let roots: Vec<Reg> = code
        .iter()
        .filter_map(|instr| match instr {
            RegInstr::DeepCopy { reg } if *reg < n_regs => Some(*reg),
            _ => None,
        })
        .collect();
    let mut elidable = std::collections::HashSet::new();
    for &root in &roots {
        let mut tainted = vec![false; n_regs];
        tainted[root] = true;
        // Forward alias closure: `dst` aliases `src`'s inner `Rc`. No type gate (unlike the
        // native pass) — the lowerer has no `NativeTy` yet, and tainting a would-be scalar only
        // keeps more copies.
        let mut changed = true;
        while changed {
            changed = false;
            for instr in code {
                // A `Move` copies the value whole, so taint always flows (a scalar
                // dst is harmless — a never-tainted scalar already yields the right
                // verdict, but a `Move` of a heap value must propagate).
                if let RegInstr::Move { dst, src } = instr {
                    if *src < n_regs && *dst < n_regs && tainted[*src] && !tainted[*dst] {
                        tainted[*dst] = true;
                        changed = true;
                    }
                }
                // Extractions (`ListGet`/`MapGet`/`GetField`/`GetFieldSlot`/
                // `UnwrapSome`/`UnwrapVariantValue`/`DequePop*`) pull an INTERIOR value out of a
                // collection/struct/variant. When the lowerer proved `dst` holds a
                // `Copy` scalar (`Int`/`Bool`/`Float`/`Char`/…), that value is inline
                // with no interior `Rc`: it cannot alias `src` or carry `src`'s `Rc`
                // into an escape, so the taint must NOT flow — that spurious edge is
                // exactly what forced the O(n²) copy-keep for per-element helpers.
                // Absence from `scalar_regs` (unknown/`String`/`Bytes`/`Json`/heap)
                // keeps the sound over-tainting behavior.
                else if let RegInstr::ListGet { dst, list: src, .. }
                | RegInstr::MapGet { dst, map: src, .. }
                | RegInstr::GetField { dst, base: src, .. }
                | RegInstr::GetFieldSlot { dst, base: src, .. }
                | RegInstr::UnwrapSome { dst, src }
                | RegInstr::UnwrapVariantValue { dst, src, .. }
                | RegInstr::DequePopFront { dst, deque: src }
                | RegInstr::DequePopBack { dst, deque: src } = instr
                {
                    if *src < n_regs
                        && *dst < n_regs
                        && tainted[*src]
                        && !tainted[*dst]
                        && !scalar_regs.contains(dst)
                    {
                        tainted[*dst] = true;
                        changed = true;
                    }
                }
                // AliasReturner intrinsics: the result shares an arg's inner `Rc`, so taint flows
                // from ANY tainted arg to `dst` (conservative). This makes a downstream mutation
                // or escape of the aliased result force the copy to be kept in
                // `deepcopy_instr_forces_keep`. PureFreshReader/Keep intrinsics do not propagate
                // (a fresh result cannot alias; a Keep result already vetoes elision directly).
                if let RegInstr::CallIntrinsic {
                    dst,
                    args,
                    intrinsic,
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    args,
                    intrinsic,
                    ..
                } = instr
                {
                    if *dst < n_regs
                        && !tainted[*dst]
                        && deepcopy_intrinsic_class(*intrinsic)
                            == IntrinsicTaintClass::AliasReturner
                        && args.iter().any(|&r| r < n_regs && tainted[r])
                    {
                        tainted[*dst] = true;
                        changed = true;
                    }
                }
            }
        }
        if !code
            .iter()
            .any(|instr| deepcopy_instr_forces_keep(instr, &tainted, n_regs))
        {
            elidable.insert(root);
        }
    }
    elidable
}
