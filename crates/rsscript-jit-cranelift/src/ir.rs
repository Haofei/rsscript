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
    pub(crate) fn cc(self) -> IntCC {
        match self {
            JitCompare::Lt => IntCC::SignedLessThan,
            JitCompare::Le => IntCC::SignedLessThanOrEqual,
            JitCompare::Gt => IntCC::SignedGreaterThan,
            JitCompare::Ge => IntCC::SignedGreaterThanOrEqual,
        }
    }

    /// Ordered float comparison (NaN → false), matching Rust's `<`/`<=`/`>`/`>=`
    /// on `f64` (the interpreter's float comparison).
    pub(crate) fn fcc(self) -> FloatCC {
        match self {
            JitCompare::Lt => FloatCC::LessThan,
            JitCompare::Le => FloatCC::LessThanOrEqual,
            JitCompare::Gt => FloatCC::GreaterThan,
            JitCompare::Ge => FloatCC::GreaterThanOrEqual,
        }
    }
}

/// Rounding mode applied to an f64 before a [`JitInstr::FloatToInt`] cast. Each
/// variant maps to exactly one interpreter Float→Int operation, so the native
/// result is bit-identical: `Floor` ↔ `Math.floor` (`f.floor() as i64`), `Ceil` ↔
/// `Math.ceil` (`f.ceil() as i64`). `Math.round` is *not* representable here —
/// Rust's `f64::round` rounds half away from zero, which Cranelift's `nearest`
/// (round half to even) does not match — so it stays an interpreter-only op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatRounding {
    Floor,
    Ceil,
}

/// One instruction of the JIT IR. Registers are `u32` indices; jump `target`s are
/// indices into the function's instruction vector (matching the VM's bytecode, so
/// translation is 1:1 and target indices need no remapping).
#[derive(Debug, Clone)]
pub enum JitInstr {
    /// Placeholder that preserves 1:1 index alignment with the source bytecode
    /// (e.g. a deep-copy of an `Int`, which is a no-op on an unboxed register).
    Nop,
    /// Count an optimized self-tail-call against the language's logical call-depth
    /// limit. The top-level caller supplies the current interpreter/native depth;
    /// exceeding `max_depth` takes an anonymous fallback so the interpreter replays
    /// from the function entry and produces the canonical depth-limit error.
    TailCallGuard {
        max_depth: u32,
    },
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
    /// `dst (Int) = round(src (Float)) as i64` — the inverse of [`IntToFloat`].
    /// Applies `rounding` to the f64 `src`, then a *saturating* signed cast to
    /// i64 (`fcvt_to_sint_sat`), matching the interpreter's `f.floor()/.ceil() as
    /// i64`: Rust's `as` saturates (NaN→0, +∞→i64::MAX, -∞→i64::MIN), and the
    /// rounded value is already integral so the cast is exact in range.
    FloatToInt {
        dst: u32,
        src: u32,
        rounding: FloatRounding,
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
    /// executed visit calls the helper and stores its result in codegen-private
    /// storage identified by `memo_slot`. Later visits copy that value into `dst`.
    /// Memo slots are a dense, function-local namespace, separate from public VM
    /// registers. This keeps the IR source-index aligned without changing register
    /// windows, definite assignment, or deopt payloads.
    #[cfg(feature = "memoization")]
    MemoizedHostCall {
        helper: HostHelper,
        dst: u32,
        args: Vec<HostArg>,
        memo_slot: u32,
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
    /// A self-recursive native call: invoke THIS same function (the one being
    /// compiled). Used because a `CallNative` callee id is not minted until `compile`
    /// returns, so a self-call cannot name its own id; `CallSelf` resolves to the
    /// function's own (declared-before-defined) `FuncId`. Semantically a
    /// `CallNative` to self, but its deopt is **non-chaining**: a self-recursive
    /// function is compiled with re-run-from-top deopt (its own `precise` is off), so a
    /// bail anywhere in the recursion unwinds to the interpreter and re-runs from the
    /// top — avoiding an unbounded deopt payload chain. An **entry depth guard** bails
    /// before the host C stack can overflow.
    #[cfg(feature = "recursion")]
    CallSelf {
        dst: u32,
        args: Vec<u32>,
    },
    /// A mutually-recursive native call to another member of the same
    /// co-compiled group (native-call-ABI slice 4): invoke group member
    /// `group_index` (an index into the `funcs` slice passed to
    /// [`NativeModule::compile_recursive_group`]). Like `CallSelf`, the callee's
    /// id is not minted until the whole cycle is declared, so the call names a
    /// group index rather than a `CompiledId`. Non-chaining (re-run-from-top deopt),
    /// and every group member carries the entry depth guard so a mutual-recursion
    /// cycle cannot overflow the host C stack.
    #[cfg(feature = "recursion")]
    CallGroup {
        group_index: u32,
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
    /// `Map.get` fused with an Option match for an Int-keyed `Map<_, Float>` — the
    /// Float value-side mirror of [`MatchMapGetInt`]. On `Some` it loads the Float
    /// payload (f64 channel) into the Float `value_dst` and jumps to `some_ip`.
    MatchMapGetFloat {
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
    /// `SortedMap.get` fused with an Option match for an Int-keyed sorted
    /// `Map<_, Float>` — the Float value-side mirror of [`MatchSortedMapGetInt`].
    MatchSortedMapGetFloat {
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
    #[cfg(feature = "speculation")]
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
    #[cfg(feature = "speculation")]
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
    /// TV2 direct write: `base_ptr[index] = value` where `base` is a mutable flat
    /// `List<Float>` param (write-side counterpart of [`ListGetFloatDirect`]).
    /// Bounds-checked against `lens[base]`; OOB bails and the VM-side heap
    /// transaction restores the list snapshot. `value` is a `Float` register; the
    /// 8-byte store is identical to the Int form (the f64 value var selects f64).
    ListSetFloatDirect {
        dst: u32,
        base: u32,
        index: u32,
        value: u32,
    },
    /// TV2 direct read: `dst = len` of the flat-array param `base` (a `FlatInt` or
    /// `FlatFloat` register). Reads the length from the param's `lens` slot — no
    /// host call. `dst` is an `Int` register.
    ListLenDirect {
        dst: u32,
        base: u32,
    },
    /// TV2 direct emptiness test: `dst = (len(base) == 0)` for the flat-array param
    /// `base` (a `FlatInt`/`FlatFloat` register). Reads the length from the param's
    /// `lens` slot and compares to zero — no host call. `dst` is a `Bool` register,
    /// represented as an i64 0/1 in generated code.
    /// The flat-list counterpart of the `List.is_empty` host helper.
    ListIsEmptyDirect {
        dst: u32,
        base: u32,
    },
    /// Profile-guided monomorphic inlining guard (profile-guided inlining). `base` is a `Handle`
    /// register holding a closure handle; reads its underlying function id via
    /// [`HostHelpers::closure_id`] and, if it differs from `expected`, **bails** to
    /// the interpreter (the existing re-run-from-top fallback). Emitted just before
    /// the inlined body of the profiled-monomorphic callee, so a different callee
    /// than the one speculated never runs native code. Writes no register; the
    /// matching (hot) path falls through with zero extra work beyond the compare.
    #[cfg(feature = "speculation")]
    GuardClosureId {
        base: u32,
        expected: i64,
    },
    /// OSR-exit (OSR). Marks the post-loop instruction at the loop's exit edge: a
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
    /// Planned normal exit from a continuation region. `exit_id` is interpreted
    /// only by the embedding VM. `live` is the verifier/planner-produced state map
    /// for that exit; unlike deopt it need not capture every assigned temporary.
    RegionExit {
        exit_id: u32,
        live: Vec<u32>,
    },
}

/// Canonical control-flow shape of one JIT instruction.
///
/// This is intentionally owned by the IR model rather than the verifier.  Any new
/// opcode must choose a shape in the exhaustive [`JitInstr::effects`] match, which
/// prevents reachability, liveness, and code-generation classifiers from silently
/// drifting apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitControlFlow {
    Fallthrough,
    Jump(u32),
    Conditional(u32),
    Split { first: u32, second: u32 },
    Terminal,
}

/// Heap-visible behavior of an instruction. Flat-buffer accesses are included:
/// although they do not use the VM heap table, they are externally visible memory
/// reads/writes and therefore belong in aliasing and rollback decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitHeapEffect {
    None,
    Read,
    Write,
    ReadWrite,
}

/// Stable, backend-neutral effect facts shared by validation and tiering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitInstrEffects {
    pub control_flow: JitControlFlow,
    pub heap: JitHeapEffect,
    pub may_deopt: bool,
    pub osr_supported: bool,
}

impl JitInstr {
    /// Imported runtime helper required by this instruction, if any. Codegen uses
    /// this single classification to declare only helpers reachable from the
    /// validated function; scalar-only fuzzing therefore needs no fabricated FFI
    /// function table.
    pub(crate) fn required_host_helper(&self) -> Option<HostHelper> {
        match self {
            Self::HostCall { helper, .. } => Some(*helper),
            #[cfg(feature = "memoization")]
            Self::MemoizedHostCall { helper, .. } => Some(*helper),
            Self::MatchMapGetInt { .. } => Some(HostHelper::MapGetMatchInt),
            Self::MatchMapGetFloat { .. } => Some(HostHelper::MapGetMatchFloat),
            Self::MatchSortedMapGetInt { .. } => Some(HostHelper::SortedMapGetInt),
            Self::MatchSortedMapGetFloat { .. } => Some(HostHelper::SortedMapGetFloat),
            #[cfg(feature = "speculation")]
            Self::GuardClosureId { .. } => Some(HostHelper::ClosureId),
            _ => None,
        }
    }

    /// Return the register definitely defined by this instruction, if any.
    pub fn defined_register(&self) -> Option<u32> {
        match self {
            Self::LoadInt { dst, .. }
            | Self::LoadFloat { dst, .. }
            | Self::LoadBool { dst, .. }
            | Self::Move { dst, .. }
            | Self::Add { dst, .. }
            | Self::Sub { dst, .. }
            | Self::Mul { dst, .. }
            | Self::Div { dst, .. }
            | Self::Mod { dst, .. }
            | Self::IntToFloat { dst, .. }
            | Self::FloatToInt { dst, .. }
            | Self::HostCall { dst, .. }
            | Self::CallNative { dst, .. }
            | Self::BitAnd { dst, .. }
            | Self::BitOr { dst, .. }
            | Self::BitXor { dst, .. }
            | Self::Shl { dst, .. }
            | Self::Shr { dst, .. }
            | Self::Compare { dst, .. }
            | Self::Equal { dst, .. }
            | Self::NotEqual { dst, .. }
            | Self::MatchMapGetInt { value_dst: dst, .. }
            | Self::MatchMapGetFloat { value_dst: dst, .. }
            | Self::MatchSortedMapGetInt { value_dst: dst, .. }
            | Self::MatchSortedMapGetFloat { value_dst: dst, .. }
            | Self::ListGetIntDirect { dst, .. }
            | Self::ListSetIntDirect { dst, .. }
            | Self::ListGetFloatDirect { dst, .. }
            | Self::ListSetFloatDirect { dst, .. }
            | Self::ListLenDirect { dst, .. }
            | Self::ListIsEmptyDirect { dst, .. } => Some(*dst),
            #[cfg(feature = "memoization")]
            Self::MemoizedHostCall { dst, .. } => Some(*dst),
            #[cfg(feature = "recursion")]
            Self::CallSelf { dst, .. } | Self::CallGroup { dst, .. } => Some(*dst),
            Self::Nop
            | Self::TailCallGuard { .. }
            | Self::Jump { .. }
            | Self::JumpIfBool { .. }
            | Self::JumpIfIntCompare { .. }
            | Self::Return { .. }
            | Self::OsrExit
            | Self::RegionExit { .. }
            | Self::Bail => None,
            #[cfg(feature = "speculation")]
            Self::ProfiledJumpIfBool { .. }
            | Self::ProfiledJumpIfIntCompare { .. }
            | Self::GuardClosureId { .. } => None,
        }
    }

    /// Visit every register whose current value is consumed by this instruction.
    pub fn visit_used_registers(&self, mut visit: impl FnMut(u32)) {
        match self {
            Self::Nop
            | Self::TailCallGuard { .. }
            | Self::LoadInt { .. }
            | Self::LoadFloat { .. }
            | Self::LoadBool { .. }
            | Self::Jump { .. }
            | Self::Bail
            | Self::OsrExit => {}
            Self::RegionExit { live, .. } => live.iter().copied().for_each(&mut visit),
            Self::Move { src, .. }
            | Self::IntToFloat { src, .. }
            | Self::FloatToInt { src, .. }
            | Self::Return { src } => visit(*src),
            Self::Add { lhs, rhs, .. }
            | Self::Sub { lhs, rhs, .. }
            | Self::Mul { lhs, rhs, .. }
            | Self::Div { lhs, rhs, .. }
            | Self::Mod { lhs, rhs, .. }
            | Self::BitAnd { lhs, rhs, .. }
            | Self::BitOr { lhs, rhs, .. }
            | Self::BitXor { lhs, rhs, .. }
            | Self::Shl { lhs, rhs, .. }
            | Self::Shr { lhs, rhs, .. }
            | Self::Compare { lhs, rhs, .. }
            | Self::Equal { lhs, rhs, .. }
            | Self::NotEqual { lhs, rhs, .. }
            | Self::JumpIfIntCompare { lhs, rhs, .. } => {
                visit(*lhs);
                visit(*rhs);
            }
            #[cfg(feature = "speculation")]
            Self::ProfiledJumpIfIntCompare { lhs, rhs, .. } => {
                visit(*lhs);
                visit(*rhs);
            }
            Self::HostCall { args, .. } => {
                for arg in args {
                    if let HostArg::Reg(reg) = arg {
                        visit(*reg);
                    }
                }
            }
            #[cfg(feature = "memoization")]
            Self::MemoizedHostCall { args, .. } => {
                for arg in args {
                    if let HostArg::Reg(reg) = arg {
                        visit(*reg);
                    }
                }
            }
            Self::CallNative { args, .. } => args.iter().copied().for_each(visit),
            #[cfg(feature = "recursion")]
            Self::CallSelf { args, .. } | Self::CallGroup { args, .. } => {
                args.iter().copied().for_each(visit)
            }
            Self::MatchMapGetInt { map, key, .. }
            | Self::MatchMapGetFloat { map, key, .. }
            | Self::MatchSortedMapGetInt { map, key, .. }
            | Self::MatchSortedMapGetFloat { map, key, .. } => {
                visit(*map);
                visit(*key);
            }
            Self::JumpIfBool { cond, .. } => visit(*cond),
            #[cfg(feature = "speculation")]
            Self::ProfiledJumpIfBool { cond, .. } => visit(*cond),
            Self::ListGetIntDirect { base, index, .. }
            | Self::ListGetFloatDirect { base, index, .. } => {
                visit(*base);
                visit(*index);
            }
            Self::ListSetIntDirect {
                base, index, value, ..
            }
            | Self::ListSetFloatDirect {
                base, index, value, ..
            } => {
                visit(*base);
                visit(*index);
                visit(*value);
            }
            Self::ListLenDirect { base, .. } | Self::ListIsEmptyDirect { base, .. } => visit(*base),
            #[cfg(feature = "speculation")]
            Self::GuardClosureId { base, .. } => visit(*base),
        }
    }

    /// Canonical effect classification. The exhaustive match is a deliberate
    /// architecture guard: adding an opcode without deciding its deopt, heap, and
    /// OSR semantics is a compile error.
    pub fn effects(&self) -> JitInstrEffects {
        use JitControlFlow::{Conditional, Fallthrough, Jump, Split, Terminal};
        use JitHeapEffect::{None, Read, ReadWrite, Write};

        let control_flow = match self {
            Self::Jump { target } => Jump(*target),
            Self::JumpIfBool { target, .. } | Self::JumpIfIntCompare { target, .. } => {
                Conditional(*target)
            }
            #[cfg(feature = "speculation")]
            Self::ProfiledJumpIfBool { target, .. }
            | Self::ProfiledJumpIfIntCompare { target, .. } => Conditional(*target),
            Self::MatchMapGetInt {
                some_ip, none_ip, ..
            }
            | Self::MatchMapGetFloat {
                some_ip, none_ip, ..
            }
            | Self::MatchSortedMapGetInt {
                some_ip, none_ip, ..
            }
            | Self::MatchSortedMapGetFloat {
                some_ip, none_ip, ..
            } => Split {
                first: *some_ip,
                second: *none_ip,
            },
            Self::Return { .. } | Self::Bail | Self::OsrExit | Self::RegionExit { .. } => Terminal,
            _ => Fallthrough,
        };
        let heap = match self {
            Self::HostCall { helper, .. } => match helper.heap_effect() {
                HostHeapEffect::ReadOnly | HostHeapEffect::ExtendsInputHandles => Read,
                HostHeapEffect::AllocatesResult => Write,
                HostHeapEffect::MutatesInput | HostHeapEffect::ReplacesInput => ReadWrite,
            },
            #[cfg(feature = "memoization")]
            Self::MemoizedHostCall { helper, .. } => match helper.heap_effect() {
                HostHeapEffect::ReadOnly | HostHeapEffect::ExtendsInputHandles => Read,
                HostHeapEffect::AllocatesResult => Write,
                HostHeapEffect::MutatesInput | HostHeapEffect::ReplacesInput => ReadWrite,
            },
            Self::CallNative { .. } => ReadWrite,
            #[cfg(feature = "recursion")]
            Self::CallSelf { .. } | Self::CallGroup { .. } => ReadWrite,
            Self::MatchMapGetInt { .. }
            | Self::MatchMapGetFloat { .. }
            | Self::MatchSortedMapGetInt { .. }
            | Self::MatchSortedMapGetFloat { .. }
            | Self::ListGetIntDirect { .. }
            | Self::ListGetFloatDirect { .. }
            | Self::ListLenDirect { .. }
            | Self::ListIsEmptyDirect { .. } => Read,
            #[cfg(feature = "speculation")]
            Self::GuardClosureId { .. } => Read,
            Self::ListSetIntDirect { .. } | Self::ListSetFloatDirect { .. } => Write,
            _ => None,
        };
        let may_deopt = matches!(
            self,
            Self::TailCallGuard { .. }
                | Self::Add { .. }
                | Self::Sub { .. }
                | Self::Mul { .. }
                | Self::Div { .. }
                | Self::Mod { .. }
                | Self::Shl { .. }
                | Self::Shr { .. }
                | Self::HostCall { .. }
                | Self::CallNative { .. }
                | Self::MatchMapGetInt { .. }
                | Self::MatchMapGetFloat { .. }
                | Self::MatchSortedMapGetInt { .. }
                | Self::MatchSortedMapGetFloat { .. }
                | Self::ListGetIntDirect { .. }
                | Self::ListSetIntDirect { .. }
                | Self::ListGetFloatDirect { .. }
                | Self::ListSetFloatDirect { .. }
                | Self::Bail
                | Self::OsrExit
        );
        #[cfg(feature = "memoization")]
        let may_deopt = may_deopt || matches!(self, Self::MemoizedHostCall { .. });
        #[cfg(feature = "recursion")]
        let may_deopt = may_deopt || matches!(self, Self::CallSelf { .. } | Self::CallGroup { .. });
        #[cfg(feature = "speculation")]
        let may_deopt = may_deopt
            || matches!(
                self,
                Self::ProfiledJumpIfBool { .. }
                    | Self::ProfiledJumpIfIntCompare { .. }
                    | Self::GuardClosureId { .. }
            );
        #[cfg(feature = "recursion")]
        let osr_supported = !matches!(self, Self::CallSelf { .. } | Self::CallGroup { .. });
        #[cfg(not(feature = "recursion"))]
        let osr_supported = true;
        JitInstrEffects {
            control_flow,
            heap,
            may_deopt,
            osr_supported,
        }
    }

    /// Canonical membership test for the TV2 flat-array *direct* ops (read the raw
    /// param buffer / its `lens` slot with no host call). This is the SINGLE source
    /// of truth so the several classification sites (native-leaf eligibility, the
    /// cost model's direct-read credit, the simple-subset predicate) cannot drift —
    /// adding a new `*Direct` op only needs updating here. (Historically this set was
    /// hand-enumerated in each site and drifted: `ListSetFloatDirect`/`ListIsEmptyDirect`
    /// were missing from the native-leaf set.)
    pub fn is_flat_list_direct(&self) -> bool {
        matches!(
            self,
            JitInstr::ListGetIntDirect { .. }
                | JitInstr::ListSetIntDirect { .. }
                | JitInstr::ListGetFloatDirect { .. }
                | JitInstr::ListSetFloatDirect { .. }
                | JitInstr::ListLenDirect { .. }
                | JitInstr::ListIsEmptyDirect { .. }
        )
    }

    /// Visit registers whose heap handles must be available when entering this
    /// instruction through OSR. Keeping this classification on the instruction
    /// model prevents VM tiering code from maintaining a second opcode list.
    pub fn visit_osr_heap_inputs(&self, mut visit: impl FnMut(u32)) {
        if matches!(
            self.effects().heap,
            JitHeapEffect::None | JitHeapEffect::Write
        ) {
            return;
        }
        match self {
            JitInstr::HostCall { args, .. } => {
                for arg in args {
                    if let HostArg::Reg(reg) = arg {
                        visit(*reg);
                    }
                }
            }
            #[cfg(feature = "memoization")]
            JitInstr::MemoizedHostCall { args, .. } => {
                for arg in args {
                    if let HostArg::Reg(reg) = arg {
                        visit(*reg);
                    }
                }
            }
            JitInstr::MatchMapGetInt { map, .. }
            | JitInstr::MatchMapGetFloat { map, .. }
            | JitInstr::MatchSortedMapGetInt { map, .. }
            | JitInstr::MatchSortedMapGetFloat { map, .. } => visit(*map),
            #[cfg(feature = "speculation")]
            JitInstr::GuardClosureId { base: map, .. } => visit(*map),
            _ => {}
        }
    }
}

/// Storage class of a register: an unboxed `i64` (integers and booleans) or an
/// unboxed `f64` (floats). The arithmetic/compare instructions are
/// type-polymorphic — the same `Add`/`Compare`/… opcode lowers to integer or
/// float machine ops depending on the operand registers' types (mirroring the
/// VM, where `AddInt` etc. dispatch on the runtime `VmValue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitValueType {
    Int,
    /// Logical boolean stored as an `i64` 0/1 in machine code.
    Bool,
    Float,
    /// An opaque handle (index into the VM's per-call heap-value table) to a heap
    /// value — a struct/list/etc. — that can't live in a scalar register. Stored
    /// as `i64`; only valid as the `base` of a heap-read instruction.
    Handle,
    /// TV2: a flat `List<Int>` param passed as a raw `*const i64` data pointer (in
    /// the args word) plus its element count (in the parallel `lens` word). Stored
    /// as `i64` (the pointer bits); only valid as the `base` of a `*Direct` read.
    FlatInt,
    /// Mutable counterpart of [`FlatInt`]. The machine representation is the same,
    /// but validation requires this type for direct writes.
    FlatIntMut,
    /// TV2: a flat `List<Float>` param passed as a raw `*const f64` data pointer
    /// plus its element count. Only valid as the `base` of a `*Direct` read.
    FlatFloat,
    /// Mutable counterpart of [`FlatFloat`].
    FlatFloatMut,
}

/// Activation boundary for one or more loop-local memo slots.
///
/// The half-open instruction range `[header, exit)` must be a structured loop:
/// outside control flow may enter only at `header`, control flow may leave only at
/// `exit`, and every backedge to `header` must be an unconditional [`JitInstr::Jump`].
/// Codegen resets `memo_slots` on function/OSR/forward entries to `header`, while
/// preserving them across those validated backedges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoScope {
    pub header: u32,
    pub exit: u32,
    pub memo_slots: Vec<u32>,
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
    /// Non-parameter scalar scratch registers whose entry value is defined as zero.
    /// Producers must list a register here when a transform intentionally needs a
    /// typed placeholder on a path where the logical value is absent.
    pub zero_init_regs: Vec<u32>,
    pub code: Vec<JitInstr>,
    /// Validated loop activation boundaries for every [`JitInstr::MemoizedHostCall`].
    /// Each memo slot must belong to exactly one scope.
    pub memo_scopes: Vec<MemoScope>,
    /// Instruction indices that should start cold blocks. Producers may populate
    /// this from dynamic profile feedback; codegen treats it as a layout hint only.
    /// It never changes control flow or values.
    pub cold_blocks: Vec<u32>,
    /// Optional source-resume liveness supplied by the verified-bytecode
    /// translator. When present it has one sorted register list per JIT
    /// instruction. Validation unions it with local JIT liveness and intersects it
    /// with definite assignment before codegen. Empty keeps the conservative
    /// all-assigned compatibility behavior used by detached JIT clients.
    pub resume_live_regs: Vec<Vec<u32>>,
}

impl JitFunction {
    pub(crate) fn is_float(&self, reg: u32) -> bool {
        self.reg_types[reg as usize] == JitValueType::Float
    }
}
use super::*;
