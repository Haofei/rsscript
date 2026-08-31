use super::*;

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeFact<T: Copy + Eq> {
    Unreached,
    Unknown,
    Known(T),
}

#[cfg(feature = "native-jit")]
impl<T: Copy + Eq> NativeFact<T> {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (NativeFact::Unreached, value) | (value, NativeFact::Unreached) => value,
            (NativeFact::Unknown, NativeFact::Unknown) => NativeFact::Unknown,
            (NativeFact::Known(lhs), NativeFact::Known(rhs)) if lhs == rhs => {
                NativeFact::Known(lhs)
            }
            _ => NativeFact::Unknown,
        }
    }

    fn is_unreached(self) -> bool {
        matches!(self, NativeFact::Unreached)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum NativeHeapProvenance {
    External,
    Fresh(usize),
    Unknown,
}

pub(in crate::reg_vm) struct NativeHeapProvenanceFacts {
    cfg: NativeRegionCfg,
    before: Vec<Vec<NativeFact<NativeHeapProvenance>>>,
}

impl NativeHeapProvenanceFacts {
    // IPs intentionally index several parallel semantic fact tables.
    pub(in crate::reg_vm) fn compute(
        code: &[RegInstr],
        jit_code: &[vm_jit::JitInstr],
        n_params: usize,
        native_reg_types: &[NativeTy],
    ) -> Option<Self> {
        if code.len() != jit_code.len() || code.is_empty() {
            return None;
        }
        let cfg = NativeRegionCfg::prefix(code, code.len())?;
        let n_regs = native_reg_types.len();
        let mut before =
            vec![vec![NativeFact::Unreached; n_regs]; cfg.exit.saturating_sub(cfg.entry)];
        before[0] = native_reg_types
            .iter()
            .enumerate()
            .map(|(reg, ty)| {
                if reg < n_params && native_ty_is_heap_receiver(*ty) {
                    NativeFact::Known(NativeHeapProvenance::External)
                } else {
                    NativeFact::Unknown
                }
            })
            .collect();

        let mut changed = true;
        while changed {
            changed = false;
            for ip in parallel_indices(cfg.entry..cfg.exit) {
                let slot = ip - cfg.entry;
                if before[slot].iter().all(|fact| fact.is_unreached()) {
                    continue;
                }
                let mut out = before[slot].clone();
                native_heap_provenance_transfer(ip, &jit_code[ip], native_reg_types, &mut out);
                for &successor in &cfg.successors[slot] {
                    let successor_slot = successor - cfg.entry;
                    for (dst, incoming) in
                        before[successor_slot].iter_mut().zip(out.iter().copied())
                    {
                        let next = dst.merge(incoming);
                        if *dst != next {
                            *dst = next;
                            changed = true;
                        }
                    }
                }
            }
        }

        Some(Self { cfg, before })
    }

    pub(in crate::reg_vm) fn before(&self, reg: u32, ip: usize) -> NativeHeapProvenance {
        let Some(slot) = self.cfg.slot(ip) else {
            return NativeHeapProvenance::Unknown;
        };
        match self
            .before
            .get(slot)
            .and_then(|facts| facts.get(reg as usize))
            .copied()
        {
            Some(NativeFact::Known(provenance)) => provenance,
            Some(NativeFact::Unreached | NativeFact::Unknown) | None => {
                NativeHeapProvenance::Unknown
            }
        }
    }
}

fn native_ty_is_heap_receiver(ty: NativeTy) -> bool {
    matches!(
        ty,
        NativeTy::Handle
            | NativeTy::FlatInt
            | NativeTy::FlatIntMut
            | NativeTy::FlatFloat
            | NativeTy::FlatFloatMut
    )
}

fn native_heap_provenance_transfer(
    ip: usize,
    instr: &vm_jit::JitInstr,
    native_reg_types: &[NativeTy],
    facts: &mut [NativeFact<NativeHeapProvenance>],
) {
    let set = |facts: &mut [NativeFact<NativeHeapProvenance>],
               dst: u32,
               value: NativeFact<NativeHeapProvenance>| {
        if native_reg_types
            .get(dst as usize)
            .copied()
            .is_some_and(native_ty_is_heap_receiver)
            && let Some(slot) = facts.get_mut(dst as usize)
        {
            *slot = value;
        }
    };

    match instr {
        vm_jit::JitInstr::Move { dst, src } => {
            let value = facts
                .get(*src as usize)
                .copied()
                .unwrap_or(NativeFact::Unknown);
            set(facts, *dst, value);
        }
        vm_jit::JitInstr::HostCall {
            helper, dst, args, ..
        } => {
            let value = if helper.heap_effect().produces_heap_result() {
                if matches!(helper.heap_effect(), vm_jit::HostHeapEffect::ReplacesInput) {
                    match args.first() {
                        Some(vm_jit::HostArg::Reg(reg)) => facts
                            .get(*reg as usize)
                            .copied()
                            .unwrap_or(NativeFact::Unknown),
                        _ => NativeFact::Unknown,
                    }
                } else {
                    NativeFact::Known(NativeHeapProvenance::Fresh(ip))
                }
            } else {
                NativeFact::Unknown
            };
            set(facts, *dst, value);
        }
        vm_jit::JitInstr::MemoizedHostCall {
            helper, dst, args, ..
        } => {
            let value = if helper.heap_effect().produces_heap_result() {
                if matches!(helper.heap_effect(), vm_jit::HostHeapEffect::ReplacesInput) {
                    match args.first() {
                        Some(vm_jit::HostArg::Reg(reg)) => facts
                            .get(*reg as usize)
                            .copied()
                            .unwrap_or(NativeFact::Unknown),
                        _ => NativeFact::Unknown,
                    }
                } else {
                    NativeFact::Known(NativeHeapProvenance::Fresh(ip))
                }
            } else {
                NativeFact::Unknown
            };
            set(facts, *dst, value);
        }
        vm_jit::JitInstr::CallNative { dst, .. } => {
            set(facts, *dst, NativeFact::Unknown);
        }
        _ => {
            if let Some(dst) = native_jit_heap_fact_dst(instr) {
                set(facts, dst, NativeFact::Unknown);
            }
        }
    }
}

fn native_jit_heap_fact_dst(instr: &vm_jit::JitInstr) -> Option<u32> {
    match instr {
        vm_jit::JitInstr::LoadInt { dst, .. }
        | vm_jit::JitInstr::LoadFloat { dst, .. }
        | vm_jit::JitInstr::LoadBool { dst, .. }
        | vm_jit::JitInstr::Add { dst, .. }
        | vm_jit::JitInstr::Sub { dst, .. }
        | vm_jit::JitInstr::Mul { dst, .. }
        | vm_jit::JitInstr::Div { dst, .. }
        | vm_jit::JitInstr::Mod { dst, .. }
        | vm_jit::JitInstr::BitAnd { dst, .. }
        | vm_jit::JitInstr::BitOr { dst, .. }
        | vm_jit::JitInstr::BitXor { dst, .. }
        | vm_jit::JitInstr::Shl { dst, .. }
        | vm_jit::JitInstr::Shr { dst, .. }
        | vm_jit::JitInstr::Compare { dst, .. }
        | vm_jit::JitInstr::Equal { dst, .. }
        | vm_jit::JitInstr::NotEqual { dst, .. }
        | vm_jit::JitInstr::IntToFloat { dst, .. }
        | vm_jit::JitInstr::FloatToInt { dst, .. }
        | vm_jit::JitInstr::ListGetIntDirect { dst, .. }
        | vm_jit::JitInstr::ListSetIntDirect { dst, .. }
        | vm_jit::JitInstr::ListGetFloatDirect { dst, .. }
        | vm_jit::JitInstr::ListSetFloatDirect { dst, .. }
        | vm_jit::JitInstr::ListLenDirect { dst, .. }
        | vm_jit::JitInstr::ListIsEmptyDirect { dst, .. }
        | vm_jit::JitInstr::MatchMapGetInt { value_dst: dst, .. }
        | vm_jit::JitInstr::MatchMapGetFloat { value_dst: dst, .. }
        | vm_jit::JitInstr::MatchSortedMapGetInt { value_dst: dst, .. }
        | vm_jit::JitInstr::MatchSortedMapGetFloat { value_dst: dst, .. } => Some(*dst),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum NativeControlFlow {
    #[default]
    Fallthrough,
    Jump(usize),
    Branch {
        target: usize,
    },
    Split {
        first: usize,
        second: usize,
    },
    Terminal,
}

#[cfg(feature = "native-jit")]
impl NativeControlFlow {
    pub(super) fn successors(self, ip: usize, code_len: usize, mut push: impl FnMut(usize)) {
        let fallthrough = ip + 1;
        match self {
            NativeControlFlow::Fallthrough => {
                if fallthrough < code_len {
                    push(fallthrough);
                }
            }
            NativeControlFlow::Jump(target) => push(target),
            NativeControlFlow::Branch { target } => {
                push(target);
                if fallthrough < code_len {
                    push(fallthrough);
                }
            }
            NativeControlFlow::Split { first, second } => {
                push(first);
                push(second);
            }
            NativeControlFlow::Terminal => {}
        }
    }

    pub(super) fn is_boundary(self) -> bool {
        !matches!(self, NativeControlFlow::Fallthrough)
    }
}

/// A conservative register footprint: either an EXACT set of register operands, or
/// `All` meaning "could touch every register" (used for instruction variants we do
/// not fully model, so an omission is sound — it over-approximates liveness and can
/// only cause more OSR bails, never a missed escape).
#[cfg(feature = "native-jit")]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(in crate::reg_vm) enum RegFootprint {
    Some(Vec<usize>),
    #[default]
    All,
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_instr_successors(
    instr: &RegInstr,
    ip: usize,
    code_len: usize,
    push: impl FnMut(usize),
) {
    native_instr_semantics(instr)
        .control
        .successors(ip, code_len, push);
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_instr_is_control_boundary(instr: &RegInstr) -> bool {
    native_instr_semantics(instr).control.is_boundary()
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct NativeRegionCfg {
    entry: usize,
    exit: usize,
    successors: Vec<Vec<usize>>,
}

#[cfg(feature = "native-jit")]
impl NativeRegionCfg {
    pub(in crate::reg_vm) fn new(code: &[RegInstr], entry: usize, exit: usize) -> Option<Self> {
        if entry >= exit || exit > code.len() {
            return None;
        }
        let mut successors = vec![Vec::new(); exit - entry];
        for ip in parallel_indices(entry..exit) {
            native_instr_successors(&code[ip], ip, code.len(), |target| {
                if target >= entry && target < exit {
                    successors[ip - entry].push(target);
                }
            });
            successors[ip - entry].sort_unstable();
            successors[ip - entry].dedup();
        }
        Some(Self {
            entry,
            exit,
            successors,
        })
    }

    pub(in crate::reg_vm) fn prefix(code: &[RegInstr], exit: usize) -> Option<Self> {
        Self::new(code, 0, exit)
    }

    fn contains(&self, ip: usize) -> bool {
        ip >= self.entry && ip < self.exit
    }

    fn slot(&self, ip: usize) -> Option<usize> {
        self.contains(ip).then_some(ip - self.entry)
    }

    pub(in crate::reg_vm) fn successors(&self, ip: usize) -> Option<&[usize]> {
        self.slot(ip)
            .and_then(|slot| self.successors.get(slot).map(Vec::as_slice))
    }

    #[cfg(any(test, feature = "jit-diagnostics"))]
    pub(in crate::reg_vm) fn backedges(&self) -> Vec<(usize, usize)> {
        let mut backedges = Vec::new();
        for ip in parallel_indices(self.entry..self.exit) {
            let Some(successors) = self.successors(ip) else {
                continue;
            };
            for &target in successors {
                if target <= ip {
                    backedges.push((ip, target));
                }
            }
        }
        backedges
    }

    fn reachable_ips(&self) -> Vec<usize> {
        let mut reachable = vec![false; self.exit - self.entry];
        let mut stack = vec![self.entry];
        let mut out = Vec::new();
        while let Some(ip) = stack.pop() {
            let Some(slot) = self.slot(ip) else {
                continue;
            };
            if reachable[slot] {
                continue;
            }
            reachable[slot] = true;
            out.push(ip);
            for &successor in &self.successors[slot] {
                stack.push(successor);
            }
        }
        out
    }

    pub(super) fn reachable_mask(&self) -> Vec<bool> {
        let mut reachable = vec![false; self.exit - self.entry];
        for ip in self.reachable_ips() {
            if let Some(slot) = self.slot(ip) {
                reachable[slot] = true;
            }
        }
        reachable
    }
}

#[cfg(feature = "native-jit")]
fn native_clear_defined_fact<T: Copy + Eq>(instr: &RegInstr, facts: &mut [NativeFact<T>]) {
    if let Some(dst) = native_subset_dst(instr) {
        if let Some(fact) = facts.get_mut(dst) {
            *fact = NativeFact::Unknown;
        }
    } else if let RegInstr::CallIntrinsic { dst, .. } | RegInstr::CallTypedIntrinsic { dst, .. } =
        instr
        && let Some(fact) = facts.get_mut(*dst)
    {
        *fact = NativeFact::Unknown;
    }
}

#[cfg(feature = "native-jit")]
// The instruction index is also the fact-table coordinate.
fn native_forward_reg_facts<T: Copy + Eq>(
    cfg: &NativeRegionCfg,
    code: &[RegInstr],
    n_regs: usize,
    mut transfer: impl FnMut(usize, &RegInstr, &mut [NativeFact<T>]),
) -> Vec<Vec<NativeFact<T>>> {
    let mut input = vec![vec![NativeFact::Unreached; n_regs]; cfg.exit - cfg.entry];
    input[0] = vec![NativeFact::Unknown; n_regs];

    let mut changed = true;
    while changed {
        changed = false;
        for ip in parallel_indices(cfg.entry..cfg.exit) {
            let slot = ip - cfg.entry;
            if input[slot].iter().all(|fact| fact.is_unreached()) {
                continue;
            }

            let mut out = input[slot].clone();
            transfer(ip, &code[ip], &mut out);

            for successor in &cfg.successors[slot] {
                let successor_slot = successor - cfg.entry;
                let mut merged = input[successor_slot].clone();
                let mut merged_changed = false;
                for (dst, incoming) in merged.iter_mut().zip(out.iter().copied()) {
                    let next = dst.merge(incoming);
                    if *dst != next {
                        *dst = next;
                        merged_changed = true;
                    }
                }
                if merged_changed {
                    input[successor_slot] = merged;
                    changed = true;
                }
            }
        }
    }

    input
}

#[cfg(feature = "native-jit")]
fn native_forward_definite_regs(
    cfg: &NativeRegionCfg,
    code: &[RegInstr],
    n_regs: usize,
    mut transfer: impl FnMut(usize, &RegInstr, &mut [bool]) -> Option<()>,
) -> Option<Vec<Option<Vec<bool>>>> {
    let mut input = vec![None; cfg.exit - cfg.entry];
    input[0] = Some(vec![false; n_regs]);
    let mut worklist = vec![cfg.entry];

    while let Some(ip) = worklist.pop() {
        let Some(mut assigned) = input[ip - cfg.entry].clone() else {
            continue;
        };
        transfer(ip, &code[ip], &mut assigned)?;

        let Some(successors) = cfg.successors(ip) else {
            continue;
        };
        for &successor in successors {
            let successor_slot = successor - cfg.entry;
            let changed = match &mut input[successor_slot] {
                Some(existing) => {
                    let mut changed = false;
                    for (dst, src) in existing.iter_mut().zip(assigned.iter().copied()) {
                        let merged = *dst && src;
                        if *dst != merged {
                            *dst = merged;
                            changed = true;
                        }
                    }
                    changed
                }
                slot @ None => {
                    *slot = Some(assigned.clone());
                    true
                }
            };
            if changed {
                worklist.push(successor);
            }
        }
    }

    Some(input)
}

#[cfg(feature = "native-jit")]
struct NativeRegionLiveness {
    cfg: NativeRegionCfg,
    live_in: Vec<Vec<bool>>,
    live_out: Vec<Vec<bool>>,
}

#[cfg(feature = "native-jit")]
impl NativeRegionLiveness {
    fn compute_with_cfg(code: &[RegInstr], n_regs: usize, cfg: NativeRegionCfg) -> Self {
        let len = cfg.exit - cfg.entry;
        let mut live_in = vec![vec![false; n_regs]; len];
        let mut live_out = vec![vec![false; n_regs]; len];

        let mut changed = true;
        while changed {
            changed = false;
            for ip in parallel_indices((cfg.entry..cfg.exit).rev()) {
                let slot = ip - cfg.entry;

                let mut next_out = vec![false; n_regs];
                for &successor in &cfg.successors[slot] {
                    let successor_slot = successor - cfg.entry;
                    for (dst, src) in next_out
                        .iter_mut()
                        .zip(live_in[successor_slot].iter().copied())
                    {
                        *dst |= src;
                    }
                }

                let mut next_in = next_out.clone();
                native_liveness_transfer(&code[ip], n_regs, &mut next_in);

                if live_out[slot] != next_out {
                    live_out[slot] = next_out;
                    changed = true;
                }
                if live_in[slot] != next_in {
                    live_in[slot] = next_in;
                    changed = true;
                }
            }
        }

        Self {
            cfg,
            live_in,
            live_out,
        }
    }

    fn live_in(&self, ip: usize, reg: usize) -> Option<bool> {
        let slot = self.cfg.slot(ip)?;
        self.live_in.get(slot)?.get(reg).copied()
    }

    fn live_out(&self, ip: usize, reg: usize) -> Option<bool> {
        let slot = self.cfg.slot(ip)?;
        self.live_out.get(slot)?.get(reg).copied()
    }
}

#[cfg(feature = "native-jit")]
fn native_liveness_transfer(instr: &RegInstr, n_regs: usize, live: &mut [bool]) {
    match instr_written_reg(instr) {
        RegFootprint::Some(writes) => {
            for reg in writes {
                if reg < n_regs {
                    live[reg] = false;
                }
            }
        }
        RegFootprint::All => live.fill(false),
    }

    match instr_read_regs(instr) {
        RegFootprint::Some(reads) => {
            for reg in reads {
                if reg < n_regs {
                    live[reg] = true;
                }
            }
        }
        RegFootprint::All => live.fill(true),
    }
}

#[cfg(feature = "native-jit")]
fn native_global_def_counts(code: &[RegInstr], n_regs: usize) -> Option<Vec<usize>> {
    // The whole program is just the region spanning the full instruction range.
    native_region_def_counts(code, n_regs, 0, code.len())
}

/// Per-register definition counts over `code[header..exit]`. `None` if any
/// instruction in the range has an unbounded write footprint (`RegFootprint::All`),
/// since then no per-register count is knowable. Shared by the global and
/// region-scoped callers (a region transform's def-use building block).
#[cfg(feature = "native-jit")]
fn native_region_def_counts(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<Vec<usize>> {
    let mut counts = vec![0usize; n_regs];
    for instr in &code[header..exit] {
        match instr_written_reg(instr) {
            RegFootprint::Some(writes) => {
                for reg in writes {
                    if reg < n_regs {
                        counts[reg] += 1;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    Some(counts)
}

#[cfg(feature = "native-jit")]
struct NativeRegionValueFacts {
    cfg: NativeRegionCfg,
    const_int: Vec<Vec<NativeFact<i64>>>,
    list_len_source: Vec<Vec<NativeFact<(usize, usize)>>>,
}

#[cfg(feature = "native-jit")]
impl NativeRegionValueFacts {
    fn compute_with_cfg(code: &[RegInstr], n_regs: usize, cfg: NativeRegionCfg) -> Self {
        let const_int =
            native_forward_reg_facts(&cfg, code, n_regs, |_ip, instr, values| match instr {
                RegInstr::LoadInt { dst, value } if *dst < values.len() => {
                    values[*dst] = NativeFact::Known(*value);
                }
                RegInstr::Move { dst, src } if *dst < values.len() && *src < values.len() => {
                    values[*dst] = values[*src];
                }
                _ => native_clear_defined_fact(instr, values),
            });

        let list_len_source =
            native_forward_reg_facts(&cfg, code, n_regs, |ip, instr, sources| match instr {
                RegInstr::ListLen { dst, list } if *dst < sources.len() => {
                    sources[*dst] = NativeFact::Known((*list, ip));
                }
                RegInstr::Move { dst, src } if *dst < sources.len() && *src < sources.len() => {
                    sources[*dst] = sources[*src];
                }
                _ => native_clear_defined_fact(instr, sources),
            });

        Self {
            cfg,
            const_int,
            list_len_source,
        }
    }

    fn const_int_before(&self, reg: usize, ip: usize) -> Option<i64> {
        let slot = self.cfg.slot(ip)?;
        match *self.const_int.get(slot)?.get(reg)? {
            NativeFact::Known(value) => Some(value),
            NativeFact::Unreached | NativeFact::Unknown => None,
        }
    }

    fn list_len_source_before(&self, reg: usize, ip: usize) -> Option<(usize, usize)> {
        let slot = self.cfg.slot(ip)?;
        match *self.list_len_source.get(slot)?.get(reg)? {
            NativeFact::Known(value) => Some(value),
            NativeFact::Unreached | NativeFact::Unknown => None,
        }
    }
}

#[cfg(feature = "native-jit")]
struct NativeRegionEffects {
    roots_before: Vec<Vec<usize>>,
    list_writes: Vec<(usize, usize)>,
    move_alias_roots: Vec<usize>,
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct NativeRegionAnalysis {
    n_regs: usize,
    header: usize,
    exit: usize,
    values: NativeRegionValueFacts,
    liveness: NativeRegionLiveness,
    effects: NativeRegionEffects,
    pub(super) global_def_counts: Option<Vec<usize>>,
    region_def_counts: Option<Vec<usize>>,
}

#[cfg(feature = "native-jit")]
#[derive(Default)]
pub(in crate::reg_vm) struct NativeProfileGuidance {
    pub(in crate::reg_vm) cold_blocks: Vec<u32>,
    pub(in crate::reg_vm) hot_branch_edges: std::collections::HashMap<usize, bool>,
}

#[cfg(feature = "native-jit")]
impl NativeRegionAnalysis {
    pub(in crate::reg_vm) fn compute_prefix(
        code: &[RegInstr],
        n_regs: usize,
        header: usize,
        exit: usize,
    ) -> Option<Self> {
        if header >= exit || exit > code.len() {
            return None;
        }
        let cfg = NativeRegionCfg::prefix(code, exit)?;
        Some(Self {
            n_regs,
            header,
            exit,
            liveness: NativeRegionLiveness::compute_with_cfg(code, n_regs, cfg.clone()),
            values: NativeRegionValueFacts::compute_with_cfg(code, n_regs, cfg),
            effects: NativeRegionEffects::compute(code, n_regs, exit),
            global_def_counts: native_global_def_counts(code, n_regs),
            region_def_counts: native_region_def_counts(code, n_regs, header, exit),
        })
    }

    pub(in crate::reg_vm) fn compute_region(
        code: &[RegInstr],
        n_regs: usize,
        header: usize,
        exit: usize,
    ) -> Option<Self> {
        if header >= exit || exit > code.len() {
            return None;
        }
        let cfg = NativeRegionCfg::new(code, header, exit)?;
        Some(Self {
            n_regs,
            header,
            exit,
            liveness: NativeRegionLiveness::compute_with_cfg(code, n_regs, cfg.clone()),
            values: NativeRegionValueFacts::compute_with_cfg(code, n_regs, cfg),
            effects: NativeRegionEffects::compute(code, n_regs, exit),
            global_def_counts: native_global_def_counts(code, n_regs),
            region_def_counts: native_region_def_counts(code, n_regs, header, exit),
        })
    }

    fn const_int_before(&self, reg: usize, ip: usize) -> Option<i64> {
        self.values.const_int_before(reg, ip)
    }

    fn list_len_source_before(&self, reg: usize, ip: usize) -> Option<(usize, usize)> {
        self.values.list_len_source_before(reg, ip)
    }

    pub(in crate::reg_vm) fn live_in(&self, ip: usize, reg: usize) -> Option<bool> {
        self.liveness.live_in(ip, reg)
    }

    pub(in crate::reg_vm) fn live_out(&self, ip: usize, reg: usize) -> Option<bool> {
        self.liveness.live_out(ip, reg)
    }



    pub(in crate::reg_vm) fn reachable_mask(&self) -> Vec<bool> {
        self.liveness.cfg.reachable_mask()
    }

    fn root_before(&self, ip: usize, reg: usize) -> Option<usize> {
        self.effects.root_before(ip, reg)
    }

    fn writes_list_root(&self, start: usize, end: usize, list_root: usize) -> bool {
        self.effects.writes_list_root(start, end, list_root)
    }

    fn alias_class_readonly_for_list_slice(&self, code: &[RegInstr], slice_reg: usize) -> bool {
        self.effects
            .alias_class_readonly_for_list_slice(code, self.n_regs, slice_reg)
    }

    pub(super) fn mark_external_writes(&self, code: &[RegInstr], mask: &mut [bool]) -> Option<()> {
        if mask.len() < self.n_regs {
            return None;
        }
        for (ip, instr) in code.iter().enumerate() {
            if ip >= self.header && ip < self.exit {
                continue;
            }
            match instr_written_reg(instr) {
                RegFootprint::Some(writes) => {
                    for reg in writes {
                        if reg < self.n_regs {
                            mask[reg] = true;
                        }
                    }
                }
                RegFootprint::All => return None,
            }
        }
        Some(())
    }

    pub(super) fn global_def_count(&self, reg: usize) -> Option<usize> {
        if reg >= self.n_regs {
            return None;
        }
        Some(*self.global_def_counts.as_ref()?.get(reg)?)
    }

    pub(super) fn region_def_count(&self, reg: usize) -> Option<usize> {
        if reg >= self.n_regs {
            return None;
        }
        Some(*self.region_def_counts.as_ref()?.get(reg)?)
    }

    pub(super) fn single_def_ip_of(&self, code: &[RegInstr], reg: usize) -> Option<usize> {
        if self.global_def_count(reg)? != 1 {
            return None;
        }
        for (ip, instr) in code.iter().enumerate() {
            match instr_written_reg(instr) {
                RegFootprint::Some(writes) if writes.contains(&reg) => return Some(ip),
                RegFootprint::Some(_) => {}
                RegFootprint::All => return None,
            }
        }
        None
    }

    pub(super) fn writer_ips_of(&self, code: &[RegInstr], reg: usize) -> Option<Vec<usize>> {
        if reg >= self.n_regs {
            return None;
        }
        let mut writers = Vec::new();
        for (ip, instr) in code.iter().enumerate() {
            match instr_written_reg(instr) {
                RegFootprint::Some(writes) => {
                    if writes.contains(&reg) {
                        writers.push(ip);
                    }
                }
                RegFootprint::All => return None,
            }
        }
        Some(writers)
    }

    pub(super) fn mark_external_reads_touching(
        &self,
        code: &[RegInstr],
        source: &[bool],
        out: &mut [bool],
    ) -> Option<()> {
        if source.len() < self.n_regs || out.len() < self.n_regs {
            return None;
        }
        for (ip, instr) in code.iter().enumerate() {
            if ip >= self.header && ip < self.exit {
                continue;
            }
            match instr_read_regs(instr) {
                RegFootprint::Some(reads) => {
                    for reg in reads {
                        if reg < self.n_regs && source[reg] {
                            out[reg] = true;
                        }
                    }
                }
                RegFootprint::All => return None,
            }
        }
        Some(())
    }

    // Region IPs intentionally index bytecode and alias tables together.
    pub(super) fn close_region_move_aliases(
        &self,
        code: &[RegInstr],
        mask: &mut [bool],
    ) -> Option<()> {
        if mask.len() < self.n_regs {
            return None;
        }
        let mut changed = true;
        while changed {
            changed = false;
            for i in parallel_indices(self.header..self.exit) {
                match &code[i] {
                    RegInstr::Move { dst, src } if *dst < self.n_regs && *src < self.n_regs => {
                        if mask[*src] && !mask[*dst] {
                            mask[*dst] = true;
                            changed = true;
                        }
                    }
                    RegInstr::Move { .. } => return None,
                    _ => {}
                }
            }
        }
        Some(())
    }

    pub(super) fn close_reachable_move_aliases(
        &self,
        code: &[RegInstr],
        mask: &mut [bool],
    ) -> Option<()> {
        if mask.len() < self.n_regs {
            return None;
        }
        let reachable_ips = self.values.cfg.reachable_ips();
        let mut changed = true;
        while changed {
            changed = false;
            for &i in &reachable_ips {
                match &code[i] {
                    RegInstr::Move { dst, src } if *dst < self.n_regs && *src < self.n_regs => {
                        if mask[*src] && !mask[*dst] {
                            mask[*dst] = true;
                            changed = true;
                        }
                    }
                    RegInstr::Move { .. } => return None,
                    _ => {}
                }
            }
        }
        Some(())
    }





    pub(super) fn forward_definite_regs(
        &self,
        code: &[RegInstr],
        transfer: impl FnMut(usize, &RegInstr, &mut [bool]) -> Option<()>,
    ) -> Option<Vec<Option<Vec<bool>>>> {
        native_forward_definite_regs(&self.values.cfg, code, self.n_regs, transfer)
    }
}

#[cfg(feature = "native-jit")]
impl NativeRegionEffects {
    fn compute(code: &[RegInstr], n_regs: usize, exit: usize) -> Self {
        let bounded_exit = exit.min(code.len());
        let mut roots: Vec<usize> = (0..n_regs).collect();
        let mut roots_before = Vec::with_capacity(bounded_exit + 1);
        let mut list_writes = Vec::new();
        let mut move_alias_roots: Vec<usize> = (0..n_regs).collect();

        for (ip, instr) in code.iter().take(bounded_exit).enumerate() {
            roots_before.push(roots.clone());
            let semantics = native_instr_semantics(instr);
            if let Some(write_list) = semantics.list_write
                && roots.get(write_list).copied().is_some()
            {
                list_writes.push((ip, roots[write_list]));
            }
            match instr {
                RegInstr::Move { dst, src } if *dst < n_regs && *src < n_regs => {
                    roots[*dst] = roots[*src];
                    native_union_roots(&mut move_alias_roots, *dst, *src);
                }
                _ => {
                    if let Some(dst) = semantics.dst
                        && dst < n_regs
                    {
                        roots[dst] = dst;
                    }
                }
            }
        }
        roots_before.push(roots);

        Self {
            roots_before,
            list_writes,
            move_alias_roots,
        }
    }

    fn root_before(&self, ip: usize, reg: usize) -> Option<usize> {
        self.roots_before.get(ip)?.get(reg).copied()
    }

    fn writes_list_root(&self, start: usize, end: usize, list_root: usize) -> bool {
        self.list_writes
            .iter()
            .any(|(ip, root)| *ip >= start && *ip < end && *root == list_root)
    }

    fn alias_mask(&self, reg: usize) -> Option<Vec<bool>> {
        let root = native_find_root_readonly(&self.move_alias_roots, reg)?;
        Some(
            (0..self.move_alias_roots.len())
                .map(|other| native_find_root_readonly(&self.move_alias_roots, other) == Some(root))
                .collect(),
        )
    }

    fn alias_class_readonly_for_list_slice(
        &self,
        code: &[RegInstr],
        n_regs: usize,
        slice_reg: usize,
    ) -> bool {
        if slice_reg >= n_regs {
            return false;
        }

        let Some(alias) = self.alias_mask(slice_reg) else {
            return false;
        };

        code.iter().all(|instr| {
            match instr {
                RegInstr::ListGet { list, .. } | RegInstr::ListLen { list, .. }
                    if *list < n_regs && alias[*list] =>
                {
                    return true;
                }
                RegInstr::DeepCopy { reg } | RegInstr::DeepCopyElided { reg }
                    if *reg < n_regs && alias[*reg] =>
                {
                    return true;
                }
                RegInstr::Move { dst, src }
                    if *dst < n_regs && *src < n_regs && alias[*dst] && alias[*src] =>
                {
                    return true;
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::ListSlice,
                    ..
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::ListSlice,
                    ..
                } if *dst == slice_reg => {
                    return true;
                }
                instr => {
                    if let Some(dst) = native_subset_dst(instr)
                        && dst < n_regs
                        && alias[dst]
                    {
                        return false;
                    }
                }
            }

            match instr_read_regs(instr) {
                RegFootprint::Some(reads) => {
                    !reads.into_iter().any(|reg| reg < n_regs && alias[reg])
                }
                RegFootprint::All => false,
            }
        })
    }
}

#[cfg(feature = "native-jit")]
fn native_find_root_readonly(roots: &[usize], mut reg: usize) -> Option<usize> {
    if reg >= roots.len() {
        return None;
    }
    while roots[reg] != reg {
        reg = roots[reg];
        if reg >= roots.len() {
            return None;
        }
    }
    Some(reg)
}

#[cfg(feature = "native-jit")]
fn native_find_root_mut(roots: &mut [usize], reg: usize) -> usize {
    let parent = roots[reg];
    if parent != reg {
        let root = native_find_root_mut(roots, parent);
        roots[reg] = root;
        root
    } else {
        reg
    }
}

#[cfg(feature = "native-jit")]
fn native_union_roots(roots: &mut [usize], lhs: usize, rhs: usize) {
    let lhs_root = native_find_root_mut(roots, lhs);
    let rhs_root = native_find_root_mut(roots, rhs);
    if lhs_root != rhs_root {
        roots[lhs_root] = rhs_root;
    }
}

#[cfg(feature = "native-jit")]
fn native_full_list_slice_elision_candidate(
    code: &[RegInstr],
    header: usize,
    exit: usize,
    i: usize,
    analysis: &NativeRegionAnalysis,
) -> Option<(usize, usize)> {
    if i < header || i >= exit {
        return None;
    }
    let (dst, list, start, len) = match code.get(i)? {
        RegInstr::CallIntrinsic {
            dst,
            intrinsic: RegIntrinsic::ListSlice,
            args,
        }
        | RegInstr::CallTypedIntrinsic {
            dst,
            intrinsic: RegIntrinsic::ListSlice,
            args,
            ..
        } if args.len() == 3 => (*dst, args[0], args[1], args[2]),
        _ => return None,
    };

    if analysis.const_int_before(start, i) != Some(0) {
        return None;
    }
    if analysis.live_in(i, list) != Some(true) || analysis.live_out(i, dst) != Some(true) {
        return None;
    }
    let (len_source, len_ip) = analysis.list_len_source_before(len, i)?;
    if len_source != list {
        return None;
    }
    if !analysis.alias_class_readonly_for_list_slice(code, dst) {
        return None;
    }

    let list_root = analysis.root_before(i, list)?;
    if analysis.writes_list_root(len_ip + 1, i, list_root)
        || analysis.writes_list_root(header, exit, list_root)
    {
        return None;
    }

    Some((dst, list))
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_region_has_readonly_full_list_slice_elision(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> bool {
    if header >= exit || exit > code.len() {
        return false;
    }
    let Some(analysis) = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit) else {
        return false;
    };
    (header..exit).any(|i| {
        native_full_list_slice_elision_candidate(code, header, exit, i, &analysis).is_some()
    })
}

/// Elide a materialized full-list slice when the slice is only used for read-only
/// list queries inside a native region:
///
/// ```text
/// tmp = List.slice(list, 0, List.len(list))
/// x   = List.get(tmp, i)
/// ```
///
/// becomes a handle alias:
///
/// ```text
/// tmp = list
/// x   = List.get(tmp, i)
/// ```
///
/// This is intentionally narrower than a general partial-slice view. A partial
/// slice needs an extra `i < slice_len` guard to preserve `List.get` failures, while
/// the full-slice case preserves the same bounds behavior through the original
/// source list. We also require the source list to remain unwritten in the region so
/// the shallow copy's read-only behavior is indistinguishable from an alias.
#[cfg(feature = "native-jit")]
// Rewriting requires the original instruction index to update the origin map.
pub(in crate::reg_vm) fn native_elide_readonly_full_list_slices_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }

    let mut out = code.to_vec();
    let mut changed = false;
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;
    for i in parallel_indices(header..exit) {
        let Some((dst, list)) =
            native_full_list_slice_elision_candidate(code, header, exit, i, &analysis)
        else {
            continue;
        };

        out[i] = RegInstr::Move { dst, src: list };
        changed = true;
    }

    let ip_map: Vec<usize> = (0..code.len()).collect();
    if changed {
        Some((out, n_regs, ip_map))
    } else {
        Some((code.to_vec(), n_regs, ip_map))
    }
}
