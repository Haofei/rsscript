use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use crate::eval_types::NativeValue;
use crate::fnv::FnvHasher;

/// VM `Map` value (key -> value), keyed with Rust's per-process randomized
/// hasher so adversarial scripts cannot precompute one global collision set.
pub(crate) type ValueMap = HashMap<VmMapKey, VmValue>;

#[cfg(any(feature = "native-jit", test))]
#[allow(clippy::mutable_key_type)] // VmMapKey owns an unreachable deep snapshot.
pub(crate) fn clone_value_map_preserving_capacity(map: &ValueMap) -> ValueMap {
    let mut cloned = ValueMap::with_capacity_and_hasher(map.capacity(), map.hasher().clone());
    cloned.extend(map.iter().map(|(key, value)| (key.clone(), value.clone())));
    cloned
}

#[derive(Debug, Clone)]
pub(crate) enum VmValue {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Bytes(Rc<Vec<u8>>),
    String(Rc<String>),
    Json(Rc<serde_json::Value>),
    List(Rc<RefCell<TypedVec>>),
    /// `Deque<T>` — a double-ended queue with O(1) front/back push/pop, unlike a
    /// `Vec`-backed list whose front ops are O(n).
    Deque(Rc<RefCell<std::collections::VecDeque<VmValue>>>),
    Map(Rc<RefCell<ValueMap>>),
    /// The *heap* form of `Some(x)`: the payload is a non-scalar (heap) value.
    /// Renamed from `OptionSome` in V1 (value-representation plan §5) so that no
    /// producer can construct a `Some` without routing through the canonical
    /// [`VmValue::some`] constructor, which enforces the inline/heap split.
    ///
    /// **Canonical disjointness (V1.3):** this arm holds *only* non-scalar
    /// payloads. A scalar `Some` is *always* [`VmValue::OptionSomeScalar`]. The
    /// two arms therefore partition the `Some(x)` value space — there is exactly
    /// one representation per logical value, so `==`/`Hash`/`display` need no
    /// cross-representation normalization.
    OptionSomeHeap(Box<VmValue>),
    OptionNone,
    Struct(Rc<VmStruct>),
    Variant(Rc<VmStruct>),
    Native(Rc<VmNative>),
    Managed(Rc<RefCell<VmValue>>),
    Closure(Rc<VmClosure>),
    /// The *inline* form of `Some(scalar)`: an unboxed scalar payload, no `Box`
    /// allocation (V1.1). Added at the **end** of the enum so existing variants'
    /// Rust discriminants are unchanged — `OptionSomeHeap` keeps the old
    /// `OptionSome` discriminant, so its hash bytes are unchanged. The inline
    /// arm is hashed *as if it were the heap form* (same logical "Some" prefix +
    /// same payload sequence), see [`hash_vm_value`].
    OptionSomeScalar(Scalar),
}

/// The non-recursive, fixed-size scalar payloads that can be inlined into a
/// `VmValue` with no heap cell (V1.1). Kept as one well-tested sub-enum so the
/// inline payload type is a single thing reused across future inline forms
/// (`VariantInline`/`StructInline` in V2). `Bytes`/`String`/`Json` are *not*
/// here: they are `Rc`-backed (heap) and so take the heap `Some` arm.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Scalar {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Unit,
}

impl Scalar {
    /// Try to view a `VmValue` as an inline-eligible scalar. Returns `None` for
    /// the heap-backed (`Rc`) kinds and all composites, which must take the heap
    /// `Some` arm. This is the single place the inline/heap decision is defined.
    #[inline]
    fn from_value(value: &VmValue) -> Option<Scalar> {
        match value {
            VmValue::Int(value) => Some(Scalar::Int(*value)),
            VmValue::Float(value) => Some(Scalar::Float(*value)),
            VmValue::Bool(value) => Some(Scalar::Bool(*value)),
            VmValue::Char(value) => Some(Scalar::Char(*value)),
            VmValue::Unit => Some(Scalar::Unit),
            _ => None,
        }
    }

    /// Materialize the scalar back into a full `VmValue`. Exact inverse of
    /// [`Scalar::from_value`]; cheap (all arms are `Copy`).
    #[inline]
    pub(crate) fn to_value(self) -> VmValue {
        match self {
            Scalar::Int(value) => VmValue::Int(value),
            Scalar::Float(value) => VmValue::Float(value),
            Scalar::Bool(value) => VmValue::Bool(value),
            Scalar::Char(value) => VmValue::Char(value),
            Scalar::Unit => VmValue::Unit,
        }
    }
}

/// Type-specialized backing store for a `List` (Valhalla plan TV1). A list is
/// stored *behind the existing `List` `Rc`* in one of three layouts:
///
/// - `Boxed(Vec<VmValue>)` — the universal fallback: heap elements, generic `T`,
///   mixed/non-scalar element types. Identical to the pre-TV1 representation.
/// - `Ints(Vec<i64>)` — a homogeneous `List<Int>`: a flat `i64[]`, no per-element
///   tag, half the buffer of the boxed form.
/// - `Floats(Vec<f64>)` — a homogeneous `List<Float>`: a flat `f64[]`.
///
/// **Size neutrality (the whole point — see plan §2).** The typed kinds live
/// behind the `List` `Rc`, so `VmValue::List` is still a single pointer and
/// `size_of::<VmValue>()` is unchanged (asserted in tests). This is the opposite
/// of value-rep V2.1, which grew the shared value box and taxed every value.
///
/// **Kind-agnostic observation, NOT canonical (plan §2, revised).** `Boxed`,
/// `Ints`, and `Floats` are *interchangeable representations of the same logical
/// list* — there is **no** canonical-uniqueness requirement. All *observable*
/// operations — `eq`, `hash`, `display`, `native_value`, `deep_copy`, map-key —
/// go through the logical `VmValue` sequence ([`TypedVec::iter`] /
/// [`TypedVec::get`]), so a `Floats` list and a `Boxed([Float, …])` list with the
/// same values are byte-identically equal, hash-equal, and display-equal. A
/// producer specializes **only when it cheaply knows the element kind** (the perf
/// win); otherwise it emits `Boxed` (always correct). RSScript's static typing
/// guarantees homogeneity, so specializing is always *safe* where applied — it is
/// just not *mandatory*. The representation is invisible above this type.
#[derive(Debug, Clone)]
pub(crate) enum TypedVec {
    Boxed(Vec<VmValue>),
    Ints(Vec<i64>),
    Floats(Vec<f64>),
}

impl Default for TypedVec {
    fn default() -> Self {
        TypedVec::Boxed(Vec::new())
    }
}

/// The static element layout a list constructor specializes to (Valhalla TV1,
/// plan §4.3 "construction metadata"). Derived by the lowerer from the static
/// element type and passed to `ListNew`/`MakeList` so an *empty* `List<Float>`
/// starts as `Floats(vec![])` rather than a type-erased `Boxed`. `Boxed` is the
/// always-correct fallback for generic `List<T>`, heap elements, or unknown types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElemKind {
    Boxed,
    Ints,
    Floats,
}

impl ElemKind {
    /// Map a concrete element type *root name* (as the lowerer sees it, e.g.
    /// `"Int"`, `"Float"`) to the specializable kind. Anything else (`String`,
    /// structs, `List`, generic `T`, …) stays `Boxed`.
    pub(crate) fn from_type_name(name: &str) -> ElemKind {
        match name {
            "Int" => ElemKind::Ints,
            "Float" => ElemKind::Floats,
            _ => ElemKind::Boxed,
        }
    }

    /// An empty `TypedVec` of this kind.
    pub(crate) fn empty(self) -> TypedVec {
        match self {
            ElemKind::Boxed => TypedVec::Boxed(Vec::new()),
            ElemKind::Ints => TypedVec::Ints(Vec::new()),
            ElemKind::Floats => TypedVec::Floats(Vec::new()),
        }
    }
}

impl TypedVec {
    /// Clone values while preserving spare capacity. Heap-transaction rollback
    /// uses this snapshot so interpreter replay observes the same future growth
    /// and therefore the same capacity-based memory charges.
    pub(crate) fn clone_preserving_capacity(&self) -> Self {
        fn clone_vec<T: Clone>(values: &[T], capacity: usize) -> Vec<T> {
            let mut cloned = Vec::with_capacity(capacity);
            cloned.extend_from_slice(values);
            cloned
        }
        match self {
            TypedVec::Boxed(values) => TypedVec::Boxed(clone_vec(values, values.capacity())),
            TypedVec::Ints(values) => TypedVec::Ints(clone_vec(values, values.capacity())),
            TypedVec::Floats(values) => TypedVec::Floats(clone_vec(values, values.capacity())),
        }
    }

    /// An empty list. Empty lists are `Boxed` — there is no element to specialize
    /// on, and the first `push`/`from_values` of a homogeneous scalar run picks
    /// the kind. (An empty list compares/hashes/displays identically regardless of
    /// kind, so this is parity-neutral.)
    pub(crate) fn new() -> Self {
        TypedVec::Boxed(Vec::new())
    }

    /// Build the canonical kind for a vector of logical values: `Ints`/`Floats`
    /// if *every* element is the matching scalar (and the list is non-empty),
    /// else `Boxed`. This is the dynamic specialization fallback used when the
    /// lowerer cannot supply a static element type (list-producing ops:
    /// `map`/`filter`/`fold`/`split`/`sort`/collect/…). A `Boxed` result is
    /// always correct; the typed result is purely a layout optimization.
    pub(crate) fn from_values(values: Vec<VmValue>) -> Self {
        if values.is_empty() {
            return TypedVec::Boxed(values);
        }
        if values.iter().all(|v| matches!(v, VmValue::Int(_))) {
            return TypedVec::Ints(
                values
                    .iter()
                    .map(|v| match v {
                        VmValue::Int(i) => *i,
                        _ => unreachable!(),
                    })
                    .collect(),
            );
        }
        if values.iter().all(|v| matches!(v, VmValue::Float(_))) {
            return TypedVec::Floats(
                values
                    .iter()
                    .map(|v| match v {
                        VmValue::Float(f) => *f,
                        _ => unreachable!(),
                    })
                    .collect(),
            );
        }
        TypedVec::Boxed(values)
    }

    /// Per-element heap cost for `allocation_budget` accounting (plan §4.6). A typed
    /// `Ints`/`Floats` element is a flat 8-byte scalar; a `Boxed` element is a full
    /// `VmValue`. The allocation budget charges this per element of growth so
    /// the accounting tracks the *actual* flat buffer cost, not a misleading
    /// `VmValue`-sized charge for a list that is really an `i64[]`.
    pub(crate) fn elem_bytes(&self) -> usize {
        match self {
            TypedVec::Ints(_) => std::mem::size_of::<i64>(),
            TypedVec::Floats(_) => std::mem::size_of::<f64>(),
            TypedVec::Boxed(_) => std::mem::size_of::<VmValue>(),
        }
    }

    /// The `allocation_budget` cost of appending `value` to this list: the kind's
    /// per-element cost, but an empty list that will *specialize* to a scalar kind
    /// on push charges the scalar (8 B) cost, and an empty `Boxed` taking a heap
    /// value charges the `VmValue` cost.
    #[allow(dead_code)] // unfused reference form; superseded on the hot path by
    // `checked_push_accounted` (TV2.1), retained for the accounting-semantics tests
    pub(crate) fn push_cost(&self, value: &VmValue) -> usize {
        match (self, value) {
            (TypedVec::Ints(_), _) | (TypedVec::Floats(_), _) => self.elem_bytes(),
            (TypedVec::Boxed(v), VmValue::Int(_) | VmValue::Float(_)) if v.is_empty() => {
                std::mem::size_of::<i64>()
            }
            _ => std::mem::size_of::<VmValue>(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            TypedVec::Boxed(v) => v.len(),
            TypedVec::Ints(v) => v.len(),
            TypedVec::Floats(v) => v.len(),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        match self {
            TypedVec::Boxed(values) => values.capacity(),
            TypedVec::Ints(values) => values.capacity(),
            TypedVec::Floats(values) => values.capacity(),
        }
    }

    /// TV2: the raw flat `i64` buffer of an `Ints` list as `(ptr, len)`, for the
    /// native tier to index directly. `None` for any other kind. The pointer is
    /// only valid while the backing `Vec` is borrowed and not mutated (the caller's
    /// borrow protocol — see `try_native`).
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) fn as_ints_slice(&self) -> Option<&[i64]> {
        match self {
            TypedVec::Ints(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// TV2 write path: mutable `i64` buffer of an `Ints` list. Native marshalling
    /// converts this slice to a raw pointer while holding the mutable borrow and
    /// protecting the write with the heap transaction rollback guard.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) fn as_ints_mut_slice(&mut self) -> Option<&mut [i64]> {
        match self {
            TypedVec::Ints(v) => Some(v.as_mut_slice()),
            _ => None,
        }
    }

    /// TV2: the raw flat `f64` buffer of a `Floats` list as `(ptr, len)`. `None`
    /// for any other kind.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) fn as_floats_slice(&self) -> Option<&[f64]> {
        match self {
            TypedVec::Floats(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// TV2 write path: mutable `f64` buffer of a `Floats` list — the write-side
    /// mirror of [`as_ints_mut_slice`](Self::as_ints_mut_slice). Native marshalling
    /// converts this slice to a raw pointer while holding the mutable borrow and
    /// protecting the write with the heap transaction rollback guard.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) fn as_floats_mut_slice(&mut self) -> Option<&mut [f64]> {
        match self {
            TypedVec::Floats(v) => Some(v.as_mut_slice()),
            _ => None,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Materialize the element at `index` as a logical `VmValue`, or `None` if out
    /// of bounds. The typed kinds construct a `VmValue::Int`/`Float` on demand
    /// (cheap — a scalar copy, no allocation).
    pub(crate) fn get(&self, index: usize) -> Option<VmValue> {
        match self {
            TypedVec::Boxed(v) => v.get(index).cloned(),
            TypedVec::Ints(v) => v.get(index).map(|i| VmValue::Int(*i)),
            TypedVec::Floats(v) => v.get(index).map(|f| VmValue::Float(*f)),
        }
    }

    /// Overwrite the element at `index`. If the new value does not match the typed
    /// kind (e.g. storing a heap value into an `Ints` list — which the checker
    /// forbids for a well-typed `List<Int>`, but stays total here), the list is
    /// promoted to `Boxed` first. Panics on out-of-bounds, matching `Vec`'s
    /// `v[i] = x` (callers bounds-check before calling).
    pub(crate) fn set(&mut self, index: usize, value: VmValue) {
        match (&mut *self, &value) {
            (TypedVec::Ints(v), VmValue::Int(i)) => v[index] = *i,
            (TypedVec::Floats(v), VmValue::Float(f)) => v[index] = *f,
            (TypedVec::Boxed(v), _) => v[index] = value,
            _ => {
                self.promote_to_boxed();
                if let TypedVec::Boxed(v) = self {
                    v[index] = value;
                }
            }
        }
    }

    /// Append a logical value, specializing or promoting as needed. An empty
    /// `Boxed` list pushing a scalar specializes to the matching typed kind; a
    /// typed list pushing a mismatching value promotes to `Boxed`.
    pub(crate) fn push(&mut self, value: VmValue) {
        match (&mut *self, &value) {
            (TypedVec::Ints(v), VmValue::Int(i)) => v.push(*i),
            (TypedVec::Floats(v), VmValue::Float(f)) => v.push(*f),
            (TypedVec::Boxed(v), _) if !v.is_empty() => v.push(value),
            (TypedVec::Boxed(v), _) if v.is_empty() => {
                // An empty list takes the kind of its first scalar element.
                match value {
                    VmValue::Int(i) => *self = TypedVec::Ints(vec![i]),
                    VmValue::Float(f) => *self = TypedVec::Floats(vec![f]),
                    other => v.push(other),
                }
            }
            _ => {
                self.promote_to_boxed();
                if let TypedVec::Boxed(v) = self {
                    v.push(value);
                }
            }
        }
    }

    /// Checked push for the **VM mutation opcodes** (`ListPush`), where the type
    /// checker *guarantees* the element type. On a kind mismatch (a non-`Int` into
    /// `Ints`, etc.) this returns `Err(())` — a contract violation the caller turns
    /// into a runtime `EvalError`, never a silent promote. In practice this never
    /// fires; it is the defensive total-function tail (plan §4.2 kind-mismatch rule).
    /// An empty `Boxed` list still specializes on its first scalar (parity-neutral).
    #[allow(dead_code)] // unfused reference form; the hot path uses
    // `checked_push_accounted` (TV2.1). Retained for the kind-mismatch tests.
    pub(crate) fn checked_push(&mut self, value: VmValue) -> Result<(), VmValue> {
        match (&mut *self, &value) {
            (TypedVec::Ints(v), VmValue::Int(i)) => {
                v.push(*i);
                Ok(())
            }
            (TypedVec::Floats(v), VmValue::Float(f)) => {
                v.push(*f);
                Ok(())
            }
            (TypedVec::Boxed(v), _) if !v.is_empty() => {
                v.push(value);
                Ok(())
            }
            (TypedVec::Boxed(v), _) if v.is_empty() => {
                match value {
                    VmValue::Int(i) => *self = TypedVec::Ints(vec![i]),
                    VmValue::Float(f) => *self = TypedVec::Floats(vec![f]),
                    other => v.push(other),
                }
                Ok(())
            }
            _ => Err(value),
        }
    }

    /// TV2.1: checked push fused with **amortized capacity-growth accounting** for
    /// the `ListPush` hot path. Does the typed push directly (no intermediate
    /// `VmValue` materialization, single borrow), and returns the number of *new*
    /// heap bytes the backing `Vec` actually allocated on this push — i.e. the
    /// capacity-delta times the kind's element size, charged only when the `Vec`
    /// reallocates (geometric growth, so amortized O(1) accounting calls over a
    /// build loop instead of one `account_bytes` per element).
    ///
    /// Charging on capacity (which is always `>= len`) is conservative for the
    /// allocation budget: it over-counts the unused tail of a grown buffer, never
    /// under-counts, so an adversarial `while true { list.push(x) }` still trips the
    /// limit (in fact slightly sooner). Returns `Err(value)` on kind mismatch with
    /// no bytes charged.
    pub(crate) fn checked_push_accounted(&mut self, value: VmValue) -> Result<usize, VmValue> {
        #[inline(always)]
        fn grow_bytes<T>(v: &Vec<T>, before_cap: usize) -> usize {
            (v.capacity() - before_cap) * std::mem::size_of::<T>()
        }
        match (&mut *self, &value) {
            (TypedVec::Ints(v), VmValue::Int(i)) => {
                let before = v.capacity();
                v.push(*i);
                Ok(grow_bytes(v, before))
            }
            (TypedVec::Floats(v), VmValue::Float(f)) => {
                let before = v.capacity();
                v.push(*f);
                Ok(grow_bytes(v, before))
            }
            (TypedVec::Boxed(v), _) if !v.is_empty() => {
                let before = v.capacity();
                v.push(value);
                Ok(grow_bytes(v, before))
            }
            (TypedVec::Boxed(_), _) => {
                // Empty `Boxed`: specialize on the first scalar (parity-neutral).
                // A fresh `vec![x]` allocates capacity 1, so charge one element.
                match value {
                    VmValue::Int(i) => {
                        *self = TypedVec::Ints(vec![i]);
                        Ok(std::mem::size_of::<i64>())
                    }
                    VmValue::Float(f) => {
                        *self = TypedVec::Floats(vec![f]);
                        Ok(std::mem::size_of::<f64>())
                    }
                    other => {
                        if let TypedVec::Boxed(v) = self {
                            v.push(other);
                            Ok(std::mem::size_of::<VmValue>())
                        } else {
                            unreachable!()
                        }
                    }
                }
            }
            _ => Err(value),
        }
    }

    /// Checked overwrite for the `ListSet` opcode (see [`TypedVec::checked_push`]).
    /// Caller bounds-checks `index < len`. Returns `Err(value)` on kind mismatch.
    pub(crate) fn checked_set(&mut self, index: usize, value: VmValue) -> Result<(), VmValue> {
        match (&mut *self, &value) {
            (TypedVec::Ints(v), VmValue::Int(i)) => {
                v[index] = *i;
                Ok(())
            }
            (TypedVec::Floats(v), VmValue::Float(f)) => {
                v[index] = *f;
                Ok(())
            }
            (TypedVec::Boxed(v), _) => {
                v[index] = value;
                Ok(())
            }
            _ => Err(value),
        }
    }

    pub(crate) fn pop(&mut self) -> Option<VmValue> {
        match self {
            TypedVec::Boxed(v) => v.pop(),
            TypedVec::Ints(v) => v.pop().map(VmValue::Int),
            TypedVec::Floats(v) => v.pop().map(VmValue::Float),
        }
    }

    #[allow(dead_code)] // part of the kind-agnostic accessor API (plan §4.2); kept for the full surface
    pub(crate) fn insert(&mut self, index: usize, value: VmValue) {
        match (&mut *self, &value) {
            (TypedVec::Ints(v), VmValue::Int(i)) => v.insert(index, *i),
            (TypedVec::Floats(v), VmValue::Float(f)) => v.insert(index, *f),
            (TypedVec::Boxed(v), _) if !v.is_empty() => v.insert(index, value),
            (TypedVec::Boxed(v), _) if v.is_empty() => match value {
                VmValue::Int(i) => *self = TypedVec::Ints(vec![i]),
                VmValue::Float(f) => *self = TypedVec::Floats(vec![f]),
                other => v.insert(index, other),
            },
            _ => {
                self.promote_to_boxed();
                if let TypedVec::Boxed(v) = self {
                    v.insert(index, value);
                }
            }
        }
    }

    pub(crate) fn remove(&mut self, index: usize) -> VmValue {
        match self {
            TypedVec::Boxed(v) => v.remove(index),
            TypedVec::Ints(v) => VmValue::Int(v.remove(index)),
            TypedVec::Floats(v) => VmValue::Float(v.remove(index)),
        }
    }

    /// The first element as a logical value (materializing), or `None` if empty.
    pub(crate) fn first(&self) -> Option<VmValue> {
        self.get(0)
    }

    /// The last element as a logical value, or `None` if empty.
    pub(crate) fn last(&self) -> Option<VmValue> {
        let len = self.len();
        if len == 0 { None } else { self.get(len - 1) }
    }

    /// Reverse the elements in place (kind-preserving — a flat scalar list reverses
    /// its raw buffer; `Boxed` reverses its `Vec`).
    pub(crate) fn reverse(&mut self) {
        match self {
            TypedVec::Boxed(v) => v.reverse(),
            TypedVec::Ints(v) => v.reverse(),
            TypedVec::Floats(v) => v.reverse(),
        }
    }

    pub(crate) fn clear(&mut self) {
        match self {
            TypedVec::Boxed(v) => v.clear(),
            TypedVec::Ints(v) => v.clear(),
            TypedVec::Floats(v) => v.clear(),
        }
    }

    /// Append all logical values from an iterator, specializing/promoting per
    /// element (so an extend of mixed kinds lands in `Boxed`).
    pub(crate) fn extend(&mut self, values: impl IntoIterator<Item = VmValue>) {
        for value in values {
            self.push(value);
        }
    }

    /// Allocated heap bytes of the backing buffer: `capacity * elem_bytes` for the
    /// *current* kind. Boxed counts each slot at the full `VmValue` size.
    pub(crate) fn allocated_bytes(&self) -> usize {
        match self {
            TypedVec::Ints(v) => v.capacity() * std::mem::size_of::<i64>(),
            TypedVec::Floats(v) => v.capacity() * std::mem::size_of::<f64>(),
            TypedVec::Boxed(v) => v.capacity() * std::mem::size_of::<VmValue>(),
        }
    }

    /// `extend`, but accounted against the **destination's** real layout. Charges
    /// the capacity-growth bytes of *this* buffer (post-extend minus pre-extend),
    /// so a flat `Ints` source appended into a `Boxed` receiver is billed at the
    /// 16 B `VmValue` slot it actually occupies — not the source's 8 B. Mirrors the
    /// amortized capacity-delta model of [`checked_push_accounted`]; never
    /// under-counts (a kind promotion that shrinks net bytes charges 0, already
    /// covered by the freed buffer's earlier charge). Returns the bytes to feed to
    /// `account_bytes`.
    pub(crate) fn extend_accounted(&mut self, values: impl IntoIterator<Item = VmValue>) -> usize {
        let before = self.allocated_bytes();
        self.extend(values);
        self.allocated_bytes().saturating_sub(before)
    }

    /// Replace the entire contents with a fresh vector of logical values,
    /// re-picking the canonical kind. Mirrors `*borrowed = new_vec`.
    #[allow(dead_code)] // part of the kind-agnostic accessor API (plan §4.2)
    pub(crate) fn replace_all(&mut self, values: Vec<VmValue>) {
        *self = TypedVec::from_values(values);
    }

    /// Materialize every element as a `Vec<VmValue>` (the logical view). Used by
    /// ops that need an owned snapshot (`.borrow().clone()` sites: append-source,
    /// sort, drain-to-vec) and by callers handing the list to closure machinery.
    pub(crate) fn to_vec(&self) -> Vec<VmValue> {
        match self {
            TypedVec::Boxed(v) => v.clone(),
            TypedVec::Ints(v) => v.iter().map(|i| VmValue::Int(*i)).collect(),
            TypedVec::Floats(v) => v.iter().map(|f| VmValue::Float(*f)).collect(),
        }
    }

    /// Materialize the half-open `[start, end)` sub-range as a logical
    /// `Vec<VmValue>` (callers bounds-check `start <= end <= len`). Mirrors the old
    /// `borrowed[start..end].to_vec()`.
    pub(crate) fn slice_to_vec(&self, start: usize, end: usize) -> Vec<VmValue> {
        match self {
            TypedVec::Boxed(v) => v[start..end].to_vec(),
            TypedVec::Ints(v) => v[start..end].iter().map(|i| VmValue::Int(*i)).collect(),
            TypedVec::Floats(v) => v[start..end].iter().map(|f| VmValue::Float(*f)).collect(),
        }
    }

    /// Iterate the elements as logical `VmValue`s. The typed kinds materialize
    /// each scalar on the fly (no buffer allocation). This is the single accessor
    /// behind eq/hash/display/native-conversion, so all kinds are observably
    /// identical.
    pub(crate) fn iter(&self) -> TypedVecIter<'_> {
        match self {
            TypedVec::Boxed(v) => TypedVecIter::Boxed(v.iter()),
            TypedVec::Ints(v) => TypedVecIter::Ints(v.iter()),
            TypedVec::Floats(v) => TypedVecIter::Floats(v.iter()),
        }
    }

    /// Convert this list into the `Boxed` kind in place (materializing the typed
    /// scalars), and return the underlying `Vec<VmValue>` for `Vec`-shaped
    /// in-place mutation (sorted-set/sorted-map ops, drains, sorts that need a
    /// `&mut Vec<VmValue>`). The result is still canonical: a homogeneous scalar
    /// list that flows back out through `from_values`/`push` will re-specialize.
    /// Promoting here is parity-neutral — a `Boxed([Int, …])` and an `Ints([…])`
    /// are observably identical.
    pub(crate) fn as_boxed_mut(&mut self) -> &mut Vec<VmValue> {
        self.promote_to_boxed();
        match self {
            TypedVec::Boxed(v) => v,
            _ => unreachable!("promote_to_boxed leaves a Boxed"),
        }
    }

    fn promote_to_boxed(&mut self) {
        match self {
            TypedVec::Boxed(_) => {}
            TypedVec::Ints(v) => {
                *self = TypedVec::Boxed(v.iter().map(|i| VmValue::Int(*i)).collect());
            }
            TypedVec::Floats(v) => {
                *self = TypedVec::Boxed(v.iter().map(|f| VmValue::Float(*f)).collect());
            }
        }
    }
}

impl<'a> IntoIterator for &'a TypedVec {
    type Item = VmValue;
    type IntoIter = TypedVecIter<'a>;
    fn into_iter(self) -> TypedVecIter<'a> {
        self.iter()
    }
}

impl IntoIterator for TypedVec {
    type Item = VmValue;
    type IntoIter = std::vec::IntoIter<VmValue>;
    /// Consume into an owned `Vec<VmValue>` iterator (materializing typed scalars).
    /// Mirrors the old `for v in vec` over a `Vec<VmValue>`.
    fn into_iter(self) -> std::vec::IntoIter<VmValue> {
        match self {
            TypedVec::Boxed(v) => v.into_iter(),
            other => other.to_vec().into_iter(),
        }
    }
}

impl FromIterator<VmValue> for TypedVec {
    /// Collect a stream of logical values into the canonical kind. Mirrors the old
    /// `collect::<Vec<VmValue>>()` sites: materialize, then specialize via
    /// [`TypedVec::from_values`] (so a homogeneous scalar collect lands typed).
    fn from_iter<I: IntoIterator<Item = VmValue>>(iter: I) -> Self {
        TypedVec::from_values(iter.into_iter().collect())
    }
}

impl PartialEq for TypedVec {
    fn eq(&self, other: &Self) -> bool {
        // Compare *logically* across kinds so a `Floats` list and a
        // `Boxed([Float, …])` list with the same values are equal (parity §2).
        if self.len() != other.len() {
            return false;
        }
        self.iter().eq(other.iter())
    }
}

impl Eq for TypedVec {}

/// A materializing iterator over a [`TypedVec`]'s logical `VmValue` elements.
pub(crate) enum TypedVecIter<'a> {
    Boxed(std::slice::Iter<'a, VmValue>),
    Ints(std::slice::Iter<'a, i64>),
    Floats(std::slice::Iter<'a, f64>),
}

impl Iterator for TypedVecIter<'_> {
    type Item = VmValue;

    fn next(&mut self) -> Option<VmValue> {
        match self {
            TypedVecIter::Boxed(it) => it.next().cloned(),
            TypedVecIter::Ints(it) => it.next().map(|i| VmValue::Int(*i)),
            TypedVecIter::Floats(it) => it.next().map(|f| VmValue::Float(*f)),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            TypedVecIter::Boxed(it) => it.size_hint(),
            TypedVecIter::Ints(it) => it.size_hint(),
            TypedVecIter::Floats(it) => it.size_hint(),
        }
    }
}

impl ExactSizeIterator for TypedVecIter<'_> {}

/// The shared, interned per-type layout of a struct/variant: its type **name**
/// plus the ordered field names. Shared (via `Rc`) across all instances of the
/// same `(name, field_names)` shape so neither the name nor the names are
/// duplicated per instance, and field access can be an offset index rather than a
/// string hash.
///
/// **V2.0 (value-representation plan §6).** The name now lives here, on the shared
/// layout, rather than on each `VmStruct` instance. Two motivations:
/// 1. It is the prerequisite for V2.1 variant inlining: an inline composite carries
///    only `Rc<TypeLayout>` + inline fields, and must still be able to hash by the
///    canonical type name — which now rides on the shared layout.
/// 2. With the name on the layout, the layout fully identifies the type, so layouts
///    can be **interned** (see [`intern_layout`]): a repeated `(name, field_names)`
///    shape reuses one `Rc<TypeLayout>` (a refcount bump) instead of allocating a
///    fresh `Rc<StructLayout>` on every construction (the old `from_named` did).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct TypeLayout {
    pub(crate) name: Rc<str>,
    pub(crate) field_names: Vec<Rc<str>>,
}

impl TypeLayout {
    pub(crate) fn new(name: Rc<str>, field_names: Vec<Rc<str>>) -> Self {
        TypeLayout { name, field_names }
    }

    /// The slot of `field`, or `None` if absent. A linear scan — struct field
    /// counts are tiny, so this beats hashing, and the hot path uses precomputed
    /// slots (`GetFieldSlot`) from the lowerer anyway.
    pub(crate) fn slot(&self, field: &str) -> Option<usize> {
        self.field_names.iter().position(|name| &**name == field)
    }
}

/// Interner map: a `(name, field_names)` shape to its shared [`TypeLayout`].
type LayoutInterner = HashMap<(Rc<str>, Vec<Rc<str>>), Rc<TypeLayout>>;

thread_local! {
    /// The per-VM dynamic layout interner (V2.0). Keyed by `(name, field_names)`,
    /// it maps a type shape to its single shared `Rc<TypeLayout>`. First lookup of a
    /// shape builds + caches the layout; later lookups return a refcount-bumped clone
    /// of the *same* `Rc`.
    ///
    /// **Why thread-local rather than per-`RegVm`.** Several layout producers are
    /// free functions with no `RegVm` in scope — the runtime/native/error builders
    /// `value_ok`/`value_err`, `vm_value_from_native_value`, `json_error_value`, and
    /// the many `*_value` helpers in `reg_vm/`. Threading an interner handle through
    /// all of them would contort those signatures for no semantic gain. The VM is
    /// single-threaded, so a thread-local `RefCell<HashMap>` is the pragmatic choice
    /// and needs no `Arc`/locking.
    ///
    /// **Why this is parity-neutral.** The interner only *deduplicates* layouts; it
    /// never changes a value's name, field names, field values, arity, hash, equality,
    /// or display. `hash`/`eq`/`display` read `layout.name` + `field_names` + `fields`,
    /// which are byte-identical whether the layout is freshly built or interned. So
    /// observable behavior (including `Map` iteration order) is unchanged. The cache
    /// is process-thread-lifetime; it is never cleared, which is correct because a
    /// `(name, field_names)` shape maps to exactly one immutable layout forever.
    static LAYOUT_INTERNER: RefCell<LayoutInterner> =
        RefCell::new(HashMap::new());
}

/// Intern a `(name, field_names)` shape to its shared [`TypeLayout`] (V2.0). The
/// first call for a shape builds and caches the layout; subsequent calls return a
/// refcount-bumped clone of the same `Rc`, eliminating the per-construction
/// `Rc::new(StructLayout)` the old `from_named` did for repeated shapes.
pub(crate) fn intern_layout(name: Rc<str>, field_names: Vec<Rc<str>>) -> Rc<TypeLayout> {
    LAYOUT_INTERNER.with(|interner| {
        let mut interner = interner.borrow_mut();
        if let Some(layout) = interner.get(&(Rc::clone(&name), field_names.clone())) {
            return Rc::clone(layout);
        }
        let layout = Rc::new(TypeLayout::new(Rc::clone(&name), field_names.clone()));
        interner.insert((name, field_names), Rc::clone(&layout));
        layout
    })
}

#[derive(Debug, Clone)]
pub(crate) struct VmStruct {
    pub(crate) layout: Rc<TypeLayout>,
    pub(crate) fields: Vec<VmValue>,
}

impl VmStruct {
    /// Build a struct/variant from named field/value pairs (the field order
    /// becomes the slot order). The `(name, field_names)` shape is **interned**
    /// (V2.0) — repeated shapes share one `Rc<TypeLayout>` rather than allocating a
    /// fresh layout per construction. Callers that already hold an interned layout
    /// (e.g. copy-on-write field writes) reuse it via [`VmStruct::with_layout`].
    pub(crate) fn from_named<K: Into<Rc<str>>>(
        name: impl Into<Rc<str>>,
        fields: impl IntoIterator<Item = (K, VmValue)>,
    ) -> Self {
        let (field_names, values): (Vec<Rc<str>>, Vec<VmValue>) = fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .unzip();
        VmStruct {
            layout: intern_layout(name.into(), field_names),
            fields: values,
        }
    }

    /// Build an instance from an already-interned layout, sharing it (a refcount
    /// bump, no allocation). Used by copy-on-write field writes and deep copies,
    /// which keep the same immutable type layout and replace only the field values.
    pub(crate) fn with_layout(layout: Rc<TypeLayout>, fields: Vec<VmValue>) -> Self {
        VmStruct { layout, fields }
    }

    /// The type name, carried on the shared interned layout (V2.0).
    pub(crate) fn name(&self) -> &Rc<str> {
        &self.layout.name
    }

    pub(crate) fn slot(&self, field: &str) -> Option<usize> {
        self.layout.slot(field)
    }

    pub(crate) fn get(&self, field: &str) -> Option<&VmValue> {
        self.layout
            .slot(field)
            .and_then(|slot| self.fields.get(slot))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Iterate `(field_name, value)` pairs in slot order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Rc<str>, &VmValue)> {
        self.layout.field_names.iter().zip(self.fields.iter())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct VmClosure {
    pub(crate) function: usize,
    pub(crate) captures: Vec<VmValue>,
}

#[derive(Debug, Clone)]
pub(crate) struct VmNative {
    pub(crate) type_name: Rc<str>,
    pub(crate) id: i64,
}

/// A `Map`/`Set` key: any hashable `VmValue`. Keys are not restricted to scalars
/// — RSScript's `Hashable` bound also admits `derives(Eq, Hash)` structs/sums and
/// the structural `List`/`Option` containers over hashable types, all of which
/// the compiled backend keys on directly. Equality reuses `VmValue`'s structural
/// `PartialEq`; `Hash` is a matching recursive projection (see [`hash_vm_value`]),
/// so equal keys hash equal and the VM stays in lockstep with the derived
/// `Hash`/`Eq` of the lowered Rust.
#[derive(Debug, Clone)]
pub(crate) struct VmMapKey(VmValue);

impl VmMapKey {
    pub(crate) fn new(value: VmValue) -> Self {
        VmMapKey(snapshot_key_value(&value))
    }

    pub(crate) fn from_string(value: impl Into<String>) -> Self {
        VmMapKey(VmValue::string(value))
    }

    /// Borrow the table's private immutable snapshot for internal operations.
    pub(crate) fn value(&self) -> &VmValue {
        &self.0
    }

    /// Return a value-semantics copy that shares no mutable storage with the key
    /// retained by the hash table. Exposing the retained `Rc<RefCell<_>>` itself
    /// through `Map.keys`/`Set.to_list` would let user code mutate the key after
    /// insertion and invalidate the table's hash buckets.
    pub(crate) fn detached_value(&self) -> VmValue {
        snapshot_key_value(&self.0)
    }

    /// The key's string contents, when it is a `String` key (e.g. JSON object
    /// keys must be strings).
    pub(crate) fn as_str(&self) -> Option<&str> {
        match &self.0 {
            VmValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn display(&self) -> String {
        self.0.display()
    }

    pub(crate) fn native_value(&self) -> Option<NativeValue> {
        self.0.native_value()
    }
}

/// Materialize the immutable value snapshot stored by `Map`/`Set`. RSScript's
/// AOT backend passes value keys by value; the VM must do the same instead of
/// retaining aliases to its `Rc<RefCell<_>>` representation.
fn snapshot_key_value(value: &VmValue) -> VmValue {
    match value {
        VmValue::Unit => VmValue::Unit,
        VmValue::Int(value) => VmValue::Int(*value),
        VmValue::Float(value) => VmValue::Float(*value),
        VmValue::Bool(value) => VmValue::Bool(*value),
        VmValue::Char(value) => VmValue::Char(*value),
        VmValue::Bytes(value) => VmValue::Bytes(Rc::clone(value)),
        VmValue::String(value) => VmValue::String(Rc::clone(value)),
        VmValue::Json(value) => VmValue::Json(Rc::clone(value)),
        VmValue::List(items) => VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
            items
                .borrow()
                .iter()
                .map(|item| snapshot_key_value(&item))
                .collect(),
        )))),
        VmValue::Deque(items) => VmValue::Deque(Rc::new(RefCell::new(
            items.borrow().iter().map(snapshot_key_value).collect(),
        ))),
        VmValue::Map(entries) => VmValue::Map(Rc::new(RefCell::new(
            entries
                .borrow()
                .iter()
                .map(|(key, value)| (key.clone(), snapshot_key_value(value)))
                .collect(),
        ))),
        VmValue::OptionSomeHeap(value) => {
            VmValue::OptionSomeHeap(Box::new(snapshot_key_value(value)))
        }
        VmValue::OptionSomeScalar(value) => VmValue::OptionSomeScalar(*value),
        VmValue::OptionNone => VmValue::OptionNone,
        VmValue::Struct(data) => VmValue::Struct(Rc::new(VmStruct::with_layout(
            Rc::clone(&data.layout),
            data.fields.iter().map(snapshot_key_value).collect(),
        ))),
        VmValue::Variant(data) => VmValue::Variant(Rc::new(VmStruct::with_layout(
            Rc::clone(&data.layout),
            data.fields.iter().map(snapshot_key_value).collect(),
        ))),
        VmValue::Native(data) => VmValue::Native(Rc::clone(data)),
        VmValue::Managed(value) => snapshot_key_value(&value.borrow()),
        VmValue::Closure(value) => VmValue::Closure(Rc::clone(value)),
    }
}

impl PartialEq for VmMapKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for VmMapKey {}

impl std::hash::Hash for VmMapKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        hash_vm_value(&self.0, state);
    }
}

/// Recursively hash a `VmValue` consistently with [`VmValue`]'s `PartialEq`:
/// equal values must hash equal. `Managed` is transparent in equality, so it is
/// unwrapped here *before* the discriminant is mixed in (a `Managed(Int(1))`
/// must hash like `Int(1)`). `Float` keys cannot occur (not `Hashable`), but the
/// ±0.0 case is normalized anyway so the function is a correct `Hash` for any
/// `VmValue`.
/// Hash the discriminant prefix of the heap `Some` arm. The inline `Some` arm
/// calls this so it hashes with the *same* "Some" discriminant the heap arm
/// (and the pre-V1 `OptionSome`) emits — keeping `Some(scalar)` hashes, and thus
/// the same structural stream. The discriminant is cached once (it is `Copy`
/// and cheap to re-hash) so there is no per-hash `Box` allocation.
fn hash_some_prefix<H: std::hash::Hasher>(state: &mut H) {
    use std::hash::Hash;
    use std::mem::Discriminant;
    use std::sync::OnceLock;

    static SOME_DISCRIMINANT: OnceLock<Discriminant<VmValue>> = OnceLock::new();
    let discriminant = SOME_DISCRIMINANT
        .get_or_init(|| std::mem::discriminant(&VmValue::OptionSomeHeap(Box::new(VmValue::Unit))));
    discriminant.hash(state);
}

fn hash_vm_value<H: std::hash::Hasher>(value: &VmValue, state: &mut H) {
    hash_vm_value_inner(value, state, &mut HashMap::new(), 0);
}

fn hash_vm_value_inner<H: std::hash::Hasher>(
    value: &VmValue,
    state: &mut H,
    active: &mut HashMap<usize, u64>,
    depth: u64,
) {
    use std::hash::Hash;

    let node = vm_value_node_id(value);
    if let Some(node) = node {
        if let Some(first_depth) = active.get(&node) {
            0x5253_5343_5943_4c45_u64.hash(state);
            first_depth.hash(state);
            return;
        }
        active.insert(node, depth);
    }

    if let VmValue::Managed(inner) = value {
        hash_vm_value_inner(&inner.borrow(), state, active, depth + 1);
        if let Some(node) = node {
            active.remove(&node);
        }
        return;
    }

    // The inline `Some` form must hash *byte-identically* to the old
    // `OptionSome(Box(scalar))` so representation changes do not alter structural
    // hash equality. We achieve that by hashing the inline arm exactly as the
    // heap arm: emit the *heap*
    // discriminant ("Some") prefix, then the payload sequence. Because
    // `OptionSomeScalar` was added at the END of the enum, `OptionSomeHeap`
    // retains the old `OptionSome` discriminant, so the cached prefix below is
    // bit-for-bit the old prefix. No `Box` is allocated per hash.
    if let VmValue::OptionSomeScalar(scalar) = value {
        hash_some_prefix(state);
        hash_vm_value_inner(&scalar.to_value(), state, active, depth + 1);
        if let Some(node) = node {
            active.remove(&node);
        }
        return;
    }

    std::mem::discriminant(value).hash(state);
    match value {
        VmValue::Unit | VmValue::OptionNone => {}
        VmValue::Bool(value) => value.hash(state),
        VmValue::Int(value) => value.hash(state),
        VmValue::Char(value) => value.hash(state),
        VmValue::String(value) => value.hash(state),
        VmValue::Bytes(value) => value.hash(state),
        VmValue::Float(value) => {
            let bits = if *value == 0.0 { 0 } else { value.to_bits() };
            bits.hash(state);
        }
        VmValue::Json(value) => value.to_string().hash(state),
        VmValue::List(items) => {
            // Hash through the *logical* element sequence so a typed (`Ints`/
            // `Floats`) list and a `Boxed` list with the same values emit the
            // identical byte stream. `iter()` materializes each scalar as a
            // `VmValue`.
            let items = items.borrow();
            items.len().hash(state);
            for item in items.iter() {
                hash_vm_value_inner(&item, state, active, depth + 1);
            }
        }
        VmValue::Deque(items) => {
            let items = items.borrow();
            items.len().hash(state);
            for item in items.iter() {
                hash_vm_value_inner(item, state, active, depth + 1);
            }
        }
        VmValue::OptionSomeHeap(inner) => hash_vm_value_inner(inner, state, active, depth + 1),
        // Hashed via the early-return above (as the heap form); unreachable here.
        VmValue::OptionSomeScalar(_) => unreachable!("OptionSomeScalar handled above"),
        VmValue::Struct(data) | VmValue::Variant(data) => {
            data.name().hash(state);
            for field in &data.fields {
                hash_vm_value_inner(field, state, active, depth + 1);
            }
        }
        VmValue::Native(data) => {
            data.type_name.hash(state);
            data.id.hash(state);
        }
        // Not a hashable key shape (the checker rejects `Map`/closure keys), but
        // stay total: an order-independent fold so equal maps hash equally.
        VmValue::Map(entries) => {
            let mut acc: u64 = 0;
            for (key, value) in entries.borrow().iter() {
                let mut hasher = FnvHasher::default();
                key.hash(&mut hasher);
                hash_vm_value_inner(value, &mut hasher, active, depth + 1);
                acc = acc.wrapping_add(std::hash::Hasher::finish(&hasher));
            }
            acc.hash(state);
        }
        VmValue::Closure(closure) => (Rc::as_ptr(closure) as usize).hash(state),
        VmValue::Managed(_) => unreachable!("Managed handled above"),
    }
    if let Some(node) = node {
        active.remove(&node);
    }
}

pub(crate) fn vm_value_node_id(value: &VmValue) -> Option<usize> {
    match value {
        VmValue::List(value) => Some(Rc::as_ptr(value) as usize),
        VmValue::Deque(value) => Some(Rc::as_ptr(value) as usize),
        VmValue::Map(value) => Some(Rc::as_ptr(value) as usize),
        VmValue::Struct(value) | VmValue::Variant(value) => Some(Rc::as_ptr(value) as usize),
        VmValue::Managed(value) => Some(Rc::as_ptr(value) as usize),
        _ => None,
    }
}

impl VmValue {
    pub(crate) fn string(value: impl Into<String>) -> Self {
        Self::String(Rc::new(value.into()))
    }

    /// Build a `List` from a vector of logical values, picking the canonical
    /// [`TypedVec`] kind (`Ints`/`Floats` for a homogeneous scalar run, else
    /// `Boxed`). The single dynamic-specialization construction path for list
    /// values whose static element type the lowerer does not pin down (the many
    /// list-*producing* ops: `map`/`filter`/`fold`/`split`/collect/sort/…).
    #[allow(dead_code)] // convenience constructor mirroring `from_values`; producers wrap inline
    pub(crate) fn list(values: Vec<VmValue>) -> Self {
        Self::List(Rc::new(RefCell::new(TypedVec::from_values(values))))
    }

    /// Build an empty `List` (a `Boxed` `TypedVec`; the first scalar `push`
    /// specializes it).
    #[allow(dead_code)] // convenience constructor; empty-list producers wrap inline / via ElemKind
    pub(crate) fn empty_list() -> Self {
        Self::List(Rc::new(RefCell::new(TypedVec::new())))
    }

    /// The **single canonical constructor** for `Some(value)` (V1.1). It is the
    /// only place that decides inline-vs-heap, so the canonical-disjointness
    /// invariant (V1.3) is enforced mechanically rather than by convention:
    /// a scalar payload *always* becomes the inline [`Self::OptionSomeScalar`]
    /// (no `Box`), a non-scalar payload *always* the heap [`Self::OptionSomeHeap`].
    /// Every `Some` producer must route through here; raw construction of either
    /// arm outside this function is forbidden (and the `*Heap` rename makes a
    /// stray raw construction fail to compile).
    #[inline]
    pub(crate) fn some(value: VmValue) -> VmValue {
        match Scalar::from_value(&value) {
            Some(scalar) => VmValue::OptionSomeScalar(scalar),
            None => VmValue::OptionSomeHeap(Box::new(value)),
        }
    }

    /// The payload of a `Some(x)`, materialized as an owned `VmValue`, or `None`
    /// for `OptionNone` / any non-Option value. Unifies the inline and heap
    /// arms so callers never have to know which representation is in play. Cheap:
    /// the inline arm copies a scalar, the heap arm clones the boxed value (the
    /// same clone the old `(**value).clone()` did).
    #[inline]
    pub(crate) fn unwrap_some(&self) -> Option<VmValue> {
        match self {
            VmValue::OptionSomeScalar(scalar) => Some(scalar.to_value()),
            VmValue::OptionSomeHeap(value) => Some((**value).clone()),
            _ => None,
        }
    }

    /// Whether this value has value (not reference) semantics and no in-place
    /// mutation — so wrapping it in a `Managed` shared cell would be a semantic
    /// no-op. Used by the `manage` op to avoid leaking an opaque `Managed`
    /// around immutable scalars (`String`/`Bytes`/`Json` are `Rc`-shared and
    /// immutable; the rest are `Copy`).
    pub(crate) fn is_immutable_scalar(&self) -> bool {
        matches!(
            self,
            Self::Unit
                | Self::Int(_)
                | Self::Float(_)
                | Self::Bool(_)
                | Self::Char(_)
                | Self::Bytes(_)
                | Self::String(_)
                | Self::Json(_)
        )
    }

    pub(crate) fn display(&self) -> String {
        self.display_inner(&mut DisplayState::default())
    }

    fn display_inner(&self, state: &mut DisplayState) -> String {
        let node = vm_value_node_id(self);
        if let Some(node) = node {
            let id = state.id_for(node);
            if !state.active.insert(node) {
                return format!("<cycle#{id}>");
            }
        }

        let rendered = match self {
            Self::Unit => "Unit".to_string(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Char(value) => value.to_string(),
            Self::Bytes(value) => format!("{value:?}"),
            Self::String(value) => value.to_string(),
            Self::Json(value) => {
                serde_json::to_string(value.as_ref()).unwrap_or_else(|_| "<json>".to_string())
            }
            Self::List(values) => {
                let values = values
                    .borrow()
                    .iter()
                    .map(|value| value.display_inner(state))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
            Self::Deque(values) => {
                let values = values
                    .borrow()
                    .iter()
                    .map(|value| value.display_inner(state))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
            Self::Map(entries) => {
                let mut values = entries
                    .borrow()
                    .iter()
                    .map(|(key, value)| {
                        format!("{}: {}", key.display(), value.display_inner(state))
                    })
                    .collect::<Vec<_>>();
                values.sort();
                let values = values.join(", ");
                format!("{{{values}}}")
            }
            Self::OptionSomeHeap(value) => format!("Some({})", value.display_inner(state)),
            Self::OptionSomeScalar(scalar) => format!("Some({})", scalar.to_value().display()),
            Self::OptionNone => "None".to_string(),
            Self::Struct(data) | Self::Variant(data) => {
                if data.is_empty() {
                    data.name().to_string()
                } else {
                    let mut values = data
                        .iter()
                        .map(|(key, value)| format!("{key}: {}", value.display_inner(state)))
                        .collect::<Vec<_>>();
                    values.sort();
                    let values = values.join(", ");
                    format!("{} {{ {values} }}", data.name())
                }
            }
            Self::Native(data) => format!("<native {}#{}>", data.type_name, data.id),
            Self::Managed(value) => value.borrow().display_inner(state),
            Self::Closure(_) => "<closure>".to_string(),
        };
        if let Some(node) = node {
            state.active.remove(&node);
        }
        rendered
    }

    pub(crate) fn native_value(&self) -> Option<NativeValue> {
        self.native_value_inner(&mut HashSet::new())
    }

    fn native_value_inner(&self, active: &mut HashSet<usize>) -> Option<NativeValue> {
        let node = vm_value_node_id(self);
        if let Some(node) = node
            && !active.insert(node)
        {
            return None;
        }
        let converted = match self {
            Self::Unit => Some(NativeValue::Unit),
            Self::Int(value) => Some(NativeValue::Int(*value)),
            Self::Float(value) => Some(NativeValue::Float(*value)),
            Self::Bool(value) => Some(NativeValue::Bool(*value)),
            Self::Char(value) => Some(NativeValue::Char(*value)),
            Self::Bytes(value) => Some(NativeValue::Bytes(value.as_ref().clone())),
            Self::String(value) => Some(NativeValue::String(value.to_string())),
            Self::Json(value) => Some(NativeValue::Json(value.as_ref().clone())),
            Self::List(values) => values
                .borrow()
                .iter()
                .map(|value| value.native_value_inner(active))
                .collect::<Option<Vec<_>>>()
                .map(NativeValue::List),
            // No `Deque` in the native ABI — a deque crosses the host boundary as
            // a list (the same shape the compiled backend produces).
            Self::Deque(values) => values
                .borrow()
                .iter()
                .map(|value| value.native_value_inner(active))
                .collect::<Option<Vec<_>>>()
                .map(NativeValue::List),
            Self::Map(entries) => entries
                .borrow()
                .iter()
                .map(|(key, value)| Some((key.native_value()?, value.native_value_inner(active)?)))
                .collect::<Option<Vec<_>>>()
                .map(NativeValue::Map),
            Self::Struct(data) => native_fields(data, active).map(|fields| NativeValue::Struct {
                name: data.name().to_string(),
                fields,
            }),
            Self::Variant(data) => native_fields(data, active).map(|fields| NativeValue::Variant {
                name: data.name().to_string(),
                fields,
            }),
            Self::Native(data) => Some(NativeValue::Native {
                type_name: data.type_name.to_string(),
                id: data.id,
            }),
            Self::Managed(value) => value.borrow().native_value_inner(active),
            Self::OptionSomeHeap(value) => {
                let mut fields = BTreeMap::new();
                fields.insert("value".to_string(), value.native_value_inner(active)?);
                Some(NativeValue::Variant {
                    name: "Some".to_string(),
                    fields,
                })
            }
            Self::OptionSomeScalar(value) => {
                let mut fields = BTreeMap::new();
                fields.insert(
                    "value".to_string(),
                    value.to_value().native_value_inner(active)?,
                );
                Some(NativeValue::Variant {
                    name: "Some".to_string(),
                    fields,
                })
            }
            Self::OptionNone => Some(NativeValue::Variant {
                name: "None".to_string(),
                fields: BTreeMap::new(),
            }),
            Self::Closure(_) => None,
        };
        if let Some(node) = node {
            active.remove(&node);
        }
        converted
    }
}

fn native_fields(
    data: &VmStruct,
    active: &mut HashSet<usize>,
) -> Option<BTreeMap<String, NativeValue>> {
    data.iter()
        .map(|(field, value)| Some((field.to_string(), value.native_value_inner(active)?)))
        .collect()
}

impl PartialEq for VmValue {
    fn eq(&self, other: &Self) -> bool {
        vm_values_equal(self, other, &mut HashSet::new())
    }
}

fn vm_values_equal(left: &VmValue, right: &VmValue, active: &mut HashSet<(usize, usize)>) -> bool {
    let pair = match (vm_value_node_id(left), vm_value_node_id(right)) {
        (Some(left), Some(right)) => {
            if left == right {
                return true;
            }
            let pair = (left, right);
            if !active.insert(pair) {
                return true;
            }
            Some(pair)
        }
        _ => None,
    };

    let equal = match (left, right) {
        (VmValue::Unit, VmValue::Unit) => true,
        (VmValue::Int(left), VmValue::Int(right)) => left == right,
        // Use IEEE `==` (not bitwise) so the interpreter matches the AOT
        // backend: `NaN == NaN` is false and `0.0 == -0.0` is true. Floats
        // are never used as map keys (see `VmMapKey`), so this does not affect
        // hashing.
        (VmValue::Float(left), VmValue::Float(right)) => left == right,
        (VmValue::Bool(left), VmValue::Bool(right)) => left == right,
        (VmValue::Char(left), VmValue::Char(right)) => left == right,
        (VmValue::Bytes(left), VmValue::Bytes(right)) => left == right,
        (VmValue::String(left), VmValue::String(right)) => left == right,
        (VmValue::Json(left), VmValue::Json(right)) => left == right,
        (VmValue::OptionSomeHeap(left), VmValue::OptionSomeHeap(right)) => {
            vm_values_equal(left, right, active)
        }
        (VmValue::OptionSomeScalar(left), VmValue::OptionSomeScalar(right)) => {
            // Compare via the materialized scalar so this reuses the exact
            // `VmValue` scalar equality (e.g. IEEE float `==`, `0.0 == -0.0`)
            // — identical to the old boxed `Some` comparison.
            left.to_value() == right.to_value()
        }
        // Canonical disjointness (V1.3): a scalar `Some` is *always* inline
        // and a non-scalar `Some` *always* heap, so an inline arm and a heap
        // arm can never represent the same logical value. They are therefore
        // unconditionally unequal — no cross-representation normalization.
        (VmValue::OptionSomeScalar(_), VmValue::OptionSomeHeap(_))
        | (VmValue::OptionSomeHeap(_), VmValue::OptionSomeScalar(_)) => false,
        (VmValue::OptionNone, VmValue::OptionNone) => true,
        (VmValue::List(left), VmValue::List(right)) => {
            let left = left.borrow();
            let right = right.borrow();
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| vm_values_equal(&left, &right, active))
        }
        (VmValue::Deque(left), VmValue::Deque(right)) => {
            let left = left.borrow();
            let right = right.borrow();
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| vm_values_equal(left, right, active))
        }
        (VmValue::Map(left), VmValue::Map(right)) => {
            let left = left.borrow();
            let right = right.borrow();
            left.len() == right.len()
                && left.iter().all(|(key, left_value)| {
                    right
                        .get(key)
                        .is_some_and(|right_value| vm_values_equal(left_value, right_value, active))
                })
        }
        (VmValue::Struct(left), VmValue::Struct(right))
        | (VmValue::Variant(left), VmValue::Variant(right)) => {
            left.name() == right.name()
                && left.fields.len() == right.fields.len()
                && left
                    .fields
                    .iter()
                    .zip(&right.fields)
                    .all(|(left, right)| vm_values_equal(left, right, active))
        }
        (VmValue::Native(left), VmValue::Native(right)) => {
            left.type_name == right.type_name && left.id == right.id
        }
        (VmValue::Managed(left), VmValue::Managed(right)) => {
            vm_values_equal(&left.borrow(), &right.borrow(), active)
        }
        (VmValue::Managed(left), right) => vm_values_equal(&left.borrow(), right, active),
        (left, VmValue::Managed(right)) => vm_values_equal(left, &right.borrow(), active),
        (VmValue::Closure(left), VmValue::Closure(right)) => Rc::ptr_eq(left, right),
        _ => false,
    };

    if let Some(pair) = pair {
        active.remove(&pair);
    }
    equal
}

impl Eq for VmValue {}

#[derive(Default)]
struct DisplayState {
    active: HashSet<usize>,
    ids: HashMap<usize, usize>,
}

impl DisplayState {
    fn id_for(&mut self, node: usize) -> usize {
        let next = self.ids.len() + 1;
        *self.ids.entry(node).or_insert(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_value_representation_stays_within_budget() {
        // V1 size budget (plan §3): inlining a `Scalar` (16B: an `f64`/`i64`
        // payload + a tag) into `OptionSomeScalar` widens `VmValue`, but it must
        // stay within the hard ≤ 24-byte cap so scalar-heavy code does not regress
        // on copy/cache traffic.
        let size = std::mem::size_of::<VmValue>();
        // V1 measured: 16 bytes — unchanged from pre-V1. The inline `Scalar`
        // (16B) fits within the layout the `Box`/`Rc` arms already imposed, so
        // inlining the scalar `Some` adds *no* size. Cap kept at 24 for V2 headroom.
        assert!(size <= 24, "VmValue grew to {size} bytes, budget is 24");
    }

    #[test]
    fn cyclic_values_have_bounded_display_and_equality() {
        fn self_cycle() -> VmValue {
            let cell = Rc::new(RefCell::new(VmValue::Unit));
            let managed = VmValue::Managed(Rc::clone(&cell));
            *cell.borrow_mut() = VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(vec![
                managed.clone(),
            ]))));
            managed
        }

        let left = self_cycle();
        let right = self_cycle();
        assert!(left.display().contains("<cycle#"));
        assert_eq!(left, right);
    }

    #[test]
    fn vm_struct_shrank_after_name_moved_to_layout() {
        // V2.0: the type name moved off each `VmStruct` instance onto the shared
        // interned `Rc<TypeLayout>`. The instance is now `{ layout: Rc<TypeLayout>
        // (8B), fields: Vec (24B) } = 32B`, down from the pre-V2.0
        // `{ name: Rc<str> (16B fat ptr), layout: Rc<StructLayout> (8B),
        // fields: Vec (24B) } = 48B`. `VmValue` itself is unchanged (Struct/Variant
        // still hold an `Rc<VmStruct>` = one 8B pointer), so the 16-byte `VmValue`
        // size is untouched (asserted above).
        let struct_size = std::mem::size_of::<VmStruct>();
        assert_eq!(struct_size, 32, "VmStruct expected 32 bytes after V2.0");
    }

    /// Hash a single value with a deterministic hasher to verify structural hash
    /// equivalence independent of the randomized outer map table.
    fn fnv_hash(value: &VmValue) -> u64 {
        let mut hasher = FnvHasher::default();
        hash_vm_value(value, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    }

    #[test]
    fn some_constructor_picks_canonical_representation() {
        // Scalars inline (no Box); non-scalars take the heap arm.
        assert!(matches!(
            VmValue::some(VmValue::Int(5)),
            VmValue::OptionSomeScalar(Scalar::Int(5))
        ));
        assert!(matches!(
            VmValue::some(VmValue::Unit),
            VmValue::OptionSomeScalar(Scalar::Unit)
        ));
        assert!(matches!(
            VmValue::some(VmValue::Bool(true)),
            VmValue::OptionSomeScalar(Scalar::Bool(true))
        ));
        assert!(matches!(
            VmValue::some(VmValue::string("x")),
            VmValue::OptionSomeHeap(_)
        ));
        assert!(matches!(
            VmValue::some(VmValue::List(Rc::new(RefCell::new(TypedVec::new())))),
            VmValue::OptionSomeHeap(_)
        ));
    }

    #[test]
    fn canonical_disjointness_inline_and_heap_never_alias() {
        // V1.3: prove an inline-scalar `Some` and a heap `Some` can never both
        // represent the same logical value — the constructor routes scalars to
        // inline and non-scalars to heap, so for *every* representable `Some(x)`
        // exactly one arm is produced. A hand-built heap `Some(Int(5))` is NOT a
        // value the canonical constructor can ever produce, so it can never equal
        // the inline `Some(Int(5))`.
        let inline = VmValue::some(VmValue::Int(5));
        let illegal_heap = VmValue::OptionSomeHeap(Box::new(VmValue::Int(5)));
        assert!(matches!(inline, VmValue::OptionSomeScalar(_)));
        // Disjoint arms compare unequal even with equal payloads.
        assert_ne!(inline, illegal_heap);
        // Equal scalar `Some`s (both inline) are equal and hash equally.
        let inline2 = VmValue::some(VmValue::Int(5));
        assert_eq!(inline, inline2);
        assert_eq!(fnv_hash(&inline), fnv_hash(&inline2));
    }

    #[test]
    fn inline_some_hashes_identically_to_old_boxed_some() {
        // THE HARD INVARIANT (V1.2b): `OptionSomeScalar(s)` must hash to the exact
        // same bytes as the pre-V1 `OptionSome(Box(scalar))` did. We reconstruct
        // the old representation as a heap `Some` carrying the *same* scalar and
        // assert the FNV hashes match bit-for-bit. (The heap arm keeps the old
        // `OptionSome` discriminant, so this heap hash *is* the pre-V1 hash.)
        for scalar in [
            VmValue::Int(5),
            VmValue::Int(-1),
            VmValue::Float(3.5),
            VmValue::Float(0.0),
            VmValue::Bool(true),
            VmValue::Char('z'),
            VmValue::Unit,
        ] {
            let inline = VmValue::some(scalar.clone());
            assert!(matches!(inline, VmValue::OptionSomeScalar(_)));
            let as_old_boxed = VmValue::OptionSomeHeap(Box::new(scalar.clone()));
            assert_eq!(
                fnv_hash(&inline),
                fnv_hash(&as_old_boxed),
                "inline Some({}) hash diverged from boxed form",
                scalar.display()
            );
        }
    }

    #[test]
    fn intern_layout_dedups_repeated_shapes() {
        // V2.0: the same `(name, field_names)` shape returns the *same* `Rc`
        // (a refcount bump), eliminating the per-construction `Rc::new` the old
        // `from_named` did. A different name or different fields is a distinct
        // layout.
        let a = intern_layout(Rc::from("Point"), vec![Rc::from("x"), Rc::from("y")]);
        let b = intern_layout(Rc::from("Point"), vec![Rc::from("x"), Rc::from("y")]);
        assert!(Rc::ptr_eq(&a, &b), "same shape must intern to one Rc");

        let c = intern_layout(Rc::from("Point"), vec![Rc::from("x")]);
        assert!(!Rc::ptr_eq(&a, &c), "different fields → different layout");
        let d = intern_layout(Rc::from("Other"), vec![Rc::from("x"), Rc::from("y")]);
        assert!(!Rc::ptr_eq(&a, &d), "different name → different layout");

        // The name rides on the shared layout (V2.0), and two `from_named`
        // constructions of one shape share the interned layout.
        let s1 = VmStruct::from_named("Point", [("x", VmValue::Int(1)), ("y", VmValue::Int(2))]);
        let s2 = VmStruct::from_named("Point", [("x", VmValue::Int(9)), ("y", VmValue::Int(8))]);
        assert_eq!(s1.name().as_ref(), "Point");
        assert!(
            Rc::ptr_eq(&s1.layout, &s2.layout),
            "two instances of one type must share the interned layout"
        );
    }

    #[test]
    fn struct_name_moved_to_layout_preserves_eq_hash_display() {
        // The name moved off the instance onto the shared layout (V2.0), but the
        // *bytes* hashed/compared/displayed are unchanged: hash mixes `name()` +
        // fields, eq compares `name()` + fields, display uses `name()`. Two equal
        // structs (built independently) stay equal, hash-equal, and display-equal.
        let a = VmStruct::from_named(
            "Pair",
            [("a", VmValue::Int(1)), ("b", VmValue::string("z"))],
        );
        let b = VmStruct::from_named(
            "Pair",
            [("a", VmValue::Int(1)), ("b", VmValue::string("z"))],
        );
        let va = VmValue::Struct(Rc::new(a));
        let vb = VmValue::Struct(Rc::new(b));
        assert_eq!(va, vb);
        assert_eq!(fnv_hash(&va), fnv_hash(&vb));
        assert_eq!(va.display(), vb.display());
        assert_eq!(va.display(), "Pair { a: 1, b: z }");

        // A variant hashes by its canonical name string, unchanged by the move.
        let ok = VmValue::Variant(Rc::new(VmStruct::from_named(
            "Ok",
            [("0", VmValue::Int(7))],
        )));
        assert_eq!(ok.display(), "Ok { 0: 7 }");
    }

    #[test]
    #[allow(clippy::mutable_key_type)]
    fn randomized_map_preserves_mixed_key_lookup() {
        // Iteration order is intentionally unspecified. This exercises the mixed
        // key representations through lookup instead of pinning a weak deterministic
        // hash-table order.
        let mut map: ValueMap = ValueMap::default();
        let keys = [
            VmValue::some(VmValue::Int(1)),
            VmValue::some(VmValue::Int(2)),
            VmValue::some(VmValue::Char('a')),
            VmValue::some(VmValue::Bool(false)),
            VmValue::some(VmValue::string("heap")),
            VmValue::OptionNone,
            VmValue::Int(7),
            VmValue::string("plain"),
        ];
        for (index, key) in keys.iter().enumerate() {
            map.insert(VmMapKey::new(key.clone()), VmValue::Int(index as i64));
        }
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(
                map.get(&VmMapKey::new(key.clone())),
                Some(&VmValue::Int(index as i64))
            );
        }
    }

    // ===== TV1 (Valhalla typed scalar list storage) =====================

    fn list_value(tv: TypedVec) -> VmValue {
        VmValue::List(Rc::new(RefCell::new(tv)))
    }

    #[test]
    fn vm_value_stays_sixteen_bytes_after_tv1() {
        // THE WHOLE POINT (plan §2): typed list kinds live *behind* the `List`
        // `Rc`, so `VmValue::List` is still one pointer and `VmValue` does not
        // grow. This is the opposite of value-rep V2.1, which widened the shared
        // box and taxed every value.
        assert_eq!(
            std::mem::size_of::<VmValue>(),
            16,
            "VmValue must stay 16 bytes — TV1 must not tax the shared value box"
        );
    }

    #[test]
    fn typed_and_boxed_lists_are_observably_identical() {
        // A `Floats`/`Ints` list and a `Boxed` list of the same logical values
        // must be byte-identically equal, hash-equal (Ints only — Float isn't
        // hashable), and display-equal. This is the parity guarantee.
        let ints_typed = list_value(TypedVec::Ints(vec![1, 2, 3]));
        let ints_boxed = list_value(TypedVec::Boxed(vec![
            VmValue::Int(1),
            VmValue::Int(2),
            VmValue::Int(3),
        ]));
        assert_eq!(ints_typed, ints_boxed, "Ints ≡ Boxed eq");
        assert_eq!(
            ints_typed.display(),
            ints_boxed.display(),
            "Ints ≡ Boxed display"
        );
        assert_eq!(
            fnv_hash(&ints_typed),
            fnv_hash(&ints_boxed),
            "Ints ≡ Boxed hash (List<Int> is a hashable map key)"
        );

        let floats_typed = list_value(TypedVec::Floats(vec![1.5, 2.5]));
        let floats_boxed = list_value(TypedVec::Boxed(vec![
            VmValue::Float(1.5),
            VmValue::Float(2.5),
        ]));
        assert_eq!(floats_typed, floats_boxed, "Floats ≡ Boxed eq");
        assert_eq!(
            floats_typed.display(),
            floats_boxed.display(),
            "Floats ≡ Boxed display"
        );
        // No hash assertion for Floats: `Float` is not hashable, so a `List<Float>`
        // can never be a map key — the hash path does not exist for it.
    }

    #[test]
    fn from_values_picks_the_right_kind() {
        // Construction specializes a homogeneous scalar run and falls back to
        // Boxed for mixed/non-scalar/empty.
        assert!(matches!(
            TypedVec::from_values(vec![VmValue::Int(1), VmValue::Int(2)]),
            TypedVec::Ints(_)
        ));
        assert!(matches!(
            TypedVec::from_values(vec![VmValue::Float(1.0)]),
            TypedVec::Floats(_)
        ));
        assert!(matches!(
            TypedVec::from_values(vec![VmValue::Int(1), VmValue::Float(2.0)]),
            TypedVec::Boxed(_)
        ));
        assert!(matches!(
            TypedVec::from_values(vec![VmValue::string("x")]),
            TypedVec::Boxed(_)
        ));
        // Empty is Boxed (no element to specialize on).
        assert!(matches!(TypedVec::from_values(vec![]), TypedVec::Boxed(_)));
        // ElemKind constructs an empty typed kind for a `List<Float>.new()`.
        assert!(
            matches!(ElemKind::from_type_name("Float").empty(), TypedVec::Floats(v) if v.is_empty())
        );
        assert!(
            matches!(ElemKind::from_type_name("Int").empty(), TypedVec::Ints(v) if v.is_empty())
        );
        assert!(matches!(
            ElemKind::from_type_name("String").empty(),
            TypedVec::Boxed(_)
        ));
    }

    #[test]
    fn empty_typed_list_specializes_on_first_push() {
        // The push-path specialization that makes `List<Int>.new(); push(...)`
        // kernels typed without construction metadata.
        let mut tv = TypedVec::Boxed(Vec::new());
        tv.push(VmValue::Int(7));
        assert!(matches!(tv, TypedVec::Ints(ref v) if v == &[7]));

        let mut tf = TypedVec::Boxed(Vec::new());
        tf.push(VmValue::Float(1.0));
        assert!(matches!(tf, TypedVec::Floats(_)));
    }

    #[test]
    fn construction_path_promotes_on_kind_mismatch() {
        // `from_values`/`push` (construction/producer paths) degrade gracefully:
        // a mismatched value promotes the whole list to Boxed, never panics, never
        // loses data.
        let mut tv = TypedVec::Ints(vec![1, 2]);
        tv.push(VmValue::Float(3.0));
        assert!(matches!(tv, TypedVec::Boxed(_)));
        assert_eq!(tv.len(), 3);
        assert_eq!(tv.get(2), Some(VmValue::Float(3.0)));
    }

    #[test]
    fn checked_mutation_errors_on_kind_mismatch() {
        // The VM opcode path (`ListPush`/`ListSet`) is TOTAL but loud: a kind
        // mismatch returns Err (a checker-contract violation) rather than promoting.
        let mut tv = TypedVec::Ints(vec![1, 2]);
        assert!(tv.checked_push(VmValue::Float(3.0)).is_err());
        // The list is unchanged after a rejected push.
        assert!(matches!(tv, TypedVec::Ints(ref v) if v == &[1, 2]));
        // A matching push succeeds.
        assert!(tv.checked_push(VmValue::Int(3)).is_ok());
        assert_eq!(tv.len(), 3);
        // checked_set likewise.
        assert!(tv.checked_set(0, VmValue::Float(9.0)).is_err());
        assert!(tv.checked_set(0, VmValue::Int(9)).is_ok());
        assert_eq!(tv.get(0), Some(VmValue::Int(9)));
    }

    #[test]
    fn allocation_budget_charges_per_kind() {
        // 8 B/elem for the flat scalar kinds, VmValue cost for Boxed (plan §4.6).
        assert_eq!(TypedVec::Ints(vec![1, 2, 3]).elem_bytes(), 8);
        assert_eq!(TypedVec::Floats(vec![1.0]).elem_bytes(), 8);
        assert_eq!(
            TypedVec::Boxed(vec![VmValue::Int(1)]).elem_bytes(),
            std::mem::size_of::<VmValue>()
        );
        // push_cost: an empty Boxed taking a scalar charges 8 B (it will specialize);
        // a Boxed taking a heap value charges the VmValue cost.
        assert_eq!(TypedVec::Ints(vec![]).push_cost(&VmValue::Int(1)), 8);
        assert_eq!(TypedVec::Boxed(vec![]).push_cost(&VmValue::Float(1.0)), 8);
        assert_eq!(
            TypedVec::Boxed(vec![]).push_cost(&VmValue::string("x")),
            std::mem::size_of::<VmValue>()
        );
    }

    #[test]
    fn transaction_snapshot_preserves_spare_capacity() {
        let mut ints = Vec::with_capacity(32);
        ints.extend([1, 2, 3]);
        let mut floats = Vec::with_capacity(24);
        floats.extend([1.0, 2.0]);
        let mut boxed = Vec::with_capacity(16);
        boxed.push(VmValue::Int(1));

        for value in [
            TypedVec::Ints(ints),
            TypedVec::Floats(floats),
            TypedVec::Boxed(boxed),
        ] {
            let snapshot = value.clone_preserving_capacity();
            match (&value, &snapshot) {
                (TypedVec::Ints(before), TypedVec::Ints(after)) => {
                    assert_eq!(after, before);
                    assert_eq!(after.capacity(), before.capacity());
                }
                (TypedVec::Floats(before), TypedVec::Floats(after)) => {
                    assert_eq!(after, before);
                    assert_eq!(after.capacity(), before.capacity());
                }
                (TypedVec::Boxed(before), TypedVec::Boxed(after)) => {
                    assert_eq!(after, before);
                    assert_eq!(after.capacity(), before.capacity());
                }
                _ => panic!("snapshot changed TypedVec representation"),
            }
        }
    }

    #[test]
    #[allow(clippy::mutable_key_type)]
    fn map_transaction_snapshot_preserves_spare_capacity() {
        let mut map = ValueMap::with_capacity_and_hasher(128, Default::default());
        map.insert(VmMapKey::new(VmValue::Int(1)), VmValue::Int(2));
        let snapshot = clone_value_map_preserving_capacity(&map);
        assert_eq!(snapshot, map);
        assert_eq!(snapshot.capacity(), map.capacity());
    }

    #[test]
    fn checked_push_accounted_amortizes_and_stays_conservative() {
        // TV2.1: the fused push charges only on capacity reallocation, and the
        // total charged over a build never *under*-counts the live length (it
        // over-counts by the unused capacity tail — conservative for the ceiling).
        let mut tv = TypedVec::Boxed(vec![]);
        let mut charged = 0usize;
        let n = 100;
        for i in 0..n {
            charged += tv.checked_push_accounted(VmValue::Int(i)).unwrap();
        }
        assert!(matches!(tv, TypedVec::Ints(_)));
        assert_eq!(tv.len(), n as usize);
        // Charged == capacity * 8, which is >= len * 8 (never under-counts).
        let cap = match &tv {
            TypedVec::Ints(v) => v.capacity(),
            _ => unreachable!(),
        };
        assert_eq!(charged, cap * 8);
        assert!(charged >= n as usize * 8);
        // Most pushes charge zero (amortized): far fewer charge-events than pushes.
        let mut events = 0;
        let mut tv2 = TypedVec::Boxed(vec![]);
        for i in 0..n {
            if tv2.checked_push_accounted(VmValue::Int(i)).unwrap() != 0 {
                events += 1;
            }
        }
        assert!(
            events < 20,
            "expected amortized O(log n) charge events, got {events}"
        );
        // Kind mismatch returns Err and charges nothing.
        let mut ints = TypedVec::Ints(vec![1]);
        assert!(ints.checked_push_accounted(VmValue::Float(1.0)).is_err());
        assert!(matches!(ints, TypedVec::Ints(ref v) if v == &[1]));
    }

    #[test]
    fn extend_accounted_bills_destination_layout() {
        let vv = std::mem::size_of::<VmValue>();
        // Mixed layout: a flat `Ints` source appended into a `Boxed` receiver lands
        // as 16 B `VmValue` slots. Accounting must follow the destination — the old
        // source-`elem_bytes` charge billed 8 B/elem and under-counted by 2x.
        let mut dst = TypedVec::Boxed(vec![VmValue::string("x")]); // heterogeneous -> stays Boxed
        let charged = dst.extend_accounted(TypedVec::Ints(vec![1, 2, 3, 4]));
        assert!(matches!(dst, TypedVec::Boxed(_)));
        assert_eq!(dst.len(), 5);
        assert!(
            charged >= 4 * vv,
            "boxed append must bill the VmValue slot ({vv} B), got {charged} for 4 elems"
        );
        // The buggy source-based charge would have been 4 * 8 = 32; we are well above it.
        assert!(charged > 4 * std::mem::size_of::<i64>());
        // Homogeneous flat-to-flat append stays on the 8 B scalar path and never
        // under-counts the live length.
        let mut ints = TypedVec::Ints(vec![1, 2, 3]);
        let grew = ints.extend_accounted(TypedVec::Ints(vec![4, 5, 6]));
        assert!(matches!(ints, TypedVec::Ints(_)));
        assert_eq!(ints.len(), 6);
        let cap = match &ints {
            TypedVec::Ints(v) => v.capacity(),
            _ => unreachable!(),
        };
        assert!(grew <= cap * 8 && ints.len() * 8 <= cap * 8);
    }

    #[test]
    #[allow(clippy::mutable_key_type)]
    fn typed_and_boxed_list_keys_share_hash_and_equality() {
        // `List<Int>` is a hashable map key. Iteration order is unspecified; the
        // semantic contract is that typed and boxed representations resolve the
        // same entries.
        let mut map: ValueMap = ValueMap::default();
        let keys = [
            list_value(TypedVec::Ints(vec![3, 1])),
            list_value(TypedVec::Ints(vec![1])),
            list_value(TypedVec::Ints(vec![2, 2, 2])),
            list_value(TypedVec::Ints(vec![])),
        ];
        for (index, key) in keys.iter().enumerate() {
            map.insert(VmMapKey::new(key.clone()), VmValue::Int(index as i64));
        }
        assert_eq!(map.len(), keys.len());

        // And a Boxed key of the same values resolves to the SAME entry (kind-
        // agnostic eq+hash), proving interchangeability.
        let boxed_key = VmMapKey::new(list_value(TypedVec::Boxed(vec![
            VmValue::Int(3),
            VmValue::Int(1),
        ])));
        assert_eq!(map.get(&boxed_key), Some(&VmValue::Int(0)));
    }
}
