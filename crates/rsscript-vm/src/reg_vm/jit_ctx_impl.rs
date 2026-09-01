//! `impl JitCallCtx` split from `reg_vm/mod.rs` for module-size partitioning.

#[cfg(feature = "native-jit")]
use super::*;

#[cfg(feature = "native-jit")]
impl JitCallCtx {
    pub(in crate::reg_vm) fn enter_frame(deadline: Option<rsscript_operation::MonotonicDeadline>) {
        JIT_CALL_CTX.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            if ctx.active_depth == 0 {
                ctx.reset_inputs();
                ctx.clear_results();
                ctx.clear_writebacks();
                ctx.active_token = ctx.allocate_token();
                ctx.deadline = deadline;
            }
            ctx.active_depth = ctx.active_depth.saturating_add(1);
        });
        jit_clear_heap_handle_caches();
    }

    pub(in crate::reg_vm) fn exit_frame() -> bool {
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
                ctx.deadline = None;
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

    pub(in crate::reg_vm) fn is_active() -> bool {
        JIT_CALL_CTX.with(|ctx| ctx.borrow().active_depth > 0)
    }

    pub(in crate::reg_vm) fn active_token() -> vm_jit::HostCtx {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth > 0 {
                ctx.active_token
            } else {
                0
            }
        })
    }

    pub(in crate::reg_vm) fn token_is_active(token: vm_jit::HostCtx) -> bool {
        token != 0
            && JIT_CALL_CTX.with(|ctx| {
                let ctx = ctx.borrow();
                ctx.active_depth > 0 && ctx.active_token == token
            })
    }

    pub(in crate::reg_vm) fn deadline_expired() -> bool {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            ctx.active_depth > 0
                && ctx
                    .deadline
                    .is_some_and(rsscript_operation::MonotonicDeadline::is_expired)
        })
    }

    pub(in crate::reg_vm) fn push_heap_arg(value: VmValue) -> usize {
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

    pub(in crate::reg_vm) fn with_heap_arg<R>(
        index: usize,
        read: impl FnOnce(&VmValue) -> Option<R>,
    ) -> Option<R> {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_args.get(index).and_then(read)
        })
    }

    pub(in crate::reg_vm) fn clone_heap_arg(index: usize) -> Option<VmValue> {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_args.get(index).cloned()
        })
    }

    pub(in crate::reg_vm) fn clear_heap_results() {
        JIT_CALL_CTX.with(|ctx| ctx.borrow_mut().clear_results());
    }

    pub(in crate::reg_vm) fn push_heap_result(value: VmValue, root: Option<usize>) -> Option<i64> {
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

    pub(in crate::reg_vm) fn clone_heap_result(index: usize) -> Option<VmValue> {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_results.get(index).cloned()
        })
    }

    pub(in crate::reg_vm) fn heap_result_root(index: usize) -> Option<usize> {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if ctx.active_depth == 0 {
                return None;
            }
            ctx.heap_result_roots.get(index).copied().flatten()
        })
    }

    pub(in crate::reg_vm) fn heap_results_empty() -> bool {
        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            ctx.heap_results.is_empty() && ctx.heap_result_roots.is_empty()
        })
    }

    pub(in crate::reg_vm) fn clear_heap_writebacks() {
        JIT_CALL_CTX.with(|ctx| ctx.borrow_mut().clear_writebacks());
    }

    pub(in crate::reg_vm) fn push_heap_writeback(root: usize, handle: i64) {
        JIT_CALL_CTX.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            if ctx.active_depth > 0 {
                ctx.heap_writebacks.push((root, handle));
            }
        });
    }

    pub(in crate::reg_vm) fn heap_writebacks_empty() -> bool {
        JIT_CALL_CTX.with(|ctx| ctx.borrow().heap_writebacks.is_empty())
    }

    pub(in crate::reg_vm) fn with_heap_writebacks<R>(read: impl FnOnce(&[(usize, i64)]) -> R) -> R {
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
impl JitHostCallCtx {
    pub(in crate::reg_vm) fn active() -> Option<Self> {
        JitCallCtx::is_active().then_some(Self { call_context: 0 })
    }

    pub(in crate::reg_vm) fn from_token(token: vm_jit::HostCtx) -> Option<Self> {
        let user = vm_jit::user_host_ctx(token);
        JitCallCtx::token_is_active(user).then_some(Self {
            call_context: token,
        })
    }

    pub(in crate::reg_vm) fn signal_bail(self) {
        vm_jit::signal_bail(self.call_context);
    }

    pub(in crate::reg_vm) fn push_heap_arg(self, value: VmValue) -> usize {
        JitCallCtx::push_heap_arg(value)
    }

    pub(in crate::reg_vm) fn with_heap_arg<R>(
        self,
        index: usize,
        read: impl FnOnce(&VmValue) -> Option<R>,
    ) -> Option<R> {
        JitCallCtx::with_heap_arg(index, read)
    }

    pub(in crate::reg_vm) fn clone_heap_arg(self, index: usize) -> Option<VmValue> {
        JitCallCtx::clone_heap_arg(index)
    }

    pub(in crate::reg_vm) fn push_heap_result(
        self,
        value: VmValue,
        root: Option<usize>,
    ) -> Option<i64> {
        JitCallCtx::push_heap_result(value, root)
    }

    pub(in crate::reg_vm) fn publish_heap_result(self, value: VmValue) -> i64 {
        jit_push_heap_result_with_root_with_ctx(self, value, None)
    }

    pub(in crate::reg_vm) fn publish_heap_handle(self, value: Option<VmValue>) -> i64 {
        match value {
            Some(value) => self.push_heap_arg(value) as i64,
            None => {
                self.signal_bail();
                0
            }
        }
    }

    pub(in crate::reg_vm) fn clone_heap_result(self, index: usize) -> Option<VmValue> {
        JitCallCtx::clone_heap_result(index)
    }

    pub(in crate::reg_vm) fn heap_result_root(self, index: usize) -> Option<usize> {
        JitCallCtx::heap_result_root(index)
    }

    pub(in crate::reg_vm) fn push_heap_writeback(self, root: usize, handle: i64) {
        JitCallCtx::push_heap_writeback(root, handle);
    }

    pub(in crate::reg_vm) fn with_heap_writebacks<R>(
        self,
        read: impl FnOnce(&[(usize, i64)]) -> R,
    ) -> R {
        JitCallCtx::with_heap_writebacks(read)
    }

    pub(in crate::reg_vm) fn heap_read<R>(
        self,
        handle: i64,
        read: impl FnOnce(&VmValue) -> Option<R>,
    ) -> Option<R> {
        let index = usize::try_from(handle).ok()?;
        self.with_heap_arg(index, read)
    }

    pub(in crate::reg_vm) fn heap_read_handle<R>(
        self,
        handle: i64,
        read: impl FnOnce(&VmValue) -> Option<R>,
    ) -> Option<R> {
        let value = jit_cached_heap_value_with_ctx(self, handle)?;
        read(&value)
    }

    pub(in crate::reg_vm) fn heap_list_handle(self, handle: i64) -> Option<Rc<RefCell<TypedVec>>> {
        jit_heap_list_handle_with_ctx(self, handle)
    }

    pub(in crate::reg_vm) fn heap_map_handle(self, handle: i64) -> Option<Rc<RefCell<ValueMap>>> {
        jit_heap_map_handle_with_ctx(self, handle)
    }

    pub(in crate::reg_vm) fn heap_deque_handle(
        self,
        handle: i64,
    ) -> Option<Rc<RefCell<VecDeque<VmValue>>>> {
        jit_heap_deque_handle_with_ctx(self, handle)
    }

    pub(in crate::reg_vm) fn with_journaled_list_write<R>(
        self,
        handle: i64,
        write: impl FnOnce(&mut TypedVec) -> Option<R>,
    ) -> Option<R> {
        jit_with_journaled_list_write_with_ctx(self, handle, write)
    }

    pub(in crate::reg_vm) fn with_journaled_map_write<R>(
        self,
        handle: i64,
        write: impl FnOnce(&mut ValueMap) -> Option<R>,
    ) -> Option<R> {
        jit_with_journaled_map_write_with_ctx(self, handle, write)
    }

    pub(in crate::reg_vm) fn with_journaled_deque_write<R>(
        self,
        handle: i64,
        write: impl FnOnce(&mut VecDeque<VmValue>) -> Option<R>,
    ) -> Option<R> {
        jit_with_journaled_deque_write_with_ctx(self, handle, write)
    }
}
