//! JitHeapHandle impl + JIT host helpers (part 1) — impls/free-fns split from `reg_vm/mod.rs` for module-size partitioning.
//! All type definitions stay in mod.rs.

#[cfg(feature = "native-jit")]
use super::*;

#[cfg(feature = "native-jit")]
impl JitHeapHandle {
    pub(in crate::reg_vm) fn encode_output(index: usize) -> Option<i64> {
        let index = i64::try_from(index).ok()?;
        index.checked_add(1)?.checked_neg()
    }

    pub(in crate::reg_vm) fn decode(bits: i64) -> Option<Self> {
        if bits >= 0 {
            return usize::try_from(bits).ok().map(JitHeapHandle::Input);
        }
        let index = bits.checked_add(1)?.checked_neg()?;
        usize::try_from(index).ok().map(JitHeapHandle::Output)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_cached_heap_value_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
) -> Option<VmValue> {
    if let Some(value) = JIT_HEAP_VALUE_CACHE.with(|cache| {
        cache
            .borrow()
            .iter()
            .find(|entry| entry.handle == handle)
            .map(|entry| entry.value.clone())
    }) {
        return Some(value);
    }

    let value = match JitHeapHandle::decode(handle)? {
        JitHeapHandle::Input(index) => ctx.clone_heap_arg(index),
        JitHeapHandle::Output(index) => ctx.clone_heap_result(index),
    }?;

    JIT_HEAP_VALUE_CACHE.with(|cache| {
        const CACHE_LIMIT: usize = 4;
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_LIMIT {
            cache.remove(0);
        }
        cache.push(JitHeapValueCache {
            handle,
            value: value.clone(),
        });
    });
    Some(value)
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_materialize_heap_result(handle: i64) -> Option<VmValue> {
    match JitHeapHandle::decode(handle)? {
        JitHeapHandle::Input(index) => JitHostCallCtx::active()?.clone_heap_arg(index),
        JitHeapHandle::Output(index) => JitHostCallCtx::active()?.clone_heap_result(index),
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_heap_result_root_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
) -> Option<usize> {
    match JitHeapHandle::decode(handle)? {
        JitHeapHandle::Input(index) => Some(index),
        JitHeapHandle::Output(index) => ctx.heap_result_root(index),
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_heap_handle_needs_write_undo(handle: i64) -> bool {
    matches!(JitHeapHandle::decode(handle), Some(JitHeapHandle::Input(_)))
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_materialize_heap_writebacks(
    input_slots: &[(usize, usize)],
) -> Option<Vec<(usize, VmValue)>> {
    JitHostCallCtx::active()?.with_heap_writebacks(|writebacks| {
        let mut materialized = Vec::new();
        for (input, slot) in input_slots {
            if let Some((_, handle)) = writebacks
                .iter()
                .rev()
                .find(|(updated_input, _)| updated_input == input)
            {
                materialized.push((*slot, jit_materialize_heap_result(*handle)?));
            }
        }
        Some(materialized)
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_heap_list_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
) -> Option<Rc<RefCell<TypedVec>>> {
    if let Some(cached) = JIT_LIST_HANDLE_CACHE.with(|cache| {
        let cache = cache.borrow();
        cache
            .as_ref()
            .and_then(|cached| (cached.handle == handle).then(|| Rc::clone(&cached.list)))
    }) {
        return Some(cached);
    }

    ctx.heap_read_handle(handle, |value| match value {
        VmValue::List(list) => Some(Rc::clone(list)),
        VmValue::Managed(inner) => match &*inner.borrow() {
            VmValue::List(list) => Some(Rc::clone(list)),
            _ => None,
        },
        _ => None,
    })
    .inspect(|list| {
        JIT_LIST_HANDLE_CACHE.with(|cache| {
            *cache.borrow_mut() = Some(JitListHandleCache {
                handle,
                list: Rc::clone(list),
            });
        });
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_heap_map_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
) -> Option<Rc<RefCell<ValueMap>>> {
    if let Some(cached) = JIT_MAP_HANDLE_CACHE.with(|cache| {
        let cache = cache.borrow();
        cache
            .as_ref()
            .and_then(|cached| (cached.handle == handle).then(|| Rc::clone(&cached.map)))
    }) {
        return Some(cached);
    }

    ctx.heap_read_handle(handle, |value| match value {
        VmValue::Map(map) => Some(Rc::clone(map)),
        VmValue::Managed(inner) => match &*inner.borrow() {
            VmValue::Map(map) => Some(Rc::clone(map)),
            _ => None,
        },
        _ => None,
    })
    .inspect(|map| {
        JIT_MAP_HANDLE_CACHE.with(|cache| {
            *cache.borrow_mut() = Some(JitMapHandleCache {
                handle,
                map: Rc::clone(map),
            });
        });
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_heap_deque_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
) -> Option<Rc<RefCell<VecDeque<VmValue>>>> {
    if handle >= 0
        && let Some(cached) = JIT_DEQUE_HANDLE_CACHE.with(|cache| {
            let cache = cache.borrow();
            cache
                .as_ref()
                .and_then(|cached| (cached.handle == handle).then(|| Rc::clone(&cached.deque)))
        })
    {
        return Some(cached);
    }
    ctx.heap_read_handle(handle, |value| match value {
        VmValue::Deque(deque) => Some(Rc::clone(deque)),
        VmValue::Managed(inner) => match &*inner.borrow() {
            VmValue::Deque(deque) => Some(Rc::clone(deque)),
            _ => None,
        },
        _ => None,
    })
    .inspect(|deque| {
        if handle >= 0 {
            JIT_DEQUE_HANDLE_CACHE.with(|cache| {
                *cache.borrow_mut() = Some(JitDequeHandleCache {
                    handle,
                    deque: Rc::clone(deque),
                });
            });
        }
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_snapshot_list_before_write(
    handle: i64,
    list: &Rc<RefCell<TypedVec>>,
) -> bool {
    if !JitCallCtx::is_active() {
        return false;
    }
    if !jit_heap_handle_needs_write_undo(handle) {
        return true;
    }
    jit_snapshot_input_list_before_write(list)
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_snapshot_input_list_before_write(
    list: &Rc<RefCell<TypedVec>>,
) -> bool {
    if !JitCallCtx::is_active() {
        return false;
    }
    if !jit_mark_heap_snapshot(JitHeapSnapshotKey::List(Rc::as_ptr(list))) {
        return true;
    }
    JIT_HEAP_WRITE_UNDO.with(|undo| {
        undo.borrow_mut().push(JitHeapWriteUndo::List(
            Rc::clone(list),
            list.borrow().clone_preserving_capacity(),
        ));
    });
    true
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_struct_field_list(
    value: &VmValue,
    slot: usize,
) -> Option<Rc<RefCell<TypedVec>>> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => match data.fields.get(slot)? {
            VmValue::List(list) => Some(Rc::clone(list)),
            VmValue::Managed(inner) => match &*inner.borrow() {
                VmValue::List(list) => Some(Rc::clone(list)),
                _ => None,
            },
            _ => None,
        },
        VmValue::Managed(inner) => jit_struct_field_list(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_value_may_contain_list(value: &VmValue) -> bool {
    matches!(
        value,
        VmValue::List(_)
            | VmValue::Deque(_)
            | VmValue::Map(_)
            | VmValue::OptionSomeHeap(_)
            | VmValue::Struct(_)
            | VmValue::Variant(_)
            | VmValue::Managed(_)
            | VmValue::Closure(_)
    )
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_value_contains_list_rc(
    value: &VmValue,
    needle: &Rc<RefCell<TypedVec>>,
) -> bool {
    // `seen` tracks the pointer identity of EVERY interior-mutable container already
    // entered (`List`/`Deque`/`Map`/`Managed`), not just `Managed`. A heap graph can be
    // cyclic (e.g. a `List` that, through a `RefCell`, contains itself), so without
    // recording every container identity the recursion would loop forever / stack-overflow.
    // A revisit returns `false`: the needle is matched by `Rc::ptr_eq` on FIRST visit, so a
    // back-edge to an already-seen node cannot be (a fresh path to) the needle.
    pub(in crate::reg_vm) fn contains(
        value: &VmValue,
        needle: &Rc<RefCell<TypedVec>>,
        seen: &mut Vec<usize>,
    ) -> bool {
        // Returns false if `ptr` was already visited; otherwise records it and returns true.
        fn first_visit(seen: &mut Vec<usize>, ptr: usize) -> bool {
            if seen.contains(&ptr) {
                return false;
            }
            seen.push(ptr);
            true
        }
        match value {
            VmValue::List(list) => {
                Rc::ptr_eq(list, needle)
                    || (first_visit(seen, Rc::as_ptr(list) as usize) && {
                        let borrowed = list.borrow();
                        borrowed.iter().any(|item| contains(&item, needle, seen))
                    })
            }
            VmValue::Deque(deque) => {
                first_visit(seen, Rc::as_ptr(deque) as usize)
                    && deque
                        .borrow()
                        .iter()
                        .any(|item| contains(item, needle, seen))
            }
            VmValue::Map(map) => {
                first_visit(seen, Rc::as_ptr(map) as usize)
                    && map.borrow().iter().any(|(key, value)| {
                        contains(key.value(), needle, seen) || contains(value, needle, seen)
                    })
            }
            VmValue::OptionSomeHeap(value) => contains(value, needle, seen),
            VmValue::Struct(data) | VmValue::Variant(data) => data
                .fields
                .iter()
                .any(|field| contains(field, needle, seen)),
            VmValue::Managed(inner) => {
                first_visit(seen, Rc::as_ptr(inner) as usize)
                    && contains(&inner.borrow(), needle, seen)
            }
            VmValue::Closure(closure) => closure
                .captures
                .iter()
                .any(|capture| contains(capture, needle, seen)),
            _ => false,
        }
    }

    if !jit_value_may_contain_list(value) {
        return false;
    }
    contains(value, needle, &mut Vec::new())
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_heap_inputs_alias_flat_mut(
    input_slots: &[(usize, usize)],
    flat_mut_owned: &[Rc<RefCell<TypedVec>>],
) -> bool {
    if flat_mut_owned.is_empty() || input_slots.is_empty() {
        return false;
    }
    let Some(ctx) = JitHostCallCtx::active() else {
        return false;
    };
    input_slots.iter().any(|(input, _)| {
        ctx.with_heap_arg(*input, |value| {
            if !jit_value_may_contain_list(value) {
                return Some(false);
            }
            Some(
                flat_mut_owned
                    .iter()
                    .any(|list| jit_value_contains_list_rc(value, list)),
            )
        })
        .unwrap_or(false)
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_selected_heap_inputs_alias_flat_mut(
    input_slots: &[(usize, usize)],
    flat_mut_owned: &[Rc<RefCell<TypedVec>>],
    frame_base: usize,
    heap_input_regs: &[usize],
) -> bool {
    if flat_mut_owned.is_empty() || input_slots.is_empty() || heap_input_regs.is_empty() {
        return false;
    }
    let Some(ctx) = JitHostCallCtx::active() else {
        return false;
    };
    input_slots.iter().any(|(input, absolute_reg)| {
        let Some(reg) = absolute_reg.checked_sub(frame_base) else {
            return false;
        };
        if !heap_input_regs.contains(&reg) {
            return false;
        }
        ctx.with_heap_arg(*input, |value| {
            if !jit_value_may_contain_list(value) {
                return Some(false);
            }
            Some(
                flat_mut_owned
                    .iter()
                    .any(|list| jit_value_contains_list_rc(value, list)),
            )
        })
        .unwrap_or(false)
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_snapshot_map_before_write(
    handle: i64,
    map: &Rc<RefCell<ValueMap>>,
) -> bool {
    if !JitCallCtx::is_active() {
        return false;
    }
    if !jit_heap_handle_needs_write_undo(handle) {
        return true;
    }
    if !jit_mark_heap_snapshot(JitHeapSnapshotKey::Map(Rc::as_ptr(map))) {
        return true;
    }
    JIT_HEAP_WRITE_UNDO.with(|undo| {
        undo.borrow_mut().push(JitHeapWriteUndo::Map(
            Rc::clone(map),
            clone_value_map_preserving_capacity(&map.borrow()),
        ));
    });
    true
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_snapshot_deque_before_write(
    handle: i64,
    deque: &Rc<RefCell<VecDeque<VmValue>>>,
) -> bool {
    if !JitCallCtx::is_active() {
        return false;
    }
    if !jit_heap_handle_needs_write_undo(handle) {
        return true;
    }
    if !jit_mark_heap_snapshot(JitHeapSnapshotKey::Deque(Rc::as_ptr(deque))) {
        return true;
    }
    JIT_HEAP_WRITE_UNDO.with(|undo| {
        undo.borrow_mut().push(JitHeapWriteUndo::Deque(
            Rc::clone(deque),
            deque.borrow().clone(),
        ));
    });
    true
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_with_journaled_list_write_with_ctx<R>(
    ctx: JitHostCallCtx,
    handle: i64,
    write: impl FnOnce(&mut TypedVec) -> Option<R>,
) -> Option<R> {
    let list = ctx.heap_list_handle(handle)?;
    if !jit_snapshot_list_before_write(handle, &list) {
        return None;
    }
    write(&mut list.borrow_mut())
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_with_journaled_map_write_with_ctx<R>(
    ctx: JitHostCallCtx,
    handle: i64,
    write: impl FnOnce(&mut ValueMap) -> Option<R>,
) -> Option<R> {
    let map = ctx.heap_map_handle(handle)?;
    if !jit_snapshot_map_before_write(handle, &map) {
        return None;
    }
    write(&mut map.borrow_mut())
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_with_journaled_deque_write_with_ctx<R>(
    ctx: JitHostCallCtx,
    handle: i64,
    write: impl FnOnce(&mut VecDeque<VmValue>) -> Option<R>,
) -> Option<R> {
    let deque = ctx.heap_deque_handle(handle)?;
    if !jit_snapshot_deque_before_write(handle, &deque) {
        return None;
    }
    write(&mut deque.borrow_mut())
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_restore_heap_writes() {
    JIT_HEAP_WRITE_UNDO.with(|undo| {
        for entry in undo.borrow_mut().drain(..).rev() {
            match entry {
                JitHeapWriteUndo::List(list, original) => {
                    *list.borrow_mut() = original;
                }
                JitHeapWriteUndo::Map(map, original) => {
                    *map.borrow_mut() = original;
                }
                JitHeapWriteUndo::Deque(deque, original) => {
                    *deque.borrow_mut() = original;
                }
            }
        }
    });
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_struct_field_int(value: &VmValue, slot: usize) -> Option<i64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => match data.fields.get(slot)? {
            VmValue::Int(v) => Some(*v),
            _ => None,
        },
        VmValue::Managed(inner) => jit_struct_field_int(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_struct_with_int_field_updates(
    value: &VmValue,
    updates: &[(usize, i64)],
) -> Option<VmValue> {
    match value {
        VmValue::Struct(data) => {
            let mut fields = data.fields.clone();
            for (slot, updated) in updates {
                let field = fields.get_mut(*slot)?;
                if !matches!(field, VmValue::Int(_)) {
                    return None;
                }
                *field = VmValue::Int(*updated);
            }
            Some(VmValue::Struct(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        VmValue::Variant(data) => {
            let mut fields = data.fields.clone();
            for (slot, updated) in updates {
                let field = fields.get_mut(*slot)?;
                if !matches!(field, VmValue::Int(_)) {
                    return None;
                }
                *field = VmValue::Int(*updated);
            }
            Some(VmValue::Variant(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        VmValue::Managed(inner) => jit_struct_with_int_field_updates(&inner.borrow(), updates),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_struct_field_float(value: &VmValue, slot: usize) -> Option<f64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => match data.fields.get(slot)? {
            VmValue::Float(v) => Some(*v),
            _ => None,
        },
        VmValue::Managed(inner) => jit_struct_field_float(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_deadline_expired(_ctx: vm_jit::HostCtx) -> i64 {
        let Some(_ctx) = JitHostCallCtx::from_token(_ctx) else {
            // A stale/malformed host context must fail closed. The caller will
            // bail through the ordinary deadline check path.
            return 1;
        };
        i64::from(JitCallCtx::deadline_expired())
    }
}

#[cfg(all(test, feature = "native-jit"))]
jit_host_boundary! {
    extern "C" fn rss_jit_panicking_helper_for_test(_ctx: vm_jit::HostCtx) -> i64 {
        panic!("intentional host-helper panic")
    }
}

#[cfg(all(test, feature = "native-jit"))]
#[test]
pub(in crate::reg_vm) fn native_host_helper_panics_are_contained_at_the_ffi_boundary() {
    assert_eq!(rss_jit_panicking_helper_for_test(0), 0);
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_field_int(_ctx: vm_jit::HostCtx, handle: i64, slot: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match usize::try_from(slot)
            .ok()
            .and_then(|slot| ctx.heap_read_handle(handle, |value| jit_struct_field_int(value, slot)))
        {
            Some(value) => value,
            None => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_field_set_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        slot: i64,
        value: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_field_set_int_with_ctx(ctx, handle, slot, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_field_set_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    slot: i64,
    value: i64,
) -> i64 {
    let Some(slot) = usize::try_from(slot).ok() else {
        ctx.signal_bail();
        return 0;
    };
    let root = jit_heap_result_root_with_ctx(ctx, handle);
    let updated = ctx.heap_read_handle(handle, |heap| match heap {
        VmValue::Struct(data) => {
            let mut fields = data.fields.clone();
            let field = fields.get_mut(slot)?;
            if !matches!(field, VmValue::Int(_)) {
                return None;
            }
            *field = VmValue::Int(value);
            Some(VmValue::Struct(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        VmValue::Variant(data) => {
            let mut fields = data.fields.clone();
            let field = fields.get_mut(slot)?;
            if !matches!(field, VmValue::Int(_)) {
                return None;
            }
            *field = VmValue::Int(value);
            Some(VmValue::Variant(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        _ => None,
    });
    match updated {
        Some(value) => {
            let handle = jit_push_heap_result_with_root_with_ctx(ctx, value, root);
            if let Some(root) = root {
                ctx.push_heap_writeback(root, handle);
            }
            handle
        }
        None => {
            ctx.signal_bail();
            0
        }
    }
}

// transactional heap mutation (heap-value struct write): set a struct/variant field to a **heap** value —
// the heap analog of [`rss_jit_field_set_int`]. Resolves the value handle, then COW-
// rebuilds the struct with the field replaced and publishes the new value (ReplacesInput
// + writeback to the root). A scalar field at the slot is a shape mismatch ⇒ bail.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_field_set_handle(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        slot: i64,
        value_handle: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_field_set_handle_with_ctx(ctx, handle, slot, value_handle)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_field_set_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    slot: i64,
    value_handle: i64,
) -> i64 {
    let Some(slot) = usize::try_from(slot).ok() else {
        ctx.signal_bail();
        return 0;
    };
    // Resolve the new heap field value before the COW write.
    let Some(new_value) = ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
        ctx.signal_bail();
        return 0;
    };
    let root = jit_heap_result_root_with_ctx(ctx, handle);
    let updated = ctx.heap_read_handle(handle, |heap| {
        let (mut fields, layout, is_variant) = match heap {
            VmValue::Struct(data) => (data.fields.clone(), Rc::clone(&data.layout), false),
            VmValue::Variant(data) => (data.fields.clone(), Rc::clone(&data.layout), true),
            _ => return None,
        };
        let field = fields.get_mut(slot)?;
        // A scalar field can never hold a heap value ⇒ shape mismatch ⇒ bail.
        if matches!(
            field,
            VmValue::Int(_) | VmValue::Float(_) | VmValue::Bool(_)
        ) {
            return None;
        }
        *field = new_value;
        let s = Rc::new(VmStruct::with_layout(layout, fields));
        Some(if is_variant {
            VmValue::Variant(s)
        } else {
            VmValue::Struct(s)
        })
    });
    match updated {
        Some(value) => {
            let handle = jit_push_heap_result_with_root_with_ctx(ctx, value, root);
            if let Some(root) = root {
                ctx.push_heap_writeback(root, handle);
            }
            handle
        }
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_field_set_float(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        slot: i64,
        value: f64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_field_set_float_with_ctx(ctx, handle, slot, value)
    }
}

/// Copy-on-write set of a `Float` struct/variant field — the write-side mirror of
/// `rss_jit_field_float`. A non-Float field (or out-of-range slot / wrong handle)
/// bails out-of-band, so a mis-typed lowering falls back to the interpreter rather
/// than corrupting the value.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_field_set_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    slot: i64,
    value: f64,
) -> i64 {
    let Some(slot) = usize::try_from(slot).ok() else {
        ctx.signal_bail();
        return 0;
    };
    let root = jit_heap_result_root_with_ctx(ctx, handle);
    let updated = ctx.heap_read_handle(handle, |heap| match heap {
        VmValue::Struct(data) => {
            let mut fields = data.fields.clone();
            let field = fields.get_mut(slot)?;
            if !matches!(field, VmValue::Float(_)) {
                return None;
            }
            *field = VmValue::Float(value);
            Some(VmValue::Struct(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        VmValue::Variant(data) => {
            let mut fields = data.fields.clone();
            let field = fields.get_mut(slot)?;
            if !matches!(field, VmValue::Float(_)) {
                return None;
            }
            *field = VmValue::Float(value);
            Some(VmValue::Variant(Rc::new(VmStruct::with_layout(
                Rc::clone(&data.layout),
                fields,
            ))))
        }
        _ => None,
    });
    match updated {
        Some(value) => {
            let handle = jit_push_heap_result_with_root_with_ctx(ctx, value, root);
            if let Some(root) = root {
                ctx.push_heap_writeback(root, handle);
            }
            handle
        }
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(list) = ctx.heap_list_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        match i64::try_from(list.borrow().len()) {
            Ok(value) => value,
            Err(_) => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(list) = ctx.heap_list_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        i64::from(list.borrow().is_empty())
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_get_int(_ctx: vm_jit::HostCtx, handle: i64, index: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(index) = usize::try_from(index).ok() else {
            ctx.signal_bail();
            return 0;
        };
        let Some(list) = ctx.heap_list_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        let borrowed = list.borrow();
        match &*borrowed {
            TypedVec::Ints(values) => match values.get(index) {
                Some(value) => *value,
                None => {
                    ctx.signal_bail();
                    0
                }
            },
            TypedVec::Boxed(values) => match values.get(index) {
                Some(VmValue::Int(value)) => *value,
                Some(_) | None => {
                    ctx.signal_bail();
                    0
                }
            },
            TypedVec::Floats(_) => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_set_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        index: i64,
        value: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_list_set_int_with_ctx(ctx, handle, index, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_list_set_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    index: i64,
    value: i64,
) -> i64 {
    let Some(index) = usize::try_from(index).ok() else {
        ctx.signal_bail();
        return 0;
    };
    match ctx.with_journaled_list_write(handle, |list| {
        if index >= list.len() {
            return None;
        }
        list.checked_set(index, VmValue::Int(value)).ok()?;
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_set_float(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        index: i64,
        value: f64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_list_set_float_with_ctx(ctx, handle, index, value)
    }
}

/// Set a `Float` list element — the write-side mirror of `rss_jit_list_get_float`.
/// A non-Float list / out-of-bounds index bails out-of-band.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_list_set_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    index: i64,
    value: f64,
) -> i64 {
    let Some(index) = usize::try_from(index).ok() else {
        ctx.signal_bail();
        return 0;
    };
    match ctx.with_journaled_list_write(handle, |list| {
        if index >= list.len() {
            return None;
        }
        list.checked_set(index, VmValue::Float(value)).ok()?;
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_push_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_list_push_int_with_ctx(ctx, handle, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_list_push_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value: i64,
) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        // `checked_push_accounted` returns the flat-capacity growth in bytes — exactly
        // what the interpreter's `List.push` bills to `allocation_budget` (`account_bytes`).
        list.checked_push_accounted(VmValue::Int(value)).ok()
    }) {
        Some(grew) => {
            if jit_mem_charge(grew) {
                0
            } else {
                // Over budget: bail. The OSR rolls back this loop's list writes and
                // reruns on the interpreter, which recharges and errors at the exact push.
                ctx.signal_bail();
                0
            }
        }
        None => {
            ctx.signal_bail();
            0
        }
    }
}

// transactional heap mutation (heap-value collection write): push a **heap** element onto a
// `List<HeapType>` — the value side of item #1 (the key side is
// [`rss_jit_map_insert_handle_key_int`]). The value handle is resolved to its heap
// value (host-owned, input or output table) and appended via the journaled list write
// (rolled back on a later bail, the transactional fallback contract). A wrong-type/invalid handle bails.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_push_handle(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        value_handle: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_list_push_handle_with_ctx(ctx, handle, value_handle)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_list_push_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value_handle: i64,
) -> i64 {
    // Resolve the heap value (clone it out of its table) before the journaled write.
    let Some(value) = ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
        ctx.signal_bail();
        return 0;
    };
    match ctx.with_journaled_list_write(handle, move |list| list.checked_push_accounted(value).ok())
    {
        Some(grew) => {
            if !jit_mem_charge(grew) {
                ctx.signal_bail();
            }
            0
        }
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_push_float(_ctx: vm_jit::HostCtx, handle: i64, value: f64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_list_push_float_with_ctx(ctx, handle, value)
    }
}

/// Push a `Float` onto a flat `List<Float>` — the write-side mirror of
/// `rss_jit_list_get_float`. A non-Float list / invalid handle bails out-of-band,
/// so a mis-typed lowering falls back to the interpreter.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_list_push_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value: f64,
) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        list.checked_push_accounted(VmValue::Float(value)).ok()
    }) {
        Some(grew) => {
            if !jit_mem_charge(grew) {
                ctx.signal_bail();
            }
            0
        }
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_sort_int(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_list_sort_int_with_ctx(ctx, handle)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_list_sort_int_with_ctx(ctx: JitHostCallCtx, handle: i64) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        let TypedVec::Ints(values) = list else {
            return None;
        };
        values.sort_unstable();
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_new_int(_ctx: vm_jit::HostCtx) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        ctx.publish_heap_result(VmValue::List(Rc::new(RefCell::new(TypedVec::Ints(
            Vec::new(),
        )))))
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_int_key(value: i64) -> VmMapKey {
    VmMapKey::new(VmValue::Int(value))
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_insert_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        key: i64,
        value: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_map_insert_int_with_ctx(ctx, handle, key, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_map_insert_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    key: i64,
    value: i64,
) -> i64 {
    match ctx.with_journaled_map_write(handle, |map| {
        map.insert(jit_int_key(key), VmValue::Int(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

// transactional heap mutation (heap-key collection write): insert an `Int` value under a **heap key**
// (e.g. a `String`) — the non-`Int`-key analog of [`rss_jit_map_insert_int`]. The key
// handle is resolved to its heap value and wrapped in `VmMapKey`, so hashing/equality
// is the host's own canonical map-key semantics (never re-implemented in native). The
// map write is journaled, so a later bail rolls it back (the transactional fallback contract). A wrong container/key
// shape signals a bail.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_insert_handle_key_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        key_handle: i64,
        value: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_map_insert_handle_key_int_with_ctx(ctx, handle, key_handle, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_map_insert_handle_key_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    key_handle: i64,
    value: i64,
) -> i64 {
    // Resolve the heap key to the host's canonical map key BEFORE the journaled write.
    let Some(key) = ctx.heap_read_handle(key_handle, |value| Some(VmMapKey::new(value.clone())))
    else {
        ctx.signal_bail();
        return 0;
    };
    match ctx.with_journaled_map_write(handle, |map| {
        map.insert(key, VmValue::Int(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_insert_float(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        key: i64,
        value: f64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_map_insert_float_with_ctx(ctx, handle, key, value)
    }
}

/// Insert a `Float` value into an Int-keyed map — the value-side mirror of
/// `rss_jit_map_insert_int`. A bad handle bails out-of-band.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_map_insert_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    key: i64,
    value: f64,
) -> i64 {
    match ctx.with_journaled_map_write(handle, |map| {
        map.insert(jit_int_key(key), VmValue::Float(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_get_int(_ctx: vm_jit::HostCtx, handle: i64, key: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(map) = ctx.heap_map_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        match map.borrow().get(&jit_int_key(key)) {
            Some(VmValue::Int(value)) => *value,
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_get_match_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        key: i64,
        found: &mut i64,
    ) -> i64 {
        *found = 0;
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(map) = ctx.heap_map_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        match map.borrow().get(&jit_int_key(key)) {
            Some(VmValue::Int(value)) => {
                *found = 1;
                *value
            }
            None => 0,
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

// Float value-side mirror of `rss_jit_map_get_match_int`: the lookup is the
// interpreter's own `map.get`; this only extracts the `Float` payload (f64 channel)
// and writes the `found` output in the same host call. A non-Float payload bails
// out-of-band.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_get_match_float(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        key: i64,
        found: &mut i64,
    ) -> f64 {
        *found = 0;
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0.0;
        };
        let Some(map) = ctx.heap_map_handle(handle) else {
            ctx.signal_bail();
            return 0.0;
        };
        match map.borrow().get(&jit_int_key(key)) {
            Some(VmValue::Float(value)) => {
                *found = 1;
                *value
            }
            None => 0.0,
            _ => {
                ctx.signal_bail();
                0.0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_contains_int(_ctx: vm_jit::HostCtx, handle: i64, key: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(map) = ctx.heap_map_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        i64::from(map.borrow().contains_key(&jit_int_key(key)))
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(map) = ctx.heap_map_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        match i64::try_from(map.borrow().len()) {
            Ok(len) => len,
            Err(_) => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_map_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(map) = ctx.heap_map_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        i64::from(map.borrow().is_empty())
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_set_insert_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_set_insert_int_with_ctx(ctx, handle, value)
    }
}
