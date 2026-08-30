//! JIT-facing intrinsic semantics shared by translation and optimization passes.

use super::*;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IntrinsicEffect {
    /// No observable effect; result depends only on the (read-only) operands.
    Pure,
    /// Reads heap/host state but mutates nothing (e.g. a length query).
    Read,
    /// Allocates a fresh heap value from its operands; observes/mutates nothing
    /// else. This is the conservative DEFAULT.
    Allocate,
}

/// The role a foldable-string intrinsic plays in the string-length-fold pass: it is
/// either a *producer* of a string whose byte length the pass can compute from its
/// operands, or the length *query* itself. `None` ⇒ not part of that pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StringFoldRole {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BytesFoldRole {
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
#[derive(Debug, Clone, Copy)]
pub(super) struct IntrinsicDescriptor {
    /// Observable effect class (pure/read vs allocate/write/suspend).
    pub(super) effect: IntrinsicEffect,
    /// Inline numeric lowering arity, when this operation never crosses the host ABI.
    #[cfg(feature = "native-jit")]
    pub(super) native_inline_arity: Option<usize>,
    /// Typed host-helper lowering. Type-argument-dependent cases remain an explicit
    /// conservative exception in the registry projection.
    #[cfg(feature = "native-jit")]
    pub(super) native_host: Option<NativeHostIntrinsic>,
    /// If `Some`, this intrinsic is one of the six expandable Option/Result
    /// combinators, with its concrete lowering kind. The combinator-expansion pass
    /// uses this for *recognition*; it keeps the per-kind match/construct emission.
    pub(super) combinator_kind: Option<CombinatorKind>,
    /// If `Some`, this intrinsic participates in the string-length-fold pass in the
    /// given role. The pass uses this for *classification*; it keeps the exact length
    /// laws and the ASCII-only-slice bail.
    pub(super) string_fold_role: Option<StringFoldRole>,
    /// If `Some`, this intrinsic participates in the Bytes-length-fold pass in the
    /// given role (the Bytes sibling of `string_fold_role`). The pass uses this for
    /// *classification*; the exact byte-length laws stay in the pass. Bytes carry no
    /// char-boundary subtlety, so the slice law needs no ASCII gate.
    pub(super) bytes_fold_role: Option<BytesFoldRole>,
    /// Whether this intrinsic is a pure, re-runnable heap String *builder* that the
    /// deopt-before-heap cold-arm classifier permits inside a bailable cold arm (it
    /// allocates a fresh String from read-only operands and observes/mutates nothing
    /// else). A tight whitelist; impure intrinsics (I/O, env, collections, time, RNG)
    /// are excluded.
    pub(super) cold_arm_pure_builder: bool,
    /// Whether this intrinsic is a pure, first-order, side-effect-free *reader* that
    /// returns a SCALAR (Int/Bool) and that the deopt cold-arm classifier permits inside
    /// a bailable cold arm (e.g. `String.count`/`String.index_of`/`String.contains`):
    /// it reads its operands, allocates nothing, and is faithfully re-runnable on the
    /// interpreter after a native `Bail` (native never executes the arm). Distinct from
    /// `cold_arm_pure_builder` (which allocates a fresh heap value). MUST be first-order:
    /// a higher-order/closure-taking intrinsic (the `Pure` combinators) is NOT eligible
    /// because the closure can have arbitrary effects — those are excluded by leaving
    /// this `false`. A tight whitelist; when unsure, leave `false`.
    pub(super) cold_arm_pure_reader: bool,
    /// Short human-readable reason for the conservative classification, for the
    /// future missed-optimization report (e.g. "allocates", "suspends",
    /// "non-ASCII-dependent slice"). Empty for the trivial/expected cases.
    pub(super) notes: &'static str,
}

impl Default for IntrinsicDescriptor {
    /// The conservative default for the ~637 intrinsics that no site special-cases:
    /// treat as an opaque allocator that the JIT cannot fold or lower.
    fn default() -> Self {
        IntrinsicDescriptor {
            effect: IntrinsicEffect::Allocate,
            #[cfg(feature = "native-jit")]
            native_inline_arity: None,
            #[cfg(feature = "native-jit")]
            native_host: None,
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
pub(super) fn intrinsic_descriptor(intrinsic: RegIntrinsic) -> IntrinsicDescriptor {
    use IntrinsicEffect::*;
    let d = IntrinsicDescriptor::default;
    let descriptor = match intrinsic {
        // --- native_subset_instruction: native-lowerable intrinsics ---
        // `Int.to_float` lowers to a native signed-int→f64 conversion (the single-Int
        // -arg shape check stays at the call site).
        RegIntrinsic::IntToFloat => IntrinsicDescriptor {
            effect: Pure,
            notes: "native i64→f64 conversion (single Int arg)",
            ..d()
        },

        // --- Option/Result combinator expansion: the six expandable combinators ---
        RegIntrinsic::OptionMap => IntrinsicDescriptor {
            effect: Pure,
            combinator_kind: Some(CombinatorKind::OptionMap),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::OptionAndThen => IntrinsicDescriptor {
            effect: Pure,
            combinator_kind: Some(CombinatorKind::OptionAndThen),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::OptionUnwrapOr => IntrinsicDescriptor {
            effect: Pure,
            combinator_kind: Some(CombinatorKind::OptionUnwrapOr),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::ResultMap => IntrinsicDescriptor {
            effect: Pure,
            combinator_kind: Some(CombinatorKind::ResultMap),
            notes: "expandable pure Result combinator",
            ..d()
        },
        RegIntrinsic::ResultAndThen => IntrinsicDescriptor {
            effect: Pure,
            combinator_kind: Some(CombinatorKind::ResultAndThen),
            notes: "expandable pure Result combinator",
            ..d()
        },
        RegIntrinsic::ResultUnwrapOr => IntrinsicDescriptor {
            effect: Pure,
            combinator_kind: Some(CombinatorKind::ResultUnwrapOr),
            notes: "expandable pure Result combinator",
            ..d()
        },

        // --- string-length fold: the foldable string producers + the length query ---
        // `String.len` is a pure byte-length READ; the pass dissolves it to arithmetic.
        RegIntrinsic::StringLen => IntrinsicDescriptor {
            effect: Read,
            string_fold_role: Some(StringFoldRole::LengthQuery),
            notes: "byte-length query (foldable to arithmetic)",
            ..d()
        },
        // `String.from_int` allocates a fresh (always-ASCII) decimal string, but its
        // byte length is computable, so the length-fold pass can dissolve it; it is
        // also a whitelisted pure heap builder for deopt cold arms.
        RegIntrinsic::StringFromInt => IntrinsicDescriptor {
            effect: Allocate,
            string_fold_role: Some(StringFoldRole::ProducerFromInt),
            cold_arm_pure_builder: true,
            notes: "allocates ASCII decimal string; native-lowerable for final return",
            ..d()
        },
        // `String.slice` allocates a substring; foldable only when the source is
        // provably ASCII (the ASCII-gate stays in the pass).
        RegIntrinsic::StringSlice => IntrinsicDescriptor {
            effect: Allocate,
            string_fold_role: Some(StringFoldRole::ProducerSlice),
            cold_arm_pure_builder: true,
            notes: "allocates substring; native-lowerable and byte length foldable only when source is ASCII; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },
        RegIntrinsic::StringPadLeft => IntrinsicDescriptor {
            effect: Allocate,
            cold_arm_pure_builder: true,
            notes: "allocates padded string; native-lowerable as a typed host helper; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },
        RegIntrinsic::StringSplit => IntrinsicDescriptor {
            effect: Allocate,
            notes: "allocates List<String>; native-lowerable and split+len elidable",
            ..d()
        },
        RegIntrinsic::StringStartsWith => IntrinsicDescriptor {
            effect: Read,
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
            bytes_fold_role: Some(BytesFoldRole::LengthQuery),
            notes: "raw byte-length query (foldable to arithmetic; native-lowerable as a typed host helper)",
            ..d()
        },
        // `Bytes.from_string` allocates raw bytes from a String; its byte length is
        // exactly the source String's byte length (`as_bytes().len()`), so the Bytes
        // fold can dissolve it when the source length is known.
        RegIntrinsic::BytesFromString => IntrinsicDescriptor {
            effect: Allocate,
            bytes_fold_role: Some(BytesFoldRole::ProducerFromString),
            cold_arm_pure_builder: true,
            notes: "allocates raw bytes from String; byte length = source String byte length; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },
        // `Bytes.slice` allocates a byte-index substring; its length is the exact clamp
        // arithmetic of `bytes_slice` — NO ASCII gate (raw bytes have no char boundary).
        RegIntrinsic::BytesSlice => IntrinsicDescriptor {
            effect: Allocate,
            bytes_fold_role: Some(BytesFoldRole::ProducerSlice),
            cold_arm_pure_builder: true,
            notes: "allocates byte-index substring; native-lowerable and byte length foldable; pure builder (re-runnable after a cold-arm bail)",
            ..d()
        },

        // --- deopt cold-arm pure heap builders (cold_arm_pure_intrinsic) ---
        // These allocate a fresh String from read-only operands and observe/mutate
        // nothing else, so a native Bail can discard the arm and the interpreter
        // re-runs it faithfully.
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
    };

    #[cfg(feature = "native-jit")]
    let descriptor = {
        let mut descriptor = descriptor;
        descriptor.native_inline_arity = match intrinsic {
            RegIntrinsic::IntToFloat | RegIntrinsic::MathFloor | RegIntrinsic::MathCeil => Some(1),
            _ => None,
        };
        descriptor.native_host = match intrinsic {
            RegIntrinsic::StringFromInt => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::StringFromInt,
                result_ty: NativeTy::Handle,
            }),
            RegIntrinsic::StringLen => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::StringLen,
                result_ty: NativeTy::Int,
            }),
            RegIntrinsic::StringSlice => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::StringSlice,
                result_ty: NativeTy::Handle,
            }),
            RegIntrinsic::StringPadLeft => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::StringPadLeft,
                result_ty: NativeTy::Handle,
            }),
            RegIntrinsic::StringSplit => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::StringSplit,
                result_ty: NativeTy::Handle,
            }),
            RegIntrinsic::StringStartsWith => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::StringStartsWith,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::ListIsEmpty => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::ListIsEmpty,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::JsonParseOk => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::JsonParse,
                result_ty: NativeTy::Handle,
            }),
            RegIntrinsic::JsonFieldOk => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::JsonField,
                result_ty: NativeTy::Handle,
            }),
            RegIntrinsic::JsonFieldIntOk => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::JsonFieldInt,
                result_ty: NativeTy::Int,
            }),
            RegIntrinsic::BytesLen => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::BytesLen,
                result_ty: NativeTy::Int,
            }),
            RegIntrinsic::BytesSlice => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::BytesSlice,
                result_ty: NativeTy::Handle,
            }),
            RegIntrinsic::SetContains => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::MapContainsInt,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::MapIsEmpty => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::MapIsEmpty,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::MapLen => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::MapLen,
                result_ty: NativeTy::Int,
            }),
            RegIntrinsic::SetIsEmpty => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::SetIsEmpty,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::SetLen => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::SetLen,
                result_ty: NativeTy::Int,
            }),
            RegIntrinsic::SortedSetContains => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::SortedSetContainsInt,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::SortedSetIsEmpty => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::SortedSetIsEmpty,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::SortedSetLen => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::ListLen,
                result_ty: NativeTy::Int,
            }),
            RegIntrinsic::SortedMapContainsKey => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::SortedMapContainsKeyInt,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::SortedMapIsEmpty => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::SortedMapIsEmpty,
                result_ty: NativeTy::Bool,
            }),
            RegIntrinsic::SortedMapLen => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::SortedMapLen,
                result_ty: NativeTy::Int,
            }),
            RegIntrinsic::DequeLen => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::DequeLen,
                result_ty: NativeTy::Int,
            }),
            RegIntrinsic::DequeIsEmpty => Some(NativeHostIntrinsic {
                helper: vm_jit::HostHelper::DequeIsEmpty,
                result_ty: NativeTy::Bool,
            }),
            _ => None,
        };
        descriptor
    };
    descriptor
}

// Not `native-jit`-gated: the intrinsic descriptor table (always compiled, for the
// table unit test and lever-2) embeds `Option<CombinatorKind>`. Read by the
// `native-jit` combinator-expansion pass and the table unit test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CombinatorKind {
    OptionMap,
    OptionAndThen,
    OptionUnwrapOr,
    ResultMap,
    ResultAndThen,
    ResultUnwrapOr,
}
