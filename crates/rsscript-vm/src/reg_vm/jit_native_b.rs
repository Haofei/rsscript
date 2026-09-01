//! JIT host helpers (part 2) + NativeState impl — impls/free-fns split from `reg_vm/mod.rs` for module-size partitioning.
//! All type definitions stay in mod.rs.

#[cfg(feature = "native-jit")]
use super::*;

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_set_insert_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value: i64,
) -> i64 {
    match ctx.with_journaled_map_write(handle, |map| {
        Some(i64::from(
            map.insert(jit_int_key(value), VmValue::Unit).is_none(),
        ))
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

// transactional heap mutation (heap-value collection write): insert a **heap** value (e.g. a `String`) into
// a `Set<HeapType>`. The value handle is resolved to its heap value and wrapped in
// `VmMapKey` — hashing/equality is the host's own canonical key, never re-implemented in
// native (a set is a map with `Unit` values, like [`rss_jit_set_insert_int`]). The write
// is journaled (the transaction rollback contract). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_set_insert_handle(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        value_handle: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_set_insert_handle_with_ctx(ctx, handle, value_handle)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_set_insert_handle_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value_handle: i64,
) -> i64 {
    let Some(key) = ctx.heap_read_handle(value_handle, |value| Some(VmMapKey::new(value.clone())))
    else {
        ctx.signal_bail();
        return 0;
    };
    match ctx.with_journaled_map_write(handle, move |map| {
        Some(i64::from(map.insert(key, VmValue::Unit).is_none()))
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
    extern "C" fn rss_jit_set_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
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
    extern "C" fn rss_jit_set_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
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
    extern "C" fn rss_jit_sorted_set_insert_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_sorted_set_insert_int_with_ctx(ctx, handle, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_sorted_set_insert_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value: i64,
) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        sorted_insert_vm(list.as_boxed_mut(), VmValue::Int(value))
            .ok()
            .map(i64::from)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

// transactional heap mutation (heap-value collection write): insert a **heap** value (e.g. `String`) into a
// sorted set — the heap analog of [`rss_jit_sorted_set_insert_int`]. The value handle is
// resolved and the host's own `sorted_insert_vm` (ordering + dedup) does the work; the
// write is journaled (the transactional fallback contract). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_sorted_set_insert_handle(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        value_handle: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(value) = ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
            ctx.signal_bail();
            return 0;
        };
        match ctx.with_journaled_list_write(handle, move |list| {
            sorted_insert_vm(list.as_boxed_mut(), value)
                .ok()
                .map(i64::from)
        }) {
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
    extern "C" fn rss_jit_sorted_set_contains_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        value: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match ctx.heap_read_handle(handle, |heap| match heap {
            VmValue::List(list) => Some(Rc::clone(list)),
            VmValue::Managed(inner) => match &*inner.borrow() {
                VmValue::List(list) => Some(Rc::clone(list)),
                _ => None,
            },
            _ => None,
        }) {
            Some(list) => match sorted_contains_vm(&list.borrow(), &VmValue::Int(value)) {
                Ok(found) => i64::from(found),
                Err(_) => {
                    ctx.signal_bail();
                    0
                }
            },
            None => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_sorted_set_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
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
pub(in crate::reg_vm) fn jit_sorted_map_entry_int(
    backing: &TypedVec,
    index: usize,
) -> Result<Option<(i64, i64)>, EvalError> {
    let Some(entry) = backing.get(index) else {
        return Ok(None);
    };
    let pair = expect_list_ref(&entry)?;
    let pair = pair.borrow();
    let entry_key = pair
        .first()
        .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing key.".to_string()))?;
    let entry_value = pair
        .get(1)
        .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing value.".to_string()))?;
    match (entry_key, entry_value) {
        (VmValue::Int(entry_key), VmValue::Int(entry_value)) => Ok(Some((entry_key, entry_value))),
        _ => Err(EvalError::Runtime(
            "reg VM SortedMap<Int, Int> native helper saw non-Int entry.".to_string(),
        )),
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_sorted_map_find_int(
    backing: &TypedVec,
    key: i64,
) -> Result<Option<(usize, i64)>, EvalError> {
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let Some((entry_key, entry_value)) = jit_sorted_map_entry_int(backing, mid)? else {
            return Err(EvalError::Runtime(
                "reg VM SortedMap entry missing during native lookup.".to_string(),
            ));
        };
        match entry_key.cmp(&key) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(Some((mid, entry_value))),
        }
    }
    Ok(None)
}

/// Int-key / Float-value sorted-map entry at `index` — the value-side mirror of
/// `jit_sorted_map_entry_int` (key still Int, value Float).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_sorted_map_entry_int_key_float(
    backing: &TypedVec,
    index: usize,
) -> Result<Option<(i64, f64)>, EvalError> {
    let Some(entry) = backing.get(index) else {
        return Ok(None);
    };
    let pair = expect_list_ref(&entry)?;
    let pair = pair.borrow();
    let entry_key = pair
        .first()
        .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing key.".to_string()))?;
    let entry_value = pair
        .get(1)
        .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing value.".to_string()))?;
    match (entry_key, entry_value) {
        (VmValue::Int(entry_key), VmValue::Float(entry_value)) => {
            Ok(Some((entry_key, entry_value)))
        }
        _ => Err(EvalError::Runtime(
            "reg VM SortedMap<Int, Float> native helper saw non-(Int,Float) entry.".to_string(),
        )),
    }
}

/// Binary search an Int-keyed sorted map for `key`, returning the Float value.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_sorted_map_find_int_key_float(
    backing: &TypedVec,
    key: i64,
) -> Result<Option<f64>, EvalError> {
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let Some((entry_key, entry_value)) = jit_sorted_map_entry_int_key_float(backing, mid)?
        else {
            return Err(EvalError::Runtime(
                "reg VM SortedMap entry missing during native lookup.".to_string(),
            ));
        };
        match entry_key.cmp(&key) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(Some(entry_value)),
        }
    }
    Ok(None)
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_sorted_map_insert_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        key: i64,
        value: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_sorted_map_insert_int_with_ctx(ctx, handle, key, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_sorted_map_insert_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    key: i64,
    value: i64,
) -> i64 {
    match ctx.with_journaled_list_write(handle, |list| {
        sorted_map_insert_in_place(list.as_boxed_mut(), VmValue::Int(key), VmValue::Int(value))
            .ok()?;
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

// transactional heap mutation (heap-key collection write): insert an `Int` value under a **heap** key (e.g.
// `String`) into a sorted map — the heap-key analog of [`rss_jit_sorted_map_insert_int`].
// The key handle is resolved and the host's own `sorted_map_insert_in_place` (ordering)
// does the work; the write is journaled (the transactional fallback contract). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_sorted_map_insert_handle_key_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        key_handle: i64,
        value: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(key) = ctx.heap_read_handle(key_handle, |key| Some(key.clone())) else {
            ctx.signal_bail();
            return 0;
        };
        match ctx.with_journaled_list_write(handle, move |list| {
            sorted_map_insert_in_place(list.as_boxed_mut(), key, VmValue::Int(value)).ok()?;
            Some(0)
        }) {
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
    extern "C" fn rss_jit_sorted_map_get_int(
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
        let Some(list) = ctx.heap_list_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        let backing = list.borrow();
        if let Some(cached) = JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
            let cache_value = *cache.borrow();
            let cache_value = cache_value.filter(|cache| cache.handle == handle)?;
            match jit_sorted_map_entry_int(&backing, cache_value.next_index) {
                Ok(Some((entry_key, entry_value))) if entry_key == key => {
                    cache.borrow_mut().replace(JitSortedMapScanCache {
                        handle,
                        next_index: cache_value.next_index.saturating_add(1),
                    });
                    Some(Ok(entry_value))
                }
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            }
        }) {
            return match cached {
                Ok(value) => {
                    *found = 1;
                    value
                }
                Err(_) => {
                    JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
                        cache.borrow_mut().take();
                    });
                    ctx.signal_bail();
                    0
                }
            };
        }
        match jit_sorted_map_find_int(&backing, key) {
            Ok(Some((index, value))) => {
                *found = 1;
                JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
                    cache.borrow_mut().replace(JitSortedMapScanCache {
                        handle,
                        next_index: index.saturating_add(1),
                    });
                });
                value
            }
            Ok(None) => {
                JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
                    cache.borrow_mut().take();
                });
                0
            }
            _ => {
                JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
                    cache.borrow_mut().take();
                });
                ctx.signal_bail();
                0
            }
        }
    }
}

// Int-key / Float-value sorted-map get (mirror of `rss_jit_sorted_map_get_int`),
// writing the same-call `found` output. Plain binary search — it omits the sequential
// scan cache (a perf-only fast path), so the result is identical. A non-Float value
// or wrong shape bails to the interpreter.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_sorted_map_get_float(
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
        let Some(list) = ctx.heap_list_handle(handle) else {
            ctx.signal_bail();
            return 0.0;
        };
        match jit_sorted_map_find_int_key_float(&list.borrow(), key) {
            Ok(Some(value)) => {
                *found = 1;
                value
            }
            Ok(None) => 0.0,
            Err(_) => {
                ctx.signal_bail();
                0.0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_sorted_map_contains_key_int(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        key: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(list) = ctx.heap_list_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        match jit_sorted_map_find_int(&list.borrow(), key) {
            Ok(found) => i64::from(found.is_some()),
            Err(_) => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_sorted_map_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
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
    extern "C" fn rss_jit_sorted_map_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(list) = ctx.heap_list_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        match i64::try_from(list.borrow().len()) {
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
    extern "C" fn rss_jit_deque_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(deque) = ctx.heap_deque_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        match i64::try_from(deque.borrow().len()) {
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
    extern "C" fn rss_jit_deque_is_empty(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(deque) = ctx.heap_deque_handle(handle) else {
            ctx.signal_bail();
            return 0;
        };
        i64::from(deque.borrow().is_empty())
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_deque_push_back_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_deque_push_back_int_with_ctx(ctx, handle, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_deque_push_back_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value: i64,
) -> i64 {
    match ctx.with_journaled_deque_write(handle, |deque| {
        deque.push_back(VmValue::Int(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

// transactional heap mutation (heap-value collection write): push a **heap** value onto the back of a
// `Deque<HeapType>` — the heap analog of [`rss_jit_deque_push_back_int`]. Resolves the
// value handle; the write is journaled (the transactional fallback contract). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_deque_push_back_handle(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        value_handle: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(value) = ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
            ctx.signal_bail();
            return 0;
        };
        match ctx.with_journaled_deque_write(handle, move |deque| {
            deque.push_back(value);
            Some(0)
        }) {
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
    extern "C" fn rss_jit_deque_push_back_float(_ctx: vm_jit::HostCtx, handle: i64, value: f64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_deque_push_back_float_with_ctx(ctx, handle, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_deque_push_back_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value: f64,
) -> i64 {
    match ctx.with_journaled_deque_write(handle, |deque| {
        deque.push_back(VmValue::Float(value));
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
    extern "C" fn rss_jit_deque_push_front_int(_ctx: vm_jit::HostCtx, handle: i64, value: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_deque_push_front_int_with_ctx(ctx, handle, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_deque_push_front_int_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value: i64,
) -> i64 {
    match ctx.with_journaled_deque_write(handle, |deque| {
        deque.push_front(VmValue::Int(value));
        Some(0)
    }) {
        Some(value) => value,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

// transactional heap mutation (heap-value collection write): push a **heap** value onto the front of a
// `Deque<HeapType>` — the heap analog of [`rss_jit_deque_push_front_int`]. Resolves the
// value handle; the write is journaled (the transactional fallback contract). A wrong shape/invalid handle bails.
#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_deque_push_front_handle(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        value_handle: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let Some(value) = ctx.heap_read_handle(value_handle, |value| Some(value.clone())) else {
            ctx.signal_bail();
            return 0;
        };
        match ctx.with_journaled_deque_write(handle, move |deque| {
            deque.push_front(value);
            Some(0)
        }) {
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
    extern "C" fn rss_jit_deque_push_front_float(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        value: f64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        rss_jit_deque_push_front_float_with_ctx(ctx, handle, value)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn rss_jit_deque_push_front_float_with_ctx(
    ctx: JitHostCallCtx,
    handle: i64,
    value: f64,
) -> i64 {
    match ctx.with_journaled_deque_write(handle, |deque| {
        deque.push_front(VmValue::Float(value));
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
pub(in crate::reg_vm) fn jit_deque_pop_int(
    ctx: JitHostCallCtx,
    handle: i64,
    pop: impl FnOnce(&mut VecDeque<VmValue>) -> Option<VmValue>,
) -> i64 {
    match ctx.with_journaled_deque_write(handle, pop) {
        Some(VmValue::Int(value)) => value,
        _ => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_deque_pop_front_int(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        jit_deque_pop_int(ctx, handle, VecDeque::pop_front)
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_deque_pop_back_int(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        jit_deque_pop_int(ctx, handle, VecDeque::pop_back)
    }
}

/// Float value-side mirror of `jit_deque_pop_int`: pop a `Float`; an empty deque or
/// non-Float element bails (the interpreter then runs the `None` path).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_deque_pop_float(
    ctx: JitHostCallCtx,
    handle: i64,
    pop: impl FnOnce(&mut VecDeque<VmValue>) -> Option<VmValue>,
) -> f64 {
    match ctx.with_journaled_deque_write(handle, pop) {
        Some(VmValue::Float(value)) => value,
        _ => {
            ctx.signal_bail();
            0.0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_deque_pop_front_float(_ctx: vm_jit::HostCtx, handle: i64) -> f64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0.0;
        };
        jit_deque_pop_float(ctx, handle, VecDeque::pop_front)
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_deque_pop_back_float(_ctx: vm_jit::HostCtx, handle: i64) -> f64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0.0;
        };
        jit_deque_pop_float(ctx, handle, VecDeque::pop_back)
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_field_float(_ctx: vm_jit::HostCtx, handle: i64, slot: i64) -> f64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0.0;
        };
        match usize::try_from(slot)
            .ok()
            .and_then(|slot| ctx.heap_read_handle(handle, |value| jit_struct_field_float(value, slot)))
        {
            Some(value) => value,
            None => {
                ctx.signal_bail();
                0.0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_get_float(_ctx: vm_jit::HostCtx, handle: i64, index: i64) -> f64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0.0;
        };
        let Some(index) = usize::try_from(index).ok() else {
            ctx.signal_bail();
            return 0.0;
        };
        let Some(list) = ctx.heap_list_handle(handle) else {
            ctx.signal_bail();
            return 0.0;
        };
        let borrowed = list.borrow();
        match &*borrowed {
            TypedVec::Floats(values) => match values.get(index) {
                Some(value) => *value,
                None => {
                    ctx.signal_bail();
                    0.0
                }
            },
            TypedVec::Boxed(values) => match values.get(index) {
                Some(VmValue::Float(value)) => *value,
                Some(_) | None => {
                    ctx.signal_bail();
                    0.0
                }
            },
            TypedVec::Ints(_) => {
                ctx.signal_bail();
                0.0
            }
        }
    }
}

/// The underlying function id of the closure behind `handle`, as `i64`. Used by the
/// profile-guided inlining monomorphic-inlining guard ([`vm_jit::JitInstr::GuardClosureId`]). Total: a
/// non-closure / invalid handle, or a function id too large for `i64`, returns `-1`,
/// which never equals a real (`>= 0`) callee id, so the guard simply bails. Never
/// signals the out-of-band bail flag.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_closure_function_id(value: &VmValue) -> Option<i64> {
    match value {
        VmValue::Closure(closure) => i64::try_from(closure.function).ok(),
        VmValue::Managed(inner) => jit_closure_function_id(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_closure_id(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            return -1;
        };
        ctx.heap_read(handle, jit_closure_function_id).unwrap_or(-1)
    }
}

/// The scalar bits of capture `index` of the closure behind `handle`, as `i64` (an
/// `Int` directly, a `Float` reinterpreted via [`f64::to_bits`], a `Bool` as 0/1).
/// Used by the capturing-closure inline support
/// ([`vm_jit::HostHelper::ClosureCapture`]) to materialize a scalar capture into the
/// inlined callee body. A non-scalar (heap) capture, an out-of-range index, or a
/// non-closure handle signals the out-of-band bail flag — defensive, since the
/// producer only emits `ClosureCapture` for captures it proved scalar.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_closure_capture_scalar(value: &VmValue, index: usize) -> Option<i64> {
    match value {
        VmValue::Closure(closure) => match closure.captures.get(index)? {
            VmValue::Int(v) => Some(*v),
            VmValue::Float(v) => Some(v.to_bits() as i64),
            VmValue::Bool(b) => Some(i64::from(*b)),
            _ => None,
        },
        VmValue::Managed(inner) => jit_closure_capture_scalar(&inner.borrow(), index),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_closure_capture(_ctx: vm_jit::HostCtx, handle: i64, index: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match usize::try_from(index)
            .ok()
            .and_then(|index| ctx.heap_read(handle, |value| jit_closure_capture_scalar(value, index)))
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
pub(in crate::reg_vm) fn jit_struct_field_closure_function_id(
    value: &VmValue,
    slot: usize,
) -> Option<i64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            jit_closure_function_id(data.fields.get(slot)?)
        }
        VmValue::Managed(inner) => jit_struct_field_closure_function_id(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_field_closure_id(_ctx: vm_jit::HostCtx, handle: i64, slot: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            return -1;
        };
        usize::try_from(slot)
            .ok()
            .and_then(|slot| {
                ctx.heap_read(handle, |value| {
                    jit_struct_field_closure_function_id(value, slot)
                })
            })
            .unwrap_or(-1)
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_struct_field_closure_capture_scalar(
    value: &VmValue,
    slot: usize,
    index: usize,
) -> Option<i64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            jit_closure_capture_scalar(data.fields.get(slot)?, index)
        }
        VmValue::Managed(inner) => {
            jit_struct_field_closure_capture_scalar(&inner.borrow(), slot, index)
        }
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_field_closure_capture(
        _ctx: vm_jit::HostCtx,
        handle: i64,
        slot: i64,
        index: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match usize::try_from(slot).ok().and_then(|slot| {
            usize::try_from(index).ok().and_then(|index| {
                ctx.heap_read(handle, |value| {
                    jit_struct_field_closure_capture_scalar(value, slot, index)
                })
            })
        }) {
            Some(value) => value,
            None => {
                ctx.signal_bail();
                0
            }
        }
    }
}

/// A clone of the struct/variant field `slot` IF it is itself a heap value (a
/// stored closure, struct, variant, or list) — the only fields a `FieldHandle`
/// read is allowed to fetch as a fresh handle. A scalar/absent field returns
/// `None` (→ bail), so a misclassified slot never produces a bogus handle.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_struct_field_heap_value(
    value: &VmValue,
    slot: usize,
) -> Option<VmValue> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            let field = data.fields.get(slot)?;
            jit_heap_value_clone(field)
        }
        VmValue::Managed(inner) => jit_struct_field_heap_value(&inner.borrow(), slot),
        _ => None,
    }
}

/// A clone of list element `index` IF it is itself a heap value (e.g. a struct
/// holding a stored closure). A scalar/absent element returns `None` (→ bail).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_list_get_heap_value(value: &VmValue, index: i64) -> Option<VmValue> {
    match value {
        VmValue::List(list) => {
            let index = usize::try_from(index).ok()?;
            let elem = list.borrow().get(index)?;
            jit_heap_value_clone(&elem)
        }
        VmValue::Managed(inner) => jit_list_get_heap_value(&inner.borrow(), index),
        _ => None,
    }
}

/// `Some(clone)` only when `value` is a heap value the host helpers can read
/// through a handle (closure/struct/variant/list, transparently unwrapping a
/// `Managed` cell). A scalar is `None`: handles only ever name heap values.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_heap_value_clone(value: &VmValue) -> Option<VmValue> {
    match value {
        VmValue::Closure(_)
        | VmValue::Struct(_)
        | VmValue::Variant(_)
        | VmValue::List(_)
        | VmValue::Managed(_) => Some(value.clone()),
        _ => None,
    }
}

/// Push a freshly-fetched heap value into the per-call handle table and return its
/// index, or signal the standard re-run-from-top bail (returning 0) when the field/
/// element was not a heap value the helper could fetch.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_push_heap_result_with_root_with_ctx(
    ctx: JitHostCallCtx,
    value: VmValue,
    root: Option<usize>,
) -> i64 {
    match ctx.push_heap_result(value, root) {
        Some(handle) => handle,
        None => {
            ctx.signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_from_int(_ctx: vm_jit::HostCtx, value: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        ctx.publish_heap_result(VmValue::String(Rc::new(value.to_string())))
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_string_len(value: &VmValue) -> Option<i64> {
    match value {
        VmValue::String(value) => i64::try_from(value.len()).ok(),
        VmValue::Managed(inner) => jit_string_len(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_string_clone(value: &VmValue) -> Option<Rc<String>> {
    match value {
        VmValue::String(value) => Some(Rc::clone(value)),
        VmValue::Managed(inner) => jit_string_clone(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
impl NativeState {
    pub(in crate::reg_vm) fn new_with_plan(plan: &NativeExecutionPlan) -> Result<Self, EvalError> {
        let _ = plan.allow_recursive_calls;
        let max_code_bytes = plan.admission.max_code_bytes;
        let max_compile_millis = plan.admission.max_compile_millis;
        let optimize_work_threshold = plan.admission.optimize_work_threshold;
        // A zero threshold is the explicit eager/benchmark mode. Preserve the
        // pre-ladder contract by compiling its only tier at `speed`; compiling
        // baseline and optimized copies on every fresh evaluation doubles cold
        // compile cost and can make helper-heavy kernels run in baseline code.
        let eager_optimized = plan.tier_up_threshold == 0 && !plan.baseline;
        let executable_memory_budget = vm_jit::ExecutableMemoryBudget::new(max_code_bytes);
        let has_optimized_module = !plan.baseline && !eager_optimized;
        let baseline_arena_bytes = if has_optimized_module {
            max_code_bytes / 2
        } else {
            max_code_bytes
        };
        let optimized_arena_bytes = max_code_bytes.saturating_sub(baseline_arena_bytes);
        Ok(Self {
            verified_facts: None,
            baseline_module: LazyNativeModule::new(
                jit_host_helpers(),
                !eager_optimized,
                executable_memory_budget.clone(),
                baseline_arena_bytes,
            ),
            optimized_module: if plan.baseline || eager_optimized {
                None
            } else {
                Some(LazyNativeModule::new(
                    jit_host_helpers(),
                    false,
                    executable_memory_budget.clone(),
                    optimized_arena_bytes,
                ))
            },
            executable_memory_budget,
            admission: NativeAdmissionBudget {
                max_code_bytes,
                max_compile_nanos: u128::from(max_compile_millis) * 1_000_000,
                admitted_code_bytes: 0,
                compile_nanos: 0,
                code_exhausted: false,
            },
            cache: HashMap::new(),
            optimized_cache: HashMap::new(),
            optimization_sources: HashMap::new(),
            counts: HashMap::new(),
            whole_controllers: HashMap::new(),
            optimize_work_threshold,
            noamortize_counts: HashMap::new(),
            tier_up_threshold: plan.tier_up_threshold,
            force_bail: plan.force_bail,
            forced_safepoint: plan.forced_safepoint,
            force_all_safepoints: plan.force_all_safepoints,
            cost_model: plan.cost_model,
            osr_work_threshold: plan.osr_work_threshold,
            stats: NativeStats::default(),
            collect_stats: plan.collect_stats,
            precise_deopt: plan.precise_deopt,
            auto_osr_enabled: plan.auto_osr_enabled,
            eager_osr: plan.eager_osr,
            osr_candidates: HashMap::new(),
            osr_triggers: HashMap::new(),
            osr_cache: HashMap::new(),
            optimized_osr_cache: HashMap::new(),
            osr_optimization_sources: HashMap::new(),
            osr_controllers: HashMap::new(),
            continuation_cache: HashMap::new(),
            continuation_controllers: HashMap::new(),
            continuation_plans: HashMap::new(),
            continuation_entry_sets: HashMap::new(),
            continuation_functions: HashMap::new(),
            scratch_args: Vec::new(),
            scratch_lens: Vec::new(),
            scratch_flat_owned: Vec::new(),
            scratch_flat_mut_owned: Vec::new(),
            scratch_heap_input_slots: Vec::new(),
            scratch_osr_window: Vec::new(),
            scratch_osr_lens: Vec::new(),
            scratch_osr_flat_owned: Vec::new(),
            scratch_osr_flat_mut_owned: Vec::new(),
            scratch_osr_flat_slots: Vec::new(),
            scratch_osr_flat_mut_slots: Vec::new(),
            scratch_osr_heap_input_slots: Vec::new(),
            call_session: vm_jit::NativeCallSession::new(),
            report: plan.report,
            report_native_ok: std::collections::HashSet::new(),
            report_osr_ok: std::collections::HashSet::new(),
            osr_dynamic_bail: false,
        })
    }

    /// Record a consecutive failure against one shape version. Reaching the
    /// threshold negative-caches only that version; invariant translation
    /// failures remain the sole owner of evaluation-local JIT native status.
    pub(in crate::reg_vm) fn record_bail(&mut self, version_key: &NativeVersionKey) {
        let disabled = self
            .whole_controllers
            .entry(version_key.clone())
            .or_default()
            .dynamic_bail(NATIVE_BAIL_GIVEUP_THRESHOLD);
        if self.collect_stats {
            self.stats.shape_bails += 1;
        }
        if disabled {
            self.cache.insert(version_key.clone(), None);
            self.optimized_cache.remove(version_key);
            self.optimization_sources.remove(version_key);
        }
    }

    pub(in crate::reg_vm) fn whole_shape_count(&self, instance: &JitInstanceKey) -> usize {
        self.cache
            .keys()
            .filter(|key| &key.instance == instance)
            .count()
    }

    pub(in crate::reg_vm) fn whole_instance_count(&self, function: usize) -> usize {
        self.cache
            .keys()
            .filter(|key| key.instance.function == function)
            .map(|key| &key.instance.type_arguments)
            .collect::<HashSet<_>>()
            .len()
    }

    pub(in crate::reg_vm) fn has_whole_instance(&self, instance: &JitInstanceKey) -> bool {
        self.cache.keys().any(|key| &key.instance == instance)
    }

    pub(in crate::reg_vm) fn osr_shape_count(
        &self,
        region: RegionKey,
        type_arguments: &VerifiedTypeArgsKey,
    ) -> usize {
        self.osr_cache
            .keys()
            .filter(|key| key.region == region && &key.type_arguments == type_arguments)
            .count()
    }

    pub(in crate::reg_vm) fn osr_instance_count(&self, region: RegionKey) -> usize {
        self.osr_cache
            .keys()
            .filter(|key| key.region == region)
            .map(|key| &key.type_arguments)
            .collect::<HashSet<_>>()
            .len()
    }

    pub(in crate::reg_vm) fn has_osr_instance(
        &self,
        region: RegionKey,
        type_arguments: &VerifiedTypeArgsKey,
    ) -> bool {
        self.osr_cache
            .keys()
            .any(|key| key.region == region && &key.type_arguments == type_arguments)
    }

    pub(in crate::reg_vm) fn continuation_instance_count(
        &self,
        function: usize,
        entry: usize,
    ) -> usize {
        self.continuation_cache
            .keys()
            .filter(|key| key.instance.function == function && key.entry == entry)
            .map(|key| &key.instance.type_arguments)
            .collect::<HashSet<_>>()
            .len()
    }

    pub(in crate::reg_vm) fn has_continuation_instance(
        &self,
        instance: &JitInstanceKey,
        entry: usize,
    ) -> bool {
        self.continuation_cache
            .keys()
            .any(|key| &key.instance == instance && key.entry == entry)
    }
}
