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
/// indices into this function's instruction vector. Source and interpreter-resume
/// positions are carried independently by [`JitFunction::instruction_origins`].
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
    #[cfg(feature = "readonly-licm")]
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
    JumpIfIntCompare {
        lhs: u32,
        rhs: u32,
        op: JitCompare,
        expected: bool,
        target: u32,
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

/// Profitability category shared by the VM tiering policy and the backend.
/// We keep policy weights outside the IR, but the semantic opcode category lives
/// here so adding an instruction cannot silently miss one of several hand-written
/// capability tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitInstrCostClass {
    Neutral,
    ScalarAlu,
    LoadMove,
    Branch,
    ProfiledBranch,
    DirectList,
    MapMatch,
    CachedReadonlyHostCall,
    HostCall,
    ClosureIdentityHostCall,
    ClosureGuard,
    NativeCall,
}

/// Canonical backend capabilities of one JIT instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitInstrDescriptor {
    pub effects: JitInstrEffects,
    pub cost_class: JitInstrCostClass,
    pub native_leaf: bool,
    pub compact_scalar_frame: bool,
    pub step_batch_safe: bool,
    pub flat_list_direct: bool,
    pub(crate) required_host_helper: Option<HostHelper>,
}

impl JitInstr {
    /// Return the single capability/effect descriptor consumed by codegen,
    /// native-call admission, and VM profitability accounting.
    pub fn descriptor(&self) -> JitInstrDescriptor {
        use JitInstrCostClass as Cost;

        let flat_list_direct = matches!(
            self,
            Self::ListGetIntDirect { .. }
                | Self::ListSetIntDirect { .. }
                | Self::ListGetFloatDirect { .. }
                | Self::ListSetFloatDirect { .. }
                | Self::ListLenDirect { .. }
                | Self::ListIsEmptyDirect { .. }
        );

        let required_host_helper = match self {
            Self::HostCall { helper, .. } => Some(*helper),
            #[cfg(feature = "readonly-licm")]
            Self::MemoizedHostCall { helper, .. } => Some(*helper),
            Self::MatchMapGetInt { .. } => Some(HostHelper::MapGetMatchInt),
            Self::MatchMapGetFloat { .. } => Some(HostHelper::MapGetMatchFloat),
            Self::MatchSortedMapGetInt { .. } => Some(HostHelper::SortedMapGetInt),
            Self::MatchSortedMapGetFloat { .. } => Some(HostHelper::SortedMapGetFloat),
            _ => None,
        };

        let native_leaf = matches!(
            self,
            Self::Nop
                | Self::TailCallGuard { .. }
                | Self::LoadInt { .. }
                | Self::LoadFloat { .. }
                | Self::LoadBool { .. }
                | Self::Move { .. }
                | Self::Add { .. }
                | Self::Sub { .. }
                | Self::Mul { .. }
                | Self::Div { .. }
                | Self::Mod { .. }
                | Self::IntToFloat { .. }
                | Self::FloatToInt { .. }
                | Self::BitAnd { .. }
                | Self::BitOr { .. }
                | Self::BitXor { .. }
                | Self::Shl { .. }
                | Self::Shr { .. }
                | Self::Compare { .. }
                | Self::Equal { .. }
                | Self::NotEqual { .. }
                | Self::Jump { .. }
                | Self::JumpIfBool { .. }
                | Self::JumpIfIntCompare { .. }
                | Self::CallNative { .. }
                | Self::HostCall { .. }
                | Self::Return { .. }
                | Self::Bail
                | Self::ListGetIntDirect { .. }
                | Self::ListSetIntDirect { .. }
                | Self::ListGetFloatDirect { .. }
                | Self::ListSetFloatDirect { .. }
                | Self::ListLenDirect { .. }
                | Self::ListIsEmptyDirect { .. }
        );
        #[cfg(feature = "readonly-licm")]
        let native_leaf = native_leaf || matches!(self, Self::MemoizedHostCall { .. });
        let native_leaf = native_leaf
            && !matches!(
                self,
                Self::HostCall { helper, .. } if helper.heap_effect().extends_input_handles()
            );
        #[cfg(feature = "readonly-licm")]
        let native_leaf = native_leaf
            && !matches!(
                self,
                Self::MemoizedHostCall { helper, .. }
                    if helper.heap_effect().extends_input_handles()
            );

        let compact_scalar_frame =
            !matches!(self, Self::HostCall { .. } | Self::CallNative { .. }) && !flat_list_direct;
        #[cfg(feature = "readonly-licm")]
        let compact_scalar_frame =
            compact_scalar_frame && !matches!(self, Self::MemoizedHostCall { .. });

        let step_batch_safe = matches!(
            self,
            Self::Nop
                | Self::LoadInt { .. }
                | Self::LoadFloat { .. }
                | Self::LoadBool { .. }
                | Self::Move { .. }
                | Self::BitAnd { .. }
                | Self::BitOr { .. }
                | Self::BitXor { .. }
                | Self::Compare { .. }
                | Self::Equal { .. }
                | Self::NotEqual { .. }
                | Self::Jump { .. }
                | Self::JumpIfBool { .. }
                | Self::JumpIfIntCompare { .. }
                | Self::Return { .. }
        );

        let cost_class = match self {
            Self::Add { .. }
            | Self::Sub { .. }
            | Self::Mul { .. }
            | Self::Div { .. }
            | Self::Mod { .. }
            | Self::BitAnd { .. }
            | Self::BitOr { .. }
            | Self::BitXor { .. }
            | Self::Shl { .. }
            | Self::Shr { .. }
            | Self::Compare { .. }
            | Self::Equal { .. }
            | Self::NotEqual { .. } => Cost::ScalarAlu,
            Self::LoadInt { .. }
            | Self::LoadFloat { .. }
            | Self::LoadBool { .. }
            | Self::Move { .. }
            | Self::IntToFloat { .. }
            | Self::FloatToInt { .. } => Cost::LoadMove,
            Self::Jump { .. } | Self::JumpIfBool { .. } | Self::JumpIfIntCompare { .. } => {
                Cost::Branch
            }
            Self::ListGetIntDirect { .. }
            | Self::ListSetIntDirect { .. }
            | Self::ListGetFloatDirect { .. }
            | Self::ListSetFloatDirect { .. }
            | Self::ListLenDirect { .. }
            | Self::ListIsEmptyDirect { .. } => Cost::DirectList,
            Self::MatchMapGetInt { .. }
            | Self::MatchMapGetFloat { .. }
            | Self::MatchSortedMapGetInt { .. }
            | Self::MatchSortedMapGetFloat { .. } => Cost::MapMatch,
            #[cfg(feature = "readonly-licm")]
            Self::MemoizedHostCall { .. } => Cost::CachedReadonlyHostCall,
            Self::HostCall {
                helper: HostHelper::ClosureId,
                ..
            } => Cost::ClosureIdentityHostCall,
            Self::HostCall { .. } => Cost::HostCall,
            Self::CallNative { .. } => Cost::NativeCall,
            Self::Nop
            | Self::TailCallGuard { .. }
            | Self::Return { .. }
            | Self::Bail
            | Self::OsrExit
            | Self::RegionExit { .. } => Cost::Neutral,
        };

        JitInstrDescriptor {
            effects: self.effect_facts(),
            cost_class,
            native_leaf,
            compact_scalar_frame,
            step_batch_safe,
            flat_list_direct,
            required_host_helper,
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
            #[cfg(feature = "readonly-licm")]
            Self::MemoizedHostCall { dst, .. } => Some(*dst),
            Self::Nop
            | Self::TailCallGuard { .. }
            | Self::Jump { .. }
            | Self::JumpIfBool { .. }
            | Self::JumpIfIntCompare { .. }
            | Self::Return { .. }
            | Self::OsrExit
            | Self::RegionExit { .. }
            | Self::Bail => None,
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
            Self::HostCall { args, .. } => {
                for arg in args {
                    if let HostArg::Reg(reg) = arg {
                        visit(*reg);
                    }
                }
            }
            #[cfg(feature = "readonly-licm")]
            Self::MemoizedHostCall { args, .. } => {
                for arg in args {
                    if let HostArg::Reg(reg) = arg {
                        visit(*reg);
                    }
                }
            }
            Self::CallNative { args, .. } => args.iter().copied().for_each(visit),
            Self::MatchMapGetInt { map, key, .. }
            | Self::MatchMapGetFloat { map, key, .. }
            | Self::MatchSortedMapGetInt { map, key, .. }
            | Self::MatchSortedMapGetFloat { map, key, .. } => {
                visit(*map);
                visit(*key);
            }
            Self::JumpIfBool { cond, .. } => visit(*cond),
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
        }
    }

    /// Canonical effect classification. The exhaustive match is a deliberate
    /// architecture guard: adding an opcode without deciding its deopt, heap, and
    /// OSR semantics is a compile error.
    fn effect_facts(&self) -> JitInstrEffects {
        use JitControlFlow::{Conditional, Fallthrough, Jump, Split, Terminal};
        use JitHeapEffect::{None, Read, ReadWrite, Write};

        let control_flow = match self {
            Self::Jump { target } => Jump(*target),
            Self::JumpIfBool { target, .. } | Self::JumpIfIntCompare { target, .. } => {
                Conditional(*target)
            }
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
            #[cfg(feature = "readonly-licm")]
            Self::MemoizedHostCall { helper, .. } => match helper.heap_effect() {
                HostHeapEffect::ReadOnly | HostHeapEffect::ExtendsInputHandles => Read,
                HostHeapEffect::AllocatesResult => Write,
                HostHeapEffect::MutatesInput | HostHeapEffect::ReplacesInput => ReadWrite,
            },
            Self::CallNative { .. } => ReadWrite,
            Self::MatchMapGetInt { .. }
            | Self::MatchMapGetFloat { .. }
            | Self::MatchSortedMapGetInt { .. }
            | Self::MatchSortedMapGetFloat { .. }
            | Self::ListGetIntDirect { .. }
            | Self::ListGetFloatDirect { .. }
            | Self::ListLenDirect { .. }
            | Self::ListIsEmptyDirect { .. } => Read,
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
        #[cfg(feature = "readonly-licm")]
        let may_deopt = may_deopt || matches!(self, Self::MemoizedHostCall { .. });
        let osr_supported = true;
        JitInstrEffects {
            control_flow,
            heap,
            may_deopt,
            osr_supported,
        }
    }

    /// Canonical effect classification projected from the instruction descriptor.
    pub fn effects(&self) -> JitInstrEffects {
        self.descriptor().effects
    }

    /// Canonical membership test for the TV2 flat-array *direct* ops (read the raw
    /// param buffer / its `lens` slot with no host call). This is the SINGLE source
    /// of truth so the several classification sites (native-leaf eligibility, the
    /// cost model's direct-read credit, the simple-subset predicate) cannot drift —
    /// adding a new `*Direct` op only needs updating here. (Historically this set was
    /// hand-enumerated in each site and drifted: `ListSetFloatDirect`/`ListIsEmptyDirect`
    /// were missing from the native-leaf set.)
    pub fn is_flat_list_direct(&self) -> bool {
        self.descriptor().flat_list_direct
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
            #[cfg(feature = "readonly-licm")]
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

/// Stable source identity and accounting carried by one in-process JIT IR item.
///
/// JIT instruction indices are CFG identities only.  They must not be interpreted
/// as bytecode positions: rewrites may expand, fuse, or reorder native operations.
/// `source_cost` is the number of interpreter source steps owned by this item;
/// across a rewrite the total cost must be preserved and an expanded source
/// instruction assigns its cost to exactly one generated item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitInstructionOrigin {
    pub source_ip: u32,
    pub resume_ip: u32,
    pub source_cost: u32,
}

impl JitInstructionOrigin {
    pub const fn identity(ip: u32) -> Self {
        Self {
            source_ip: ip,
            resume_ip: ip,
            source_cost: 1,
        }
    }
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
    /// Per-instruction source/deopt/accounting identity. Production VM producers
    /// always populate this table. An empty table is retained only for detached
    /// lockstep clients and is interpreted as identity metadata.
    pub instruction_origins: Vec<JitInstructionOrigin>,
    /// Number of instructions in the source bytecode function. This bounds both
    /// source and interpreter-resume positions independently from `code.len()`.
    /// Zero is the detached-client compatibility value and means `code.len()`.
    pub source_instruction_count: u32,
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

    pub(crate) fn instruction_origin(&self, ip: usize) -> JitInstructionOrigin {
        self.instruction_origins
            .get(ip)
            .copied()
            .unwrap_or_else(|| {
                let mut origin = JitInstructionOrigin::identity(ip as u32);
                if matches!(
                    self.code.get(ip),
                    Some(JitInstr::RegionExit { .. } | JitInstr::OsrExit | JitInstr::Bail)
                ) {
                    origin.source_cost = 0;
                }
                origin
            })
    }

    pub fn source_instruction_count(&self) -> usize {
        if self.source_instruction_count == 0 {
            self.code.len()
        } else {
            self.source_instruction_count as usize
        }
    }

    /// Compact the register namespace according to `ordered_old_regs`.
    ///
    /// This is an in-process producer utility used by the VM's continuation
    /// lowerer. The first `n_live_in` compact slots become the OSR entry prefix;
    /// all remaining slots are region-local definitions. Instruction indices are
    /// deliberately unchanged so source/deopt origin maps stay stable.
    #[doc(hidden)]
    pub fn compact_registers(&mut self, ordered_old_regs: &[u32], n_live_in: u32) -> Option<()> {
        if usize::try_from(n_live_in).ok()? > ordered_old_regs.len() {
            return None;
        }
        let old_len = usize::try_from(self.n_regs).ok()?;
        if self.reg_types.len() != old_len {
            return None;
        }
        let mut old_to_new = vec![None; old_len];
        for (new, &old) in ordered_old_regs.iter().enumerate() {
            let old = usize::try_from(old).ok()?;
            if old >= old_len || old_to_new[old].is_some() {
                return None;
            }
            old_to_new[old] = Some(u32::try_from(new).ok()?);
        }
        for instr in &mut self.code {
            instr.remap_registers(&old_to_new)?;
        }
        for regs in &mut self.resume_live_regs {
            for reg in regs.iter_mut() {
                *reg = old_to_new.get(*reg as usize).copied().flatten()?;
            }
            regs.sort_unstable();
            regs.dedup();
        }
        for reg in &mut self.zero_init_regs {
            *reg = old_to_new.get(*reg as usize).copied().flatten()?;
        }
        self.zero_init_regs.sort_unstable();
        self.zero_init_regs.dedup();
        self.reg_types = ordered_old_regs
            .iter()
            .map(|old| self.reg_types.get(*old as usize).copied())
            .collect::<Option<Vec<_>>>()?;
        self.n_params = n_live_in;
        self.n_regs = u32::try_from(ordered_old_regs.len()).ok()?;
        Some(())
    }
}

impl JitInstr {
    fn remap_registers(&mut self, old_to_new: &[Option<u32>]) -> Option<()> {
        fn map(reg: &mut u32, old_to_new: &[Option<u32>]) -> Option<()> {
            *reg = old_to_new.get(*reg as usize).copied().flatten()?;
            Some(())
        }
        fn map_all(regs: &mut [u32], old_to_new: &[Option<u32>]) -> Option<()> {
            for reg in regs {
                map(reg, old_to_new)?;
            }
            Some(())
        }
        fn map_args(args: &mut [HostArg], old_to_new: &[Option<u32>]) -> Option<()> {
            for arg in args {
                if let HostArg::Reg(reg) = arg {
                    map(reg, old_to_new)?;
                }
            }
            Some(())
        }

        match self {
            Self::Nop
            | Self::TailCallGuard { .. }
            | Self::Jump { .. }
            | Self::Bail
            | Self::OsrExit => {}
            Self::LoadInt { dst, .. }
            | Self::LoadFloat { dst, .. }
            | Self::LoadBool { dst, .. } => map(dst, old_to_new)?,
            Self::Move { dst, src }
            | Self::IntToFloat { dst, src }
            | Self::FloatToInt { dst, src, .. } => {
                map(dst, old_to_new)?;
                map(src, old_to_new)?;
            }
            Self::Add { dst, lhs, rhs }
            | Self::Sub { dst, lhs, rhs }
            | Self::Mul { dst, lhs, rhs }
            | Self::Div { dst, lhs, rhs }
            | Self::Mod { dst, lhs, rhs }
            | Self::BitAnd { dst, lhs, rhs }
            | Self::BitOr { dst, lhs, rhs }
            | Self::BitXor { dst, lhs, rhs }
            | Self::Shl { dst, lhs, rhs }
            | Self::Shr { dst, lhs, rhs }
            | Self::Compare { dst, lhs, rhs, .. }
            | Self::Equal { dst, lhs, rhs }
            | Self::NotEqual { dst, lhs, rhs } => {
                map(dst, old_to_new)?;
                map(lhs, old_to_new)?;
                map(rhs, old_to_new)?;
            }
            Self::HostCall { dst, args, .. } => {
                map(dst, old_to_new)?;
                map_args(args, old_to_new)?;
            }
            #[cfg(feature = "readonly-licm")]
            Self::MemoizedHostCall { dst, args, .. } => {
                map(dst, old_to_new)?;
                map_args(args, old_to_new)?;
            }
            Self::CallNative { dst, args, .. } => {
                map(dst, old_to_new)?;
                map_all(args, old_to_new)?;
            }
            Self::MatchMapGetInt {
                map: base,
                key,
                value_dst,
                ..
            }
            | Self::MatchMapGetFloat {
                map: base,
                key,
                value_dst,
                ..
            }
            | Self::MatchSortedMapGetInt {
                map: base,
                key,
                value_dst,
                ..
            }
            | Self::MatchSortedMapGetFloat {
                map: base,
                key,
                value_dst,
                ..
            } => {
                map(base, old_to_new)?;
                map(key, old_to_new)?;
                map(value_dst, old_to_new)?;
            }
            Self::JumpIfBool { cond, .. } => map(cond, old_to_new)?,
            Self::JumpIfIntCompare { lhs, rhs, .. } => {
                map(lhs, old_to_new)?;
                map(rhs, old_to_new)?;
            }
            Self::Return { src } => map(src, old_to_new)?,
            Self::ListGetIntDirect { dst, base, index }
            | Self::ListGetFloatDirect { dst, base, index } => {
                map(dst, old_to_new)?;
                map(base, old_to_new)?;
                map(index, old_to_new)?;
            }
            Self::ListSetIntDirect {
                dst,
                base,
                index,
                value,
            }
            | Self::ListSetFloatDirect {
                dst,
                base,
                index,
                value,
            } => {
                map(dst, old_to_new)?;
                map(base, old_to_new)?;
                map(index, old_to_new)?;
                map(value, old_to_new)?;
            }
            Self::ListLenDirect { dst, base } | Self::ListIsEmptyDirect { dst, base } => {
                map(dst, old_to_new)?;
                map(base, old_to_new)?;
            }
            Self::RegionExit { live, .. } => map_all(live, old_to_new)?,
        }
        Some(())
    }
}
use super::*;
