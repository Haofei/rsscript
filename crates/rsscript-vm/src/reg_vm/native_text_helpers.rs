//! String, bytes, JSON, and handle-access Host Helpers.

use super::*;

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match ctx.heap_read_handle(handle, jit_string_len) {
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
    extern "C" fn rss_jit_string_concat(_ctx: vm_jit::HostCtx, left: i64, right: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let left = ctx.heap_read_handle(left, jit_string_clone);
        let right = ctx.heap_read_handle(right, jit_string_clone);
        match (left, right) {
            (Some(left), Some(right)) => {
                let mut value = String::with_capacity(left.len() + right.len());
                value.push_str(&left);
                value.push_str(&right);
                ctx.publish_heap_result(VmValue::string(value))
            }
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_slice(_ctx: vm_jit::HostCtx, value: i64, start: i64, len: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match ctx.heap_read_handle(value, jit_string_clone) {
            Some(value) => {
                ctx.publish_heap_result(VmValue::string(string_slice_range(&value, start, len)))
            }
            None => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_pad_left(
        _ctx: vm_jit::HostCtx,
        value: i64,
        width: i64,
        fill: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = ctx.heap_read_handle(value, jit_string_clone);
        let fill = ctx.heap_read_handle(fill, jit_string_clone);
        match (value, fill) {
            (Some(value), Some(fill)) => {
                ctx.publish_heap_result(VmValue::string(string_pad(&value, width, &fill, true)))
            }
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_pad_left_len(
        _ctx: vm_jit::HostCtx,
        value: i64,
        width: i64,
        fill: i64,
    ) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = ctx.heap_read_handle(value, jit_string_clone);
        let fill = ctx.heap_read_handle(fill, jit_string_clone);
        match (value, fill) {
            (Some(value), Some(fill)) => match string_pad_len(&value, width, &fill) {
                Some(len) => len,
                None => {
                    ctx.signal_bail();
                    0
                }
            },
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_split(_ctx: vm_jit::HostCtx, value: i64, delimiter: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = ctx.heap_read_handle(value, jit_string_clone);
        let delimiter = ctx.heap_read_handle(delimiter, jit_string_clone);
        match (value, delimiter) {
            (Some(value), Some(delimiter)) => {
                let parts = value
                    .split(delimiter.as_str())
                    .map(VmValue::string)
                    .collect::<Vec<_>>();
                ctx.publish_heap_result(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    parts,
                )))))
            }
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_starts_with(_ctx: vm_jit::HostCtx, value: i64, prefix: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = ctx.heap_read_handle(value, jit_string_clone);
        let prefix = ctx.heap_read_handle(prefix, jit_string_clone);
        match (value, prefix) {
            (Some(value), Some(prefix)) => i64::from(value.starts_with(prefix.as_str())),
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_split_count(_ctx: vm_jit::HostCtx, value: i64, delimiter: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = ctx.heap_read_handle(value, jit_string_clone);
        let delimiter = ctx.heap_read_handle(delimiter, jit_string_clone);
        match (value, delimiter) {
            (Some(value), Some(delimiter)) => {
                match i64::try_from(value.split(delimiter.as_str()).count()) {
                    Ok(count) => count,
                    Err(_) => {
                        ctx.signal_bail();
                        0
                    }
                }
            }
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_string_literal(_ctx: vm_jit::HostCtx, literal_id: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = usize::try_from(literal_id)
            .ok()
            .and_then(|index| JIT_STRING_LITERALS.with(|table| table.borrow().get(index).cloned()));
        match value {
            Some(value) => ctx.publish_heap_result(VmValue::String(value)),
            None => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
fn jit_bytes_len(value: &VmValue) -> Option<i64> {
    match value {
        VmValue::Bytes(value) => i64::try_from(value.len()).ok(),
        VmValue::Managed(inner) => jit_bytes_len(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_bytes_clone(value: &VmValue) -> Option<Rc<Vec<u8>>> {
    match value {
        VmValue::Bytes(value) => Some(Rc::clone(value)),
        VmValue::Managed(inner) => jit_bytes_clone(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_bytes_len(_ctx: vm_jit::HostCtx, handle: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match ctx.heap_read_handle(handle, jit_bytes_len) {
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
    extern "C" fn rss_jit_bytes_slice(_ctx: vm_jit::HostCtx, handle: i64, start: i64, len: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match ctx.heap_read_handle(handle, jit_bytes_clone) {
            Some(value) => {
                ctx.publish_heap_result(VmValue::Bytes(Rc::new(bytes_slice(&value, start, len))))
            }
            None => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
fn jit_json_clone(value: &VmValue) -> Option<Rc<crate::serde_json::Value>> {
    match value {
        VmValue::Json(value) => Some(Rc::clone(value)),
        VmValue::Managed(inner) => jit_json_clone(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_json_parse(_ctx: vm_jit::HostCtx, text: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        match ctx
            .heap_read_handle(text, jit_string_clone)
            .and_then(|text| crate::serde_json::from_str::<crate::serde_json::Value>(&text).ok())
        {
            Some(value) => ctx.publish_heap_result(VmValue::Json(Rc::new(value))),
            None => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_json_field(_ctx: vm_jit::HostCtx, value: i64, name: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = ctx.heap_read_handle(value, jit_json_clone);
        let name = ctx.heap_read_handle(name, jit_string_clone);
        match (value, name) {
            (Some(value), Some(name)) => match value.as_object().and_then(|obj| obj.get(name.as_str()))
            {
                Some(field) => ctx.publish_heap_result(VmValue::Json(Rc::new(field.clone()))),
                None => {
                    ctx.signal_bail();
                    0
                }
            },
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_json_field_int(_ctx: vm_jit::HostCtx, value: i64, name: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = ctx.heap_read_handle(value, jit_json_clone);
        let name = ctx.heap_read_handle(name, jit_string_clone);
        match (value, name) {
            (Some(value), Some(name)) => match value
                .as_object()
                .and_then(|obj| obj.get(name.as_str()))
                .and_then(crate::serde_json::Value::as_i64)
            {
                Some(value) => value,
                None => {
                    ctx.signal_bail();
                    0
                }
            },
            _ => {
                ctx.signal_bail();
                0
            }
        }
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_field_handle(_ctx: vm_jit::HostCtx, handle: i64, slot: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = usize::try_from(slot).ok().and_then(|slot| {
            ctx.heap_read_handle(handle, |value| jit_struct_field_heap_value(value, slot))
        });
        ctx.publish_heap_handle(value)
    }
}

#[cfg(feature = "native-jit")]
jit_host_boundary! {
    extern "C" fn rss_jit_list_get_handle(_ctx: vm_jit::HostCtx, handle: i64, index: i64) -> i64 {
        let Some(ctx) = JitHostCallCtx::from_token(_ctx) else {
            vm_jit::signal_bail(_ctx);
            return 0;
        };
        let value = ctx.heap_read_handle(handle, |value| jit_list_get_heap_value(value, index));
        ctx.publish_heap_handle(value)
    }
}
