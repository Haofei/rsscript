use super::*;

pub(super) struct NativeCallScratch {
    pub(super) args: Vec<i64>,
    pub(super) lens: Vec<i64>,
    pub(super) flat_owned: Vec<Rc<RefCell<TypedVec>>>,
    pub(super) flat_mut_owned: Vec<Rc<RefCell<TypedVec>>>,
    pub(super) heap_input_slots: Vec<(usize, usize)>,
}

pub(super) fn take_native_call_scratch(
    native: &mut NativeState,
    n_params: usize,
) -> NativeCallScratch {
    let mut args = std::mem::take(&mut native.scratch_args);
    let mut lens = std::mem::take(&mut native.scratch_lens);
    let mut flat_owned = std::mem::take(&mut native.scratch_flat_owned);
    let mut flat_mut_owned = std::mem::take(&mut native.scratch_flat_mut_owned);
    let mut heap_input_slots = std::mem::take(&mut native.scratch_heap_input_slots);

    args.clear();
    args.resize(n_params, 0i64);
    lens.clear();
    lens.resize(n_params, 0i64);
    flat_owned.clear();
    flat_mut_owned.clear();
    heap_input_slots.clear();

    NativeCallScratch {
        args,
        lens,
        flat_owned,
        flat_mut_owned,
        heap_input_slots,
    }
}

impl NativeCallScratch {
    pub(super) fn restore(mut self, native: Option<&mut NativeState>) {
        let Some(native) = native else {
            return;
        };
        self.args.clear();
        self.lens.clear();
        self.flat_owned.clear();
        self.flat_mut_owned.clear();
        self.heap_input_slots.clear();
        native.scratch_args = self.args;
        native.scratch_lens = self.lens;
        native.scratch_flat_owned = self.flat_owned;
        native.scratch_flat_mut_owned = self.flat_mut_owned;
        native.scratch_heap_input_slots = self.heap_input_slots;
    }
}

pub(super) struct OsrNativeCallScratch {
    pub(super) window: Vec<i64>,
    pub(super) lens: Vec<i64>,
    pub(super) flat_owned: Vec<Rc<RefCell<TypedVec>>>,
    pub(super) flat_mut_owned: Vec<Rc<RefCell<TypedVec>>>,
    pub(super) flat_slots: Vec<(usize, NativeTy)>,
    pub(super) flat_mut_slots: Vec<(usize, usize)>,
    pub(super) heap_input_slots: Vec<(usize, usize)>,
}

pub(super) fn take_osr_native_call_scratch(
    native: &mut NativeState,
    n_jit_regs: usize,
) -> OsrNativeCallScratch {
    let mut window = std::mem::take(&mut native.scratch_osr_window);
    let mut lens = std::mem::take(&mut native.scratch_osr_lens);
    let mut flat_owned = std::mem::take(&mut native.scratch_osr_flat_owned);
    let mut flat_mut_owned = std::mem::take(&mut native.scratch_osr_flat_mut_owned);
    let mut flat_slots = std::mem::take(&mut native.scratch_osr_flat_slots);
    let mut flat_mut_slots = std::mem::take(&mut native.scratch_osr_flat_mut_slots);
    let mut heap_input_slots = std::mem::take(&mut native.scratch_osr_heap_input_slots);

    window.clear();
    window.resize(n_jit_regs, 0i64);
    lens.clear();
    lens.resize(n_jit_regs, 0i64);
    flat_owned.clear();
    flat_mut_owned.clear();
    flat_slots.clear();
    flat_mut_slots.clear();
    heap_input_slots.clear();

    OsrNativeCallScratch {
        window,
        lens,
        flat_owned,
        flat_mut_owned,
        flat_slots,
        flat_mut_slots,
        heap_input_slots,
    }
}

impl OsrNativeCallScratch {
    pub(super) fn restore(mut self, native: Option<&mut NativeState>) {
        let Some(native) = native else {
            return;
        };
        self.window.clear();
        self.lens.clear();
        self.flat_owned.clear();
        self.flat_mut_owned.clear();
        self.flat_slots.clear();
        self.flat_mut_slots.clear();
        self.heap_input_slots.clear();
        native.scratch_osr_window = self.window;
        native.scratch_osr_lens = self.lens;
        native.scratch_osr_flat_owned = self.flat_owned;
        native.scratch_osr_flat_mut_owned = self.flat_mut_owned;
        native.scratch_osr_flat_slots = self.flat_slots;
        native.scratch_osr_flat_mut_slots = self.flat_mut_slots;
        native.scratch_osr_heap_input_slots = self.heap_input_slots;
    }
}
