//! Stable byte-buffer ABI between the RSScript host and native plugin shims.
//!
//! Rust-owned containers never cross the dynamic-library boundary. Calls use
//! JSON bytes over a versioned C ABI; each side allocates and frees its own
//! buffers. The host retains the loaded library for as long as any callable
//! binding exists.
//!
//! # Trust boundary
//!
//! In-process native plugins are trusted-only. The host validates buffer
//! nullness, lengths, capacities, and configured byte limits before reading
//! plugin memory, but an address that is non-null and numerically well-formed
//! can still be dangling or otherwise unreadable. Safely containing arbitrary
//! native pointers requires process isolation and IPC, not additional checks in
//! this ABI.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub const ABI_MAGIC: u64 = 0x5253_534E_4154_4956;
pub const ABI_VERSION: u32 = 1;
pub const MAX_NATIVE_REQUEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_NATIVE_RESULT_BYTES: usize = 16 * 1024 * 1024;
#[cfg(feature = "host")]
const MAX_REGISTRY_ENTRIES: usize = 16 * 1024;
#[cfg(feature = "host")]
const MAX_BINDING_NAME_BYTES: usize = 1024;

enum BoundedJsonError {
    LimitExceeded,
    Serialize(serde_json::Error),
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    limit_exceeded: bool,
}

impl std::io::Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|next_len| next_len > self.max_bytes)
        {
            self.limit_exceeded = true;
            return Err(std::io::Error::other("JSON byte limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn to_json_bounded<T: Serialize>(value: &T, max_bytes: usize) -> Result<Vec<u8>, BoundedJsonError> {
    let mut writer = BoundedJsonWriter {
        bytes: Vec::new(),
        max_bytes,
        limit_exceeded: false,
    };
    match serde_json::to_writer(&mut writer, value) {
        Ok(()) => Ok(writer.bytes),
        Err(_) if writer.limit_exceeded => Err(BoundedJsonError::LimitExceeded),
        Err(error) => Err(BoundedJsonError::Serialize(error)),
    }
}

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

#[cfg(feature = "host")]
fn validate_native_buffer(buffer: &NativeBuffer, max_bytes: usize) -> Result<(), String> {
    if buffer.ptr.is_null() {
        if buffer.len == 0 && buffer.capacity == 0 {
            return Ok(());
        }
        return Err(
            "native ABI returned a null buffer with non-zero length or capacity".to_string(),
        );
    }
    if buffer.capacity == 0 {
        return Err("native ABI returned a non-null buffer with zero capacity".to_string());
    }
    if buffer.len > buffer.capacity {
        return Err("native ABI returned a buffer whose length exceeds its capacity".to_string());
    }
    if buffer.len > max_bytes || buffer.capacity > max_bytes {
        return Err(format!(
            "native ABI returned a buffer exceeding the {max_bytes} byte limit"
        ));
    }
    Ok(())
}

pub type NativeAbiFn = unsafe extern "C" fn(*const u8, usize, *mut NativeBuffer) -> i32;
pub type NativeFreeBufferFn = unsafe extern "C" fn(NativeBuffer);

#[repr(C)]
#[derive(Clone, Copy)]
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
    // Keep every failure path deterministic for callers that release output
    // unconditionally.
    unsafe { output.write(NativeBuffer::empty()) };
    if input_len > MAX_NATIVE_REQUEST_BYTES {
        return 4;
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
    let mut bytes = match to_json_bounded(&result, MAX_NATIVE_RESULT_BYTES) {
        Ok(bytes) => bytes,
        Err(BoundedJsonError::LimitExceeded) => return 4,
        Err(BoundedJsonError::Serialize(error)) => {
            drop(error);
            return 3;
        }
    };
    if bytes.capacity() > MAX_NATIVE_RESULT_BYTES {
        bytes.shrink_to_fit();
        if bytes.capacity() > MAX_NATIVE_RESULT_BYTES {
            return 4;
        }
    }
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
    if buffer.capacity == 0 || buffer.len > buffer.capacity || buffer.capacity > isize::MAX as usize
    {
        return;
    }
    // SAFETY: guaranteed by the function contract.
    drop(unsafe { Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.capacity) });
}

#[cfg(feature = "host")]
struct OwnedNativeBuffer {
    buffer: Option<NativeBuffer>,
    free_buffer: NativeFreeBufferFn,
}

#[cfg(feature = "host")]
impl OwnedNativeBuffer {
    fn new(buffer: NativeBuffer, free_buffer: NativeFreeBufferFn) -> Self {
        Self {
            buffer: Some(buffer),
            free_buffer,
        }
    }

    fn validated_bytes(&self) -> Result<&[u8], String> {
        let buffer = self.buffer.as_ref().expect("owned buffer is present");
        validate_native_buffer(buffer, MAX_NATIVE_RESULT_BYTES)?;
        if buffer.len == 0 {
            return Ok(&[]);
        }
        // SAFETY: the trusted plugin owns this allocation until `Drop`, and
        // its structural metadata was validated above.
        Ok(unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len) })
    }
}

#[cfg(feature = "host")]
impl Drop for OwnedNativeBuffer {
    fn drop(&mut self) {
        let Some(buffer) = self.buffer.take() else {
            return;
        };
        // SAFETY: the plugin supplied both the buffer and its matching release
        // function. The library owner outlives this guard.
        unsafe { (self.free_buffer)(buffer) };
    }
}

#[cfg(feature = "host")]
fn call_native(
    function: NativeAbiFn,
    free_buffer: NativeFreeBufferFn,
    args: Vec<NativeValue>,
) -> Result<NativeValue, String> {
    let input = match to_json_bounded(&args, MAX_NATIVE_REQUEST_BYTES) {
        Ok(input) => input,
        Err(BoundedJsonError::LimitExceeded) => {
            return Err(format!(
                "native ABI request exceeds the {MAX_NATIVE_REQUEST_BYTES} byte limit"
            ));
        }
        Err(BoundedJsonError::Serialize(error)) => {
            return Err(format!("failed to encode native arguments: {error}"));
        }
    };
    let mut output = NativeBuffer::empty();
    // SAFETY: input and output pointers remain valid for the call.
    let status = unsafe { function(input.as_ptr(), input.len(), &mut output) };
    let output = OwnedNativeBuffer::new(output, free_buffer);
    if status != 0 {
        return Err(format!("native ABI call failed with status {status}"));
    }
    serde_json::from_slice::<Result<NativeValue, String>>(output.validated_bytes()?)
        .map_err(|error| format!("invalid native result payload: {error}"))?
}

#[cfg(feature = "host")]
/// Load callable bindings from a trusted in-process native plugin.
///
/// Registry and buffer metadata are bounded and validated, but this function
/// cannot prove that plugin-provided pointers are readable. Untrusted native
/// code must run behind a process boundary.
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
        let name = decode_binding_name(entry)?;
        let function = entry.func;
        let free_buffer = registry.free_buffer;
        let owner = Arc::clone(&library);
        let callable = NativeInterpreterFn::new(move |args| {
            let _owner = &owner;
            call_native(function, free_buffer, args)
        });
        bindings.push((name, callable));
    }
    Ok(bindings)
}

#[cfg(feature = "host")]
fn decode_binding_name(entry: &NativeBindingEntry) -> Result<String, String> {
    if entry.name.is_null() || entry.name_len == 0 || entry.name_len > MAX_BINDING_NAME_BYTES {
        return Err("native shim contains an invalid binding name".to_string());
    }
    // SAFETY: in-process plugins are trusted to provide readable static memory;
    // the host can validate only nullness and the bounded length first.
    let name = unsafe { std::slice::from_raw_parts(entry.name, entry.name_len) };
    std::str::from_utf8(name)
        .map_err(|error| format!("native shim binding name was not UTF-8: {error}"))
        .map(str::to_owned)
}

#[cfg(feature = "host")]
fn load_library_once(path: &std::path::Path) -> Result<Arc<libloading::Library>, String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, Weak};

    static LIBRARIES: OnceLock<Mutex<HashMap<String, Weak<libloading::Library>>>> = OnceLock::new();
    let (verified_path, key) = stage_verified_library(path)?;
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
        unsafe { libloading::Library::new(&verified_path) }
            .map_err(|error| format!("failed to load native shim library: {error}"))?,
    );
    cache.insert(key, Arc::downgrade(&library));
    Ok(library)
}

#[cfg(feature = "host")]
fn stage_verified_library(path: &std::path::Path) -> Result<(std::path::PathBuf, String), String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::sync::OnceLock;

    const MAX_LIBRARY_BYTES: u64 = 1024 * 1024 * 1024;
    static STORE: OnceLock<Result<tempfile::TempDir, String>> = OnceLock::new();

    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize native library: {error}"))?;
    let mut source = std::fs::File::open(&canonical)
        .map_err(|error| format!("failed to open native library: {error}"))?;
    let metadata = source
        .metadata()
        .map_err(|error| format!("failed to inspect native library: {error}"))?;
    if !metadata.is_file() {
        return Err("native library must be a regular file".to_string());
    }
    if metadata.len() > MAX_LIBRARY_BYTES {
        return Err(format!(
            "native library exceeds the {} byte limit",
            MAX_LIBRARY_BYTES
        ));
    }

    let store = STORE
        .get_or_init(|| {
            tempfile::Builder::new()
                .prefix("rsscript-native-abi-")
                .tempdir()
                .map_err(|error| format!("failed to create native ABI content store: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let mut staged = tempfile::NamedTempFile::new_in(store.path())
        .map_err(|error| format!("failed to stage native library: {error}"))?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| format!("failed to read native library: {error}"))?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| "native library byte count overflow".to_string())?;
        if copied > MAX_LIBRARY_BYTES {
            return Err(format!(
                "native library exceeds the {} byte limit",
                MAX_LIBRARY_BYTES
            ));
        }
        hasher.update(&buffer[..read]);
        staged
            .write_all(&buffer[..read])
            .map_err(|error| format!("failed to stage native library: {error}"))?;
    }
    staged
        .as_file()
        .sync_all()
        .map_err(|error| format!("failed to sync staged native library: {error}"))?;

    let digest = hex_digest(hasher.finalize());
    let extension = canonical
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .unwrap_or("library");
    let key = format!(
        "abi{}-{}-{}-{digest}",
        ABI_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let verified_path = store.path().join(format!("{key}.{extension}"));
    match staged.persist_noclobber(&verified_path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(format!(
                "failed to publish verified native library: {}",
                error.error
            ));
        }
    }
    Ok((verified_path, key))
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
    #[cfg(feature = "host")]
    use std::sync::Mutex;
    #[cfg(feature = "host")]
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(feature = "host")]
    static PLUGIN_MODE: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "host")]
    static PLUGIN_CALLS: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "host")]
    static PLUGIN_FREES: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "host")]
    static PLUGIN_ALLOCATIONS: Mutex<Vec<Box<[u8]>>> = Mutex::new(Vec::new());
    #[cfg(feature = "host")]
    static PLUGIN_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(feature = "host")]
    unsafe fn write_test_output(
        output: *mut NativeBuffer,
        bytes: &[u8],
        reported_len: usize,
        reported_capacity: usize,
    ) {
        let mut allocation = bytes.to_vec().into_boxed_slice();
        let ptr = allocation.as_mut_ptr();
        PLUGIN_ALLOCATIONS
            .lock()
            .expect("allocation lock")
            .push(allocation);
        // SAFETY: the test host passes a live output slot.
        unsafe {
            output.write(NativeBuffer {
                ptr,
                len: reported_len,
                capacity: reported_capacity,
            })
        };
    }

    #[cfg(feature = "host")]
    unsafe extern "C" fn state_machine_plugin(
        _: *const u8,
        _: usize,
        output: *mut NativeBuffer,
    ) -> i32 {
        PLUGIN_CALLS.fetch_add(1, Ordering::SeqCst);
        match PLUGIN_MODE.load(Ordering::SeqCst) {
            1 => {
                // SAFETY: the test host passes a live output slot.
                unsafe { write_test_output(output, b"{}", 2, 2) };
                17
            }
            2 => {
                // SAFETY: the test host passes a live output slot.
                unsafe {
                    output.write(NativeBuffer {
                        ptr: std::ptr::null_mut(),
                        len: 1,
                        capacity: 1,
                    })
                };
                0
            }
            3 => {
                // SAFETY: the test host passes a live output slot.
                unsafe { write_test_output(output, b"x", 2, 1) };
                0
            }
            4 => {
                // SAFETY: the test host passes a live output slot.
                unsafe {
                    output.write(NativeBuffer {
                        ptr: std::ptr::NonNull::<u8>::dangling().as_ptr(),
                        len: 0,
                        capacity: 0,
                    })
                };
                0
            }
            5 => {
                // The test allocator tracks the real one-byte allocation, so
                // the fake oversized capacity is never passed to Vec.
                unsafe {
                    write_test_output(output, b"x", 1, MAX_NATIVE_RESULT_BYTES.saturating_add(1))
                };
                0
            }
            6 => {
                // SAFETY: the test host passes a live output slot.
                unsafe { write_test_output(output, &[0xff], 1, 1) };
                0
            }
            7 => {
                // SAFETY: the test host passes a live output slot.
                unsafe { write_test_output(output, b"{", 1, 1) };
                0
            }
            8 => {
                // Valid JSON with the wrong result schema.
                unsafe { write_test_output(output, b"null", 4, 4) };
                0
            }
            9 => {
                let bytes =
                    serde_json::to_vec(&Err::<NativeValue, _>("plugin rejected".to_string()))
                        .expect("serialize");
                let len = bytes.len();
                // SAFETY: the test host passes a live output slot.
                unsafe { write_test_output(output, &bytes, len, len) };
                0
            }
            10 => {
                let bytes =
                    serde_json::to_vec(&Ok::<_, String>(NativeValue::Unit)).expect("serialize");
                let len = bytes.len();
                // SAFETY: the test host passes a live output slot.
                unsafe { write_test_output(output, &bytes, len, len) };
                0
            }
            _ => 99,
        }
    }

    #[cfg(feature = "host")]
    unsafe extern "C" fn state_machine_free(buffer: NativeBuffer) {
        PLUGIN_FREES.fetch_add(1, Ordering::SeqCst);
        let mut allocations = PLUGIN_ALLOCATIONS.lock().expect("allocation lock");
        if let Some(index) = allocations
            .iter()
            .position(|allocation| std::ptr::eq(allocation.as_ptr(), buffer.ptr))
        {
            allocations.swap_remove(index);
        }
    }

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

    #[test]
    fn serialized_dispatch_rejects_oversized_request_before_reading_it() {
        let mut output = NativeBuffer {
            ptr: std::ptr::NonNull::<u8>::dangling().as_ptr(),
            len: 1,
            capacity: 1,
        };
        // SAFETY: the oversized length is rejected before the dangling input is
        // read, and output points to a live slot.
        let status = unsafe {
            dispatch_serialized(
                std::ptr::NonNull::<u8>::dangling().as_ptr(),
                MAX_NATIVE_REQUEST_BYTES + 1,
                &mut output,
                |_| Ok(NativeValue::Unit),
            )
        };
        assert_eq!(status, 4);
        assert!(output.ptr.is_null());
        assert_eq!(output.len, 0);
        assert_eq!(output.capacity, 0);
    }

    #[test]
    fn serialized_dispatch_rejects_oversized_result() {
        fn oversized(_: Vec<NativeValue>) -> Result<NativeValue, String> {
            Ok(NativeValue::String("x".repeat(MAX_NATIVE_RESULT_BYTES)))
        }

        let input = b"[]";
        let mut output = NativeBuffer::empty();
        // SAFETY: pointers refer to live input/output values for the call.
        let status =
            unsafe { dispatch_serialized(input.as_ptr(), input.len(), &mut output, oversized) };
        assert_eq!(status, 4);
        assert!(output.ptr.is_null());
        assert_eq!(output.len, 0);
        assert_eq!(output.capacity, 0);
    }

    #[cfg(feature = "host")]
    #[test]
    fn host_releases_every_plugin_output_state_exactly_once() {
        let _test_lock = PLUGIN_TEST_LOCK.lock().expect("plugin test lock");
        PLUGIN_CALLS.store(0, Ordering::SeqCst);
        PLUGIN_FREES.store(0, Ordering::SeqCst);
        PLUGIN_ALLOCATIONS.lock().expect("allocation lock").clear();

        let cases = [
            (1, "status 17"),
            (2, "null buffer"),
            (3, "length exceeds"),
            (4, "zero capacity"),
            (5, "byte limit"),
            (6, "invalid native result payload"),
            (7, "invalid native result payload"),
            (8, "invalid native result payload"),
            (9, "plugin rejected"),
        ];
        for (expected_frees, (mode, expected_error)) in cases.into_iter().enumerate() {
            PLUGIN_MODE.store(mode, Ordering::SeqCst);
            let error = call_native(state_machine_plugin, state_machine_free, vec![])
                .expect_err("malicious output must fail");
            assert!(
                error.contains(expected_error),
                "unexpected mode {mode} error: {error}"
            );
            assert_eq!(PLUGIN_FREES.load(Ordering::SeqCst), expected_frees + 1);
        }

        PLUGIN_MODE.store(10, Ordering::SeqCst);
        assert_eq!(
            call_native(state_machine_plugin, state_machine_free, vec![]),
            Ok(NativeValue::Unit)
        );
        assert_eq!(PLUGIN_CALLS.load(Ordering::SeqCst), 10);
        assert_eq!(PLUGIN_FREES.load(Ordering::SeqCst), 10);
        assert!(
            PLUGIN_ALLOCATIONS
                .lock()
                .expect("allocation lock")
                .is_empty()
        );
    }

    #[cfg(feature = "host")]
    #[test]
    fn host_rejects_oversized_request_without_calling_plugin() {
        let _test_lock = PLUGIN_TEST_LOCK.lock().expect("plugin test lock");
        PLUGIN_CALLS.store(0, Ordering::SeqCst);
        let args = vec![NativeValue::String("x".repeat(MAX_NATIVE_REQUEST_BYTES))];
        let error = call_native(state_machine_plugin, state_machine_free, args)
            .expect_err("oversized request must fail");
        assert!(error.contains("request exceeds"));
        assert_eq!(PLUGIN_CALLS.load(Ordering::SeqCst), 0);
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
        let too_many_entries = NativeRegistry {
            entries: std::ptr::NonNull::<NativeBindingEntry>::dangling().as_ptr(),
            len: MAX_REGISTRY_ENTRIES + 1,
            ..valid
        };
        assert!(validate_registry(&too_many_entries).is_err());
    }

    #[cfg(feature = "host")]
    #[test]
    fn binding_name_validation_enforces_length_and_utf8_limits() {
        unsafe extern "C" fn binding(_: *const u8, _: usize, _: *mut NativeBuffer) -> i32 {
            0
        }

        let valid_name = b"module.function";
        let valid = NativeBindingEntry {
            name: valid_name.as_ptr(),
            name_len: valid_name.len(),
            func: binding,
        };
        assert_eq!(
            decode_binding_name(&valid),
            Ok("module.function".to_string())
        );

        let empty = NativeBindingEntry {
            name_len: 0,
            ..valid
        };
        assert!(decode_binding_name(&empty).is_err());

        let invalid_utf8 = [0xff];
        let invalid = NativeBindingEntry {
            name: invalid_utf8.as_ptr(),
            name_len: invalid_utf8.len(),
            ..valid
        };
        assert!(decode_binding_name(&invalid).is_err());

        let oversized_name = vec![b'x'; MAX_BINDING_NAME_BYTES + 1];
        let oversized = NativeBindingEntry {
            name: oversized_name.as_ptr(),
            name_len: oversized_name.len(),
            ..valid
        };
        assert!(decode_binding_name(&oversized).is_err());
    }
}
