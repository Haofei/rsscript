//! Shared ABI between the RSScript host (interpreter/VM) and dynamically loaded
//! native plugin cdylibs.
//!
//! Both sides depend on this crate so they agree on the in-memory layout of
//! [`NativeValue`] and the plugin registry types. Because the values are passed
//! as ordinary Rust types across the `cdylib` boundary, host and plugin must be
//! built with the same Rust toolchain (always true within this workspace).

use std::collections::BTreeMap;
use std::os::raw::c_char;

/// A value crossing the native boundary, in both directions. This is the bridge
/// representation the VM converts to/from its internal `VmValue`, and the shape a
/// plugin shim converts to/from the native crate's concrete Rust types.
#[derive(Debug, Clone, PartialEq)]
pub enum NativeValue {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    Bytes(Vec<u8>),
    List(Vec<NativeValue>),
    Map(Vec<(NativeValue, NativeValue)>),
    Json(serde_json::Value),
    Struct {
        name: String,
        fields: BTreeMap<String, NativeValue>,
    },
    Variant {
        name: String,
        fields: BTreeMap<String, NativeValue>,
    },
    Native {
        type_name: String,
        id: i64,
    },
}

/// A host-callable native function: takes positional argument values and returns
/// either a value or an error message.
pub type NativeInterpreterFn = fn(Vec<NativeValue>) -> Result<NativeValue, String>;

/// One entry in a plugin's registry: the RSScript binding symbol (e.g.
/// `"SqlxFfi.execute"`) and the function implementing it.
#[repr(C)]
pub struct NativeBindingEntry {
    /// NUL-terminated UTF-8 symbol name. Points to data owned by the plugin and
    /// valid for the lifetime of the loaded library.
    pub name: *const c_char,
    pub func: NativeInterpreterFn,
}

/// The table a plugin returns from its registry entry point. `entries` points to
/// `len` contiguous [`NativeBindingEntry`] values owned by the plugin.
#[repr(C)]
pub struct NativeRegistry {
    pub entries: *const NativeBindingEntry,
    pub len: usize,
}

/// Symbol name the host resolves in a plugin cdylib. The symbol must have the
/// signature `unsafe extern "C" fn() -> NativeRegistry`.
pub const REGISTRY_SYMBOL: &str = "rss_native_registry";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_symbol_is_stable() {
        // The generated shim exports exactly this symbol; keep them in lockstep.
        assert_eq!(REGISTRY_SYMBOL, "rss_native_registry");
    }

    #[test]
    fn native_values_clone_and_compare_structurally() {
        let list = NativeValue::List(vec![
            NativeValue::Int(1),
            NativeValue::String("a".to_string()),
        ]);
        assert_eq!(list.clone(), list);

        let mut fields = BTreeMap::new();
        fields.insert("value".to_string(), NativeValue::Int(7));
        let some = NativeValue::Variant {
            name: "Some".to_string(),
            fields,
        };
        assert_eq!(some.clone(), some);
        assert_ne!(some, NativeValue::Unit);
    }
}

/// Host-side loader for plugin cdylibs. Lives here (rather than in the host crate)
/// because the `dlopen` boundary is inherently `unsafe`, which the host crate
/// forbids. Gated behind the `host` feature so plugins don't compile libloading.
#[cfg(feature = "host")]
pub fn load_registry(
    library_path: &std::path::Path,
) -> Result<Vec<(String, NativeInterpreterFn)>, String> {
    // SAFETY: the shim is a trusted, freshly built cdylib that exports the agreed
    // `rss_native_registry` symbol returning a `NativeRegistry` of entries that
    // live for the lifetime of the (leaked) library.
    unsafe {
        let library = libloading::Library::new(library_path)
            .map_err(|error| format!("failed to load native shim library: {error}"))?;
        let registry_fn: libloading::Symbol<unsafe extern "C" fn() -> NativeRegistry> = library
            .get(REGISTRY_SYMBOL.as_bytes())
            .map_err(|error| format!("native shim is missing `{REGISTRY_SYMBOL}`: {error}"))?;
        let registry = registry_fn();
        let entries = std::slice::from_raw_parts(registry.entries, registry.len);
        let mut bindings = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = std::ffi::CStr::from_ptr(entry.name)
                .to_str()
                .map_err(|error| format!("native shim binding name was not UTF-8: {error}"))?
                .to_string();
            bindings.push((name, entry.func));
        }
        // Keep the library mapped for the rest of the process.
        std::mem::forget(library);
        Ok(bindings)
    }
}
