//! Stable byte-buffer ABI between the RSScript host and native plugin shims.
//!
//! Rust-owned containers never cross the dynamic-library boundary. Calls use
//! JSON bytes over a versioned C ABI; each side allocates and frees its own
//! buffers. The host retains the loaded library for as long as any callable
//! binding exists.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub const ABI_MAGIC: u64 = 0x5253_534E_4154_4956;
pub const ABI_VERSION: u32 = 1;
#[cfg(feature = "host")]
const MAX_REGISTRY_ENTRIES: usize = 1_000_000;
#[cfg(feature = "host")]
const MAX_BINDING_NAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

pub type NativeHostFn = fn(Vec<NativeValue>) -> Result<NativeValue, String>;

/// Cloneable host callable. Dynamic bindings retain their owning library in the
/// closure, while in-process tests can wrap an ordinary Rust function.
#[derive(Clone)]
pub struct NativeInterpreterFn {
    inner: Arc<dyn Fn(Vec<NativeValue>) -> Result<NativeValue, String> + Send + Sync>,
}

impl NativeInterpreterFn {
    pub fn from_fn(function: NativeHostFn) -> Self {
        Self::new(function)
    }

    pub fn new(
        function: impl Fn(Vec<NativeValue>) -> Result<NativeValue, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(function),
        }
    }

    pub fn call(&self, args: Vec<NativeValue>) -> Result<NativeValue, String> {
        (self.inner)(args)
    }
}

impl From<NativeHostFn> for NativeInterpreterFn {
    fn from(function: NativeHostFn) -> Self {
        Self::from_fn(function)
    }
}

#[repr(C)]
pub struct NativeBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl NativeBuffer {
    pub const fn empty() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }
}

pub type NativeAbiFn = unsafe extern "C" fn(*const u8, usize, *mut NativeBuffer) -> i32;
pub type NativeFreeBufferFn = unsafe extern "C" fn(NativeBuffer);

#[repr(C)]
pub struct NativeBindingEntry {
    pub name: *const u8,
    pub name_len: usize,
    pub func: NativeAbiFn,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NativeRegistry {
    pub magic: u64,
    pub abi_version: u32,
    pub struct_size: u32,
    pub entries: *const NativeBindingEntry,
    pub len: usize,
    pub free_buffer: NativeFreeBufferFn,
}

pub const REGISTRY_SYMBOL: &str = "rss_native_registry";

/// Plugin-side dispatcher used by generated ABI wrappers.
///
/// # Safety
/// `input` must point to `input_len` readable bytes and `output` must point to a
/// writable `NativeBuffer` owned by the caller.
pub unsafe fn dispatch_serialized(
    input: *const u8,
    input_len: usize,
    output: *mut NativeBuffer,
    function: NativeHostFn,
) -> i32 {
    if output.is_null() || (input.is_null() && input_len != 0) {
        return 2;
    }
    let input = if input_len == 0 {
        &[]
    } else {
        // SAFETY: validated above and required by this function's contract.
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };
    let result = std::panic::catch_unwind(|| {
        serde_json::from_slice::<Vec<NativeValue>>(input)
            .map_err(|error| format!("invalid native call payload: {error}"))
            .and_then(function)
    })
    .unwrap_or_else(|_| Err("native binding panicked".to_string()));
    let mut bytes = match serde_json::to_vec(&result) {
        Ok(bytes) => bytes,
        Err(_) => return 3,
    };
    let buffer = NativeBuffer {
        ptr: bytes.as_mut_ptr(),
        len: bytes.len(),
        capacity: bytes.capacity(),
    };
    std::mem::forget(bytes);
    // SAFETY: `output` is writable by contract.
    unsafe { output.write(buffer) };
    0
}

/// Release a buffer allocated by [`dispatch_serialized`] in the plugin.
///
/// # Safety
/// The buffer must have been returned by this exact dynamic library and must not
/// have been freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rss_native_free_buffer(buffer: NativeBuffer) {
    if buffer.ptr.is_null() {
        return;
    }
    // SAFETY: guaranteed by the function contract.
    drop(unsafe { Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.capacity) });
}

#[cfg(feature = "host")]
pub fn load_registry(
    library_path: &std::path::Path,
) -> Result<Vec<(String, NativeInterpreterFn)>, String> {
    let library = load_library_once(library_path)?;
    // SAFETY: the symbol is copied immediately while `library` remains owned.
    let registry_fn = unsafe {
        *library
            .get::<unsafe extern "C" fn() -> NativeRegistry>(REGISTRY_SYMBOL.as_bytes())
            .map_err(|error| format!("native shim is missing `{REGISTRY_SYMBOL}`: {error}"))?
    };
    // SAFETY: the generated registry function has no preconditions.
    let registry = unsafe { registry_fn() };
    validate_registry(&registry)?;
    // SAFETY: registry validation checked nullness and bounded the length.
    let entries = unsafe { std::slice::from_raw_parts(registry.entries, registry.len) };
    let mut bindings = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.name.is_null() || entry.name_len > MAX_BINDING_NAME_BYTES {
            return Err("native shim contains an invalid binding name".to_string());
        }
        // SAFETY: generated entries point to static UTF-8 bytes in the library.
        let name = unsafe { std::slice::from_raw_parts(entry.name, entry.name_len) };
        let name = std::str::from_utf8(name)
            .map_err(|error| format!("native shim binding name was not UTF-8: {error}"))?
            .to_string();
        let function = entry.func;
        let free_buffer = registry.free_buffer;
        let owner = Arc::clone(&library);
        let callable = NativeInterpreterFn::new(move |args| {
            let _owner = &owner;
            let input = serde_json::to_vec(&args)
                .map_err(|error| format!("failed to encode native arguments: {error}"))?;
            let mut output = NativeBuffer::empty();
            // SAFETY: input and output pointers remain valid for the call and the
            // function belongs to the retained library.
            let status = unsafe { function(input.as_ptr(), input.len(), &mut output) };
            if status != 0 {
                return Err(format!("native ABI call failed with status {status}"));
            }
            if output.ptr.is_null() && output.len != 0 {
                return Err("native ABI returned an invalid output buffer".to_string());
            }
            let bytes = if output.len == 0 {
                Vec::new()
            } else {
                // SAFETY: output belongs to the plugin until `free_buffer` below.
                unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec()
            };
            // SAFETY: this is the matching allocator-side release function.
            unsafe { free_buffer(output) };
            serde_json::from_slice::<Result<NativeValue, String>>(&bytes)
                .map_err(|error| format!("invalid native result payload: {error}"))?
        });
        bindings.push((name, callable));
    }
    Ok(bindings)
}

#[cfg(feature = "host")]
fn load_library_once(path: &std::path::Path) -> Result<Arc<libloading::Library>, String> {
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, Weak};

    static LIBRARIES: OnceLock<Mutex<HashMap<String, Weak<libloading::Library>>>> = OnceLock::new();
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize native library: {error}"))?;
    let bytes = std::fs::read(&canonical)
        .map_err(|error| format!("failed to hash native library: {error}"))?;
    let key = format!(
        "{}:{}",
        canonical.display(),
        hex_digest(Sha256::digest(bytes))
    );
    let cache = LIBRARIES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .map_err(|_| "native library cache lock was poisoned".to_string())?;
    cache.retain(|_, library| library.strong_count() != 0);
    if let Some(library) = cache.get(&key).and_then(Weak::upgrade) {
        return Ok(library);
    }
    // SAFETY: symbols and registry metadata are validated before use.
    let library = Arc::new(
        unsafe { libloading::Library::new(&canonical) }
            .map_err(|error| format!("failed to load native shim library: {error}"))?,
    );
    cache.insert(key, Arc::downgrade(&library));
    Ok(library)
}

#[cfg(feature = "host")]
fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(feature = "host")]
fn validate_registry(registry: &NativeRegistry) -> Result<(), String> {
    if registry.magic != ABI_MAGIC {
        return Err("native shim ABI magic mismatch".to_string());
    }
    if registry.abi_version != ABI_VERSION {
        return Err(format!(
            "native shim ABI version mismatch: host {}, plugin {}",
            ABI_VERSION, registry.abi_version
        ));
    }
    if registry.struct_size as usize != std::mem::size_of::<NativeRegistry>() {
        return Err("native shim registry size mismatch".to_string());
    }
    if registry.len > MAX_REGISTRY_ENTRIES || (registry.entries.is_null() && registry.len != 0) {
        return Err("native shim registry entries are invalid".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_symbol_and_version_are_stable() {
        assert_eq!(REGISTRY_SYMBOL, "rss_native_registry");
        assert_eq!(ABI_VERSION, 1);
    }

    #[test]
    fn native_values_serialize_round_trip() {
        let value = NativeValue::List(vec![
            NativeValue::Int(1),
            NativeValue::String("a".to_string()),
        ]);
        let encoded = serde_json::to_vec(&value).expect("value serializes");
        let decoded: NativeValue = serde_json::from_slice(&encoded).expect("value parses");
        assert_eq!(decoded, value);
    }

    #[test]
    fn in_process_callable_wraps_rust_function() {
        fn echo(mut args: Vec<NativeValue>) -> Result<NativeValue, String> {
            Ok(args.remove(0))
        }
        let callable = NativeInterpreterFn::from(echo as NativeHostFn);
        assert_eq!(
            callable.call(vec![NativeValue::Int(7)]),
            Ok(NativeValue::Int(7))
        );
    }

    #[test]
    fn serialized_dispatch_round_trips_and_uses_plugin_free() {
        fn echo(mut args: Vec<NativeValue>) -> Result<NativeValue, String> {
            Ok(args.remove(0))
        }
        let input = serde_json::to_vec(&vec![NativeValue::Int(9)]).expect("args serialize");
        let mut output = NativeBuffer::empty();
        // SAFETY: pointers refer to live input/output values for the call.
        assert_eq!(
            unsafe {
                dispatch_serialized(
                    input.as_ptr(),
                    input.len(),
                    &mut output,
                    echo as NativeHostFn,
                )
            },
            0
        );
        // SAFETY: the dispatcher initialized this buffer.
        let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) };
        let result: Result<NativeValue, String> =
            serde_json::from_slice(bytes).expect("result parses");
        assert_eq!(result, Ok(NativeValue::Int(9)));
        // SAFETY: this is the matching release function and is called once.
        unsafe { rss_native_free_buffer(output) };
    }

    #[cfg(feature = "host")]
    #[test]
    fn registry_validation_rejects_magic_version_and_size_mismatch() {
        unsafe extern "C" fn free(_: NativeBuffer) {}
        let valid = NativeRegistry {
            magic: ABI_MAGIC,
            abi_version: ABI_VERSION,
            struct_size: std::mem::size_of::<NativeRegistry>() as u32,
            entries: std::ptr::null(),
            len: 0,
            free_buffer: free,
        };
        assert!(validate_registry(&valid).is_ok());
        let wrong_magic = NativeRegistry { magic: 0, ..valid };
        assert!(validate_registry(&wrong_magic).is_err());
        let wrong_version = NativeRegistry {
            abi_version: ABI_VERSION + 1,
            ..valid
        };
        assert!(validate_registry(&wrong_version).is_err());
        let wrong_size = NativeRegistry {
            struct_size: 0,
            ..valid
        };
        assert!(validate_registry(&wrong_size).is_err());
    }
}
