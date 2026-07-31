//! eval≡lowered parity: env/random/time/db/image/cache intrinsics
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parity_environment_function_object_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    local root_value = Environment.root()
    let root = manage root_value
    if !Environment.has_parent(env: read root) {
        Log.write(message: read "root-no-parent")
    }
    if !Environment.has_function(env: read root) {
        Log.write(message: read "root-no-function")
    }

    local child_value = Environment.child(parent: read root)
    let child = manage child_value
    if Environment.has_parent(env: read child) {
        Log.write(message: read "child-parent")
    }
    if !Environment.has_function(env: read child) {
        Log.write(message: read "child-no-function")
    }

    local function_value = FunctionObject.new(closure: read child)
    let function = manage function_value
    if FunctionObject.has_closure(function: read function) {
        Log.write(message: read "function-closure")
    }
    Environment.bind_function(env: mut child, function: read function)
    if Environment.has_function(env: read child) {
        Log.write(message: read "child-function")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-environment-function.rss",
        "rsscript_parity_environment_function",
        source,
    );
}

#[test]
fn parity_args_intrinsics() {
    let source = r#"
features: native

fn main() -> Unit {
    let args = Args.all()
    Log.write(message: read String.from_int(value: Args.count()))
    Log.write(message: read List.join<String>(list: read args, separator: read "|"))
    match Args.get(index: 0) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "first-none")
        }
    }
    match Args.get(index: 2) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "third-none")
        }
    }
    match Args.get(index: 99) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "missing-none")
        }
    }
    match Args.get(index: 0 - 1) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "negative-none")
        }
    }
    Log.write(message: read Args.get_or_default(index: 1, default: read "fallback"))
    Log.write(message: read Args.get_or_default(index: 99, default: read "fallback"))
    Log.write(message: read Args.get_or_default(index: 0 - 1, default: read "negative-fallback"))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend_with_args(
        "parity-args.rss",
        "rsscript_parity_args",
        source,
        &["alpha", "beta value", "gamma"],
    );
}

#[test]
fn parity_cache_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut cache = Cache.new()
    Log.write(message: read Cache.lookup(cache: read cache, key: read "missing"))
    Cache.insert(cache: mut cache, key: read "one", value: read "first")
    Cache.insert(cache: mut cache, key: read "one", value: read "second")
    Cache.insert(cache: mut cache, key: read "two", value: read "other")
    Log.write(message: read Cache.lookup(cache: read cache, key: read "one"))
    Log.write(message: read Cache.lookup(cache: read cache, key: read "two"))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-cache.rss", "rsscript_parity_cache", source);
}

#[test]
fn parity_image_intrinsics() {
    let interpreter_root = common::unique_temp_dir("rsscript-parity-image-interpreter");
    let backend_root = common::unique_temp_dir("rsscript-parity-image-backend");
    fs::create_dir_all(&interpreter_root).expect("interpreter image dir should be created");
    fs::create_dir_all(&backend_root).expect("backend image dir should be created");

    for root in [&interpreter_root, &backend_root] {
        fs::write(root.join("input.img"), b"pixels").expect("image fixture should write");
    }

    let interpreter_input = interpreter_root.join("input.img").display().to_string();
    let interpreter_output = interpreter_root.join("output.img").display().to_string();
    let backend_input = backend_root.join("input.img").display().to_string();
    let backend_output = backend_root.join("output.img").display().to_string();
    let interpreter_args = [interpreter_input.as_str(), interpreter_output.as_str()];
    let backend_args = [backend_input.as_str(), backend_output.as_str()];

    let source = r#"
features: native, local

fn main() -> Result<Unit, ImageError> {
    let input = Path.from_string(value: read Args.get_or_default(index: 0, default: read "input.img"))
    let output = Path.from_string(value: read Args.get_or_default(index: 1, default: read "output.img"))
    local image = Image.load(path: read input)?
    Image.inspect(image: read image)
    Image.resize(image: mut image, width: 10, height: 20)
    Image.normalize(image: mut image)
    Image.sharpen(image: mut image)
    Image.inspect(image: read image)

    Image.save(image: read image, path: read output)?

    local text_cache = Cache.new()
    Cache.insert(cache: mut text_cache, key: read "image", value: read "cached-image")
    let cached = Cache.get(cache: read text_cache)
    Image.inspect(image: read cached)
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_compiled_backend_with_distinct_args(
        "parity-image.rss",
        "rsscript_parity_image",
        source,
        &interpreter_args,
        &backend_args,
    );

    let interpreter_bytes =
        fs::read(interpreter_root.join("output.img")).expect("interpreter image output exists");
    let backend_bytes =
        fs::read(backend_root.join("output.img")).expect("backend image output exists");
    assert_eq!(interpreter_bytes, backend_bytes);

    let _ = fs::remove_dir_all(&interpreter_root);
    let _ = fs::remove_dir_all(&backend_root);
}

#[test]
fn parity_clock_and_instant_intrinsics() {
    let source = r#"
features: native

fn main() -> Unit {
    let unix = Clock.system_unix_ms()
    if unix > 0 {
        Log.write(message: read "unix-positive")
    }
    let start = Clock.now()
    let elapsed = Instant.elapsed(start: read start)
    let elapsed_ms = Duration.as_ms(value: read elapsed)
    Assert.equal_int(left: elapsed_ms, right: elapsed_ms)
    Log.write(message: read "elapsed-ok")
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-clock.rss", "rsscript_parity_clock", source);
}
