//! eval≡lowered parity: data/codec/collection intrinsics
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parity_strings_and_receiver_methods() {
    let source = r#"
fn main() -> Unit {
    let greeting = String.concat(left: read "hi ", right: read "there")
    Log.write(message: read greeting)
    Log.write(message: read String.from_int(value: String.len(value: read greeting)))
    let count = greeting.len()
    Log.write(message: read String.from_int(value: count))
    let n = 255
    Log.write(message: read n.to_string())
    let blank = ""
    if blank.is_empty() {
        Log.write(message: read "blank-empty")
    }
    if String.is_empty(value: read greeting) {
        Log.write(message: read "greeting-empty")
    } else {
        Log.write(message: read "greeting-nonempty")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-strings.rss", "rsscript_parity_strings", source);
}

#[test]
fn parity_string_scalar_intrinsics() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read String.trim(value: read "  Hello World  "))
    Log.write(message: read String.to_lowercase(value: read "MiXeD"))
    Log.write(message: read String.to_uppercase(value: read "MiXeD"))
    Log.write(message: read String.replace(value: read "a-b-c", from: read "-", to: read "+"))
    Log.write(message: read String.repeat(value: read "ab", count: 3))
    if String.contains(value: read "hello", needle: read "ell") {
        Log.write(message: read "contains-yes")
    } else {
        Log.write(message: read "contains-no")
    }
    if String.starts_with(value: read "hello", prefix: read "he") {
        Log.write(message: read "starts-yes")
    } else {
        Log.write(message: read "starts-no")
    }
    if String.ends_with(value: read "hello", suffix: read "xo") {
        Log.write(message: read "ends-yes")
    } else {
        Log.write(message: read "ends-no")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-string-scalar.rss",
        "rsscript_parity_string_scalar",
        source,
    );
}

#[test]
fn parity_string_option_intrinsics() {
    let source = r#"
fn main() -> Unit {
    match String.parse_int(value: read "42") {
        Some(n) => {
            Log.write(message: read String.from_int(value: n))
        }
        None => {
            Log.write(message: read "parse-none")
        }
    }
    match String.parse_int(value: read "notnum") {
        Some(n) => {
            Log.write(message: read String.from_int(value: n))
        }
        None => {
            Log.write(message: read "parse-none")
        }
    }
    match String.index_of(value: read "hello", needle: read "l") {
        Some(i) => {
            Log.write(message: read String.from_int(value: i))
        }
        None => {
            Log.write(message: read "idx-none")
        }
    }
    match String.strip_prefix(value: read "foobar", prefix: read "foo") {
        Some(rest) => {
            Log.write(message: read rest)
        }
        None => {
            Log.write(message: read "strip-none")
        }
    }
    match String.before(value: read "a=b", delimiter: read "=") {
        Some(part) => {
            Log.write(message: read part)
        }
        None => {
            Log.write(message: read "before-none")
        }
    }
    match String.after(value: read "a=b", delimiter: read "=") {
        Some(part) => {
            Log.write(message: read part)
        }
        None => {
            Log.write(message: read "after-none")
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-string-option.rss",
        "rsscript_parity_string_option",
        source,
    );
}

#[test]
fn parity_string_collection_and_conversion_intrinsics() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read String.copy(value: read "copy-me"))
    Log.write(message: read String.from_bool(value: true))
    Log.write(message: read String.slice(value: read "aébc", start: 0, len: 3))
    Log.write(message: read String.from_int(value: Bytes.len(value: read String.to_bytes(value: read "byte"))))
    let parts = String.split(value: read "red,green,blue", delimiter: read ",")
    Log.write(message: read String.from_int(value: List.len<String>(list: read parts)))
    Log.write(message: read List.join<String>(list: read parts, separator: read "|"))
    Log.write(message: read String.join(parts: read parts, separator: read "/"))
    let lines = String.lines(value: read "one\ntwo\n")
    Log.write(message: read String.from_int(value: List.len<String>(list: read lines)))
    Log.write(message: read List.join<String>(list: read lines, separator: read "+"))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-string-collections.rss",
        "rsscript_parity_string_collections",
        source,
    );
}

#[test]
fn parity_string_view_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let text = "aébc=tail"
    let view = String.view(value: read text, start: 0, len: 6)
    Log.write(message: read StringView.to_string(value: read view))
    Log.write(message: read String.from_int(value: StringView.len(value: read view)))
    if StringView.starts_with(value: read view, prefix: read "aé") {
        Log.write(message: read "starts")
    }
    if StringView.contains(value: read view, needle: read "bc") {
        Log.write(message: read "contains")
    }
    let empty = StringView.slice(value: read view, start: 99, len: 3)
    if StringView.is_empty(value: read empty) {
        Log.write(message: read "empty")
    }
    let slice = StringView.slice(value: read view, start: 1, len: 3)
    Log.write(message: read StringView.to_string(value: read slice))
    match StringView.before(value: read view, delimiter: read "=") {
        Some(left) => {
            Log.write(message: read StringView.to_string(value: read left))
        }
        None => {
            Log.write(message: read "before-none")
        }
    }
    match StringView.after(value: read view, delimiter: read "=") {
        Some(right) => {
            Log.write(message: read StringView.to_string(value: read right))
        }
        None => {
            Log.write(message: read "after-none")
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-string-view.rss",
        "rsscript_parity_string_view",
        source,
    );
}

#[test]
fn parity_scoped_view_binding() {
    // The `view name = expr` scoped-view form (spec §20.2-1) desugars to a
    // `with`-lease whose scope ends at the enclosing block. It must run
    // identically across backends, including a view derived from another view.
    let source = r#"
fn main() -> Unit {
    let text = "hello world"
    view v = String.view(value: read text, start: 0, len: 5)
    Log.write(message: read StringView.to_string(value: read v))
    view w = StringView.slice(value: read v, start: 1, len: 3)
    Log.write(message: read String.from_int(value: StringView.len(value: read w)))
    Log.write(message: read StringView.to_string(value: read w))
    return Unit
}
"#;
    // The `with`-lease lowering binds the view as `let mut`, which rustc flags as
    // unused_mut for a read-only view (same benign warning as other `with`
    // resources); tolerate it — the values + stdout match across backends.
    common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
        "parity-scoped-view.rss",
        "rsscript_parity_scoped_view",
        source,
        &[],
        &[],
    );
}

#[test]
fn parity_char_and_string_chars_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let chars = String.chars(value: read "a1 \n")
    Log.write(message: read String.from_int(value: List.len<Char>(list: read chars)))
    let first = chars[0]
    Log.write(message: read Char.to_string(value: read first))
    Log.write(message: read String.from_int(value: Char.to_code(value: read first)))
    if Char.is_alpha(value: read first) {
        Log.write(message: read "alpha")
    }
    if Char.is_alphanumeric(value: read first) {
        Log.write(message: read "alnum")
    }
    match Char.from_code(value: 49) {
        Some(ch) => {
            if Char.is_digit(value: read ch) {
                Log.write(message: read "digit")
            }
            let original = chars[1]
            Log.write(message: read String.from_int(value: Char.compare(left: read original, right: read ch)))
        }
        None => {
            Log.write(message: read "bad-code")
        }
    }
    match Char.from_code(value: 32) {
        Some(ch) => {
            if Char.is_whitespace(value: read ch) {
                Log.write(message: read "space")
            }
        }
        None => {
            Log.write(message: read "bad-space")
        }
    }
    match Char.from_code(value: 0 - 1) {
        Some(ch) => {
            Log.write(message: read Char.to_string(value: read ch))
        }
        None => {
            Log.write(message: read "invalid")
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-char.rss", "rsscript_parity_char", source);
}

#[test]
fn parity_string_builder_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    local builder = StringBuilder.new()
    StringBuilder.push(builder: mut builder, value: read "rss")
    StringBuilder.push(builder: mut builder, value: read "cript")
    StringBuilder.push(builder: mut builder, value: read "-")
    StringBuilder.push(builder: mut builder, value: read String.from_int(value: 6))
    Log.write(message: read StringBuilder.finish(builder: take builder))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-string-builder.rss",
        "rsscript_parity_string_builder",
        source,
    );
}

#[test]
fn parity_list_literal_index_and_for_loop() {
    let source = r#"
fn main() -> Unit {
    let values: List<Int> = [1, 2, 3, 4]
    Log.write(message: read String.from_int(value: values[2]))
    Log.write(message: read String.from_int(value: List.get<Int>(list: read values, index: 1)))
    Log.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    let mut total = 0
    for value in values {
        if value == 2 {
            continue
        }
        total = total + value
    }
    Log.write(message: read String.from_int(value: total))
    match List.first<Int>(list: read values) {
        Some(first) => {
            Log.write(message: read String.from_int(value: first))
        }
        None => {
            Log.write(message: read "empty")
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-list.rss", "rsscript_parity_list", source);
}

#[test]
fn parity_map_literal_and_read_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let table: Map<String, Int> = {"a" => 1, "b" => 2}
    Log.write(message: read String.from_int(value: Map.len<String, Int>(map: read table)))
    if Map.contains_key<String, Int>(map: read table, key: read "b") {
        Log.write(message: read "has-b")
    }
    match Map.get<String, Int>(map: read table, key: read "a") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "missing")
        }
    }
    Log.write(message: read String.from_int(value: Map.get_or_default<String, Int>(map: read table, key: read "z", default: read 9)))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-map.rss", "rsscript_parity_map", source);
}

#[test]
fn parity_persistent_map_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let empty = PersistentMap<String, Int>.new()
    let one = PersistentMap.insert<String, Int>(map: read empty, key: read "one", value: read 1)
    let two = PersistentMap.insert<String, Int>(map: read one, key: read "two", value: read 2)
    let old_missing = PersistentMap.contains_key<String, Int>(map: read empty, key: read "one")
    let has_one = PersistentMap.contains_key<String, Int>(map: read one, key: read "one")
    let value = PersistentMap.get<String, Int>(map: read one, key: read "one")
    let removed = PersistentMap.remove<String, Int>(map: read two, key: read "one")
    let cleared = PersistentMap.clear<String, Int>(map: read two)

    if old_missing {
        Log.write(message: read "bad-empty")
    }
    if has_one {
        Log.write(message: read "has-one")
    }
    Log.write(message: read String.from_int(value: PersistentMap.len<String, Int>(map: read two)))
    if PersistentMap.is_empty<String, Int>(map: read cleared) {
        Log.write(message: read "cleared")
    }
    match value {
        Some(item) => {
            Log.write(message: read String.from_int(value: item))
        }
        None => {
            Log.write(message: read "missing")
        }
    }
    if PersistentMap.contains_key<String, Int>(map: read removed, key: read "one") {
        Log.write(message: read "bad-removed")
    }
    if PersistentMap.contains_key<String, Int>(map: read two, key: read "one") {
        Log.write(message: read "original-kept")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-persistent-map.rss",
        "rsscript_parity_persistent_map",
        source,
    );
}

#[test]
fn parity_string_env_intrinsics_for_missing_values() {
    let source = r#"
fn main() -> Result<Unit, FileError> {
    match Env.get(name: read "__RSSCRIPT_PARITY_ENV_MISSING__") {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "env-missing")
        }
    }
    Log.write(message: read Env.get_or_default(name: read "__RSSCRIPT_PARITY_ENV_MISSING__", default: read "env-fallback"))
    Env.set(name: read "__RSSCRIPT_PARITY_ENV_SET__", value: read "ignored")
    let current = Env.current_dir()?
    if Path.is_dir(path: read current) {
        Log.write(message: read "current-dir")
    }
    Env.set_current_dir(path: read current)?
    let root = Env.run_workspace_root()
    if Path.is_dir(path: read root) {
        Log.write(message: read "workspace-root")
    }
    match Env.home_dir() {
        Some(path) => {
            if Path.is_dir(path: read path) {
                Log.write(message: read "home-dir")
            } else {
                Log.write(message: read "home-path")
            }
        }
        None => {
            Log.write(message: read "home-none")
        }
    }
    if Path.is_dir(path: read Env.temp_dir()) {
        Log.write(message: read "temp-dir")
    }
    match String.env(value: read "__RSSCRIPT_PARITY_ENV_MISSING__") {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "string-missing")
        }
    }
    Log.write(message: read String.env_or(value: read "__RSSCRIPT_PARITY_ENV_MISSING__", default: read "fallback"))
    Log.write(message: read "__RSSCRIPT_PARITY_ENV_MISSING__".env_or("method-fallback"))
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend("parity-env.rss", "rsscript_parity_env", source);
}

#[test]
fn parity_regex_intrinsics() {
    let source = r#"
features: native, local

fn main() -> Unit {
    match Regex.compile(pattern: read "([a-z]+)-(\\d+)") {
        Ok(regex) => {
            if Regex.is_match(regex: read regex, value: read "item-42") {
                Log.write(message: read "matched")
            }
            match Regex.find(regex: read regex, value: read "pre item-42 post") {
                Some(found) => {
                    Log.write(message: read found)
                }
                None => {
                    Log.write(message: read "find-none")
                }
            }
            let captures = Regex.captures(regex: read regex, value: read "item-42")
            Log.write(message: read List.join<String>(list: read captures, separator: read "|"))
            Log.write(message: read Regex.replace_all(regex: read regex, value: read "item-42 other-7", replacement: read "x"))
            let parts = Regex.split(regex: read regex, value: read "a item-42 b other-7 c")
            Log.write(message: read List.join<String>(list: read parts, separator: read "/"))
        }
        Err(error) => {
            Log.write(message: read RegexError.message(error: read error))
        }
    }
    match Regex.compile(pattern: read "[") {
        Ok(_) => {
            Log.write(message: read "unexpected")
        }
        Err(error) => {
            Log.write(message: read RegexError.message(error: read error))
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-regex.rss", "rsscript_parity_regex", source);
}

#[test]
fn parity_math_transcendental_intrinsics() {
    // Inputs chosen so every result is an exact integer-valued float, so the
    // string form is unambiguous and identical across the interpreter and the
    // lowered backend (no reliance on float-formatting parity).
    let source = r#"
features: local

fn main() -> Unit {
    Log.write(message: read String.from_float(value: Math.sin(value: 0.0)))
    Log.write(message: read String.from_float(value: Math.cos(value: 0.0)))
    Log.write(message: read String.from_float(value: Math.exp(value: 0.0)))
    Log.write(message: read String.from_float(value: Math.exp2(value: 3.0)))
    Log.write(message: read String.from_float(value: Math.log(value: 1.0)))
    Log.write(message: read String.from_float(value: Math.log2(value: 8.0)))
    Log.write(message: read String.from_float(value: Math.tanh(value: 0.0)))
    Log.write(message: read String.from_float(value: Math.trunc_float(value: 3.75)))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-math-transcendental.rss",
        "rsscript_parity_math_transcendental",
        source,
    );
}

#[test]
fn parity_math_wrapping_and_saturating_intrinsics() {
    // The `+`/`-`/`*` operators trap on overflow (§6.8); these explicit APIs are
    // the deliberate modular/clamping escape hatches and must agree on both tiers
    // at the Int boundaries. `min` is built from `max` because the bare `i64::MIN`
    // literal would overflow the Int lexer.
    let source = r#"
features: local

fn main() -> Unit {
    let max = 9223372036854775807
    let min = 0 - max - 1
    Log.write(message: read String.from_int(value: Math.wrapping_add(left: max, right: 1)))
    Log.write(message: read String.from_int(value: Math.wrapping_sub(left: min, right: 1)))
    Log.write(message: read String.from_int(value: Math.wrapping_mul(left: max, right: 2)))
    Log.write(message: read String.from_int(value: Math.saturating_add(left: max, right: 1)))
    Log.write(message: read String.from_int(value: Math.saturating_sub(left: min, right: 1)))
    Log.write(message: read String.from_int(value: Math.saturating_mul(left: max, right: 2)))
    Log.write(message: read String.from_int(value: Math.wrapping_add(left: 2, right: 3)))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-math-wrapping-saturating.rss",
        "rsscript_parity_math_wrapping_saturating",
        source,
    );
}

#[test]
fn parity_bytes_hash_and_gzip_intrinsics() {
    // Hashes/decompression use the same Rust crates on both backends, so the byte
    // outputs are identical; the gzip blob is `gzip("rsscript")` (8 bytes).
    let source = r#"
features: native, local

fn main() -> Unit {
    let bytes = Bytes.from_uints(values: read [104, 105])
    Log.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    Log.write(message: read Bytes.to_string(value: read bytes))
    let uints = Bytes.to_uints(value: read bytes)
    Log.write(message: read String.from_int(value: List.get<Int>(list: read uints, index: 0)))
    Log.write(message: read String.from_int(value: List.get<Int>(list: read uints, index: 1)))

    let sha224 = Hash.sha3_224_bytes(value: read bytes)
    Log.write(message: read String.from_int(value: Bytes.len(value: read sha224)))
    let sha256 = Hash.sha3_256_bytes(value: read bytes)
    Log.write(message: read String.from_int(value: Bytes.len(value: read sha256)))
    let shake = Hash.shake128_bytes(value: read bytes, out_len: 16)
    Log.write(message: read String.from_int(value: Bytes.len(value: read shake)))
    let sha256_uints = Bytes.to_uints(value: read sha256)
    Log.write(message: read String.from_int(value: List.get<Int>(list: read sha256_uints, index: 0)))

    let gz = Bytes.from_uints(values: read [31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 43, 42, 46, 78, 46, 202, 44, 40, 1, 0, 171, 165, 148, 251, 8, 0, 0, 0])
    match Gzip.decompress_bytes(value: read gz) {
        Ok(plain) => {
            Log.write(message: read String.from_int(value: Bytes.len(value: read plain)))
        }
        Err(error) => {
            Log.write(message: read DecodeError.message(error: read error))
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-bytes-hash-gzip.rss",
        "rsscript_parity_bytes_hash_gzip",
        source,
    );
}

#[test]
fn parity_bytes_and_json_literals() {
    let source = r#"
fn main() -> Unit {
    let data = Bytes.from_string(value: read "abcdef")
    let part = Bytes.slice(value: read data, start: 2, len: 3)
    Log.write(message: read String.from_int(value: Bytes.len(value: read part)))
    if BytesView.starts_with(value: read Bytes.view(value: read data, start: 1, len: 3), prefix: read Bytes.view(value: read data, start: 1, len: 2)) {
        Log.write(message: read "bytes-prefix")
    }
    let doc: JsonValue = {"ok": true, "name": "rss"}
    Log.write(message: read Json.kind(value: read doc))
    if Json.is_object(value: read doc) {
        Log.write(message: read "json-object")
    }
    Log.write(message: read Json.to_string(value: read Json.value(value: read {"answer": 42})))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-bytes-json.rss",
        "rsscript_parity_bytes_json",
        source,
    );
}

#[test]
fn parity_assert_hash_and_bytes_consume_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    let digest = Hash.sha256_string(value: read "abc")
    Assert.equal(left: read digest, right: read "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    let bytes = Bytes.from_string(value: read "abc")
    Assert.equal(left: read Hash.sha256_bytes(value: read bytes), right: read digest)
    Assert.equal_int(left: Bytes.len(value: read bytes), right: 3)
    Assert.equal_bool(left: Bytes.is_empty(value: read bytes), right: false)
    local disposable = Bytes.concat(left: read bytes, right: read Bytes.from_string(value: read "!"))
    Bytes.consume(bytes: take disposable)
    Log.write(message: read "assert-hash-ok")
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-assert-hash-bytes.rss",
        "rsscript_parity_assert_hash_bytes",
        source,
    );
}

#[test]
fn parity_buffer_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    local buffer = Buffer.new(size: 16)
    if Buffer.is_empty(buffer: read buffer) {
        Log.write(message: read "buffer-empty")
    }
    Log.write(message: read String.from_int(value: Buffer.len(buffer: read buffer)))
    let view = Buffer.view(buffer: read buffer, start: 0, len: 10)
    if BufferView.is_empty(value: read view) {
        Log.write(message: read "view-empty")
    }
    Log.write(message: read String.from_int(value: BufferView.len(value: read view)))
    let slice = BufferView.slice(value: read view, start: 1, len: 2)
    Log.write(message: read String.from_int(value: Bytes.len(value: read BufferView.to_bytes(value: read slice))))
    Log.write(message: read String.from_int(value: Bytes.len(value: read Bytes.from_buffer(buffer: read buffer))))
    Buffer.clear(buffer: mut buffer)
    Buffer.consume(buffer: take buffer)
    Log.write(message: read "consumed")
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-buffer.rss", "rsscript_parity_buffer", source);
}

#[test]
fn parity_list_mutating_intrinsics_and_index_assignment() {
    let source = r#"
fn main() -> Unit {
    let mut values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.push<Int>(list: mut values, value: read 3)
    List.set<Int>(list: mut values, index: 1, value: read 20)
    values[2] = 30
    let suffix: List<Int> = [40, 50]
    List.append<Int>(list: mut values, values: read suffix)
    match List.pop<Int>(list: mut values) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "pop-none")
        }
    }
    match List.remove_at<Int>(list: mut values, index: 1) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "remove-none")
        }
    }
    Log.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    Log.write(message: read String.from_int(value: List.get<Int>(list: read values, index: 1)))
    List.clear<Int>(list: mut values)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-list-mutating.rss",
        "rsscript_parity_list_mutating",
        source,
    );
}

#[test]
fn parity_list_non_closure_intrinsics() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut numbers: List<Int> = [3, 1, 2, 5, 4]
    List.sort<Int>(list: mut numbers)
    Log.write(message: read String.from_int(value: numbers[0]))
    Log.write(message: read String.from_int(value: numbers[4]))

    let reversed = List.reverse<Int>(list: read numbers)
    Log.write(message: read String.from_int(value: reversed[0]))
    let skipped = List.skip<Int>(list: read numbers, count: 2)
    Log.write(message: read String.from_int(value: skipped[0]))
    local taken = List.take<Int>(list: read numbers, count: 3)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read taken)))
    let sliced = List.slice<Int>(list: read numbers, start: 1, len: 3)
    Log.write(message: read String.from_int(value: sliced[0]))
    Log.write(message: read String.from_int(value: sliced[2]))

    let words: List<String> = ["a", "b"]
    let json_strings = List.to_json_strings(list: read words)
    Log.write(message: read Json.to_string(value: read json_strings))
    let json_values: List<JsonValue> = [Json.value(value: read {"n": 1}), Json.value(value: read {"n": 2})]
    let json_array = List.to_json_values(list: read json_values)
    Log.write(message: read Json.to_string(value: read json_array))

    List.consume<Int>(list: take taken)
    Log.write(message: read "consumed")
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-list-non-closure.rss",
        "rsscript_parity_list_non_closure",
        source,
    );
}

#[test]
fn parity_list_closure_intrinsics() {
    let source = r#"
features: local

struct Acc {
    total: Int
}

fn is_even(value: Int) -> Bool {
    let half = value / 2
    return half * 2 == value
}

fn main() -> Unit {
    let numbers: List<Int> = [1, 2, 3, 4, 5]
    let threshold = 3

    Log.write(message: read String.from_int(value: List.count_where<Int>(list: read numbers, predicate: |item| {
        return item > threshold
    })))
    Log.write(message: read String.from_bool(value: List.any<Int>(list: read numbers, predicate: |item| {
        return item == 5
    })))
    Log.write(message: read String.from_bool(value: List.all<Int>(list: read numbers, predicate: |item| {
        return item > 0
    })))
    Log.write(message: read String.from_bool(value: List.contains<Int>(list: read numbers, predicate: |item| {
        return item == 3
    })))

    match List.find<Int>(list: read numbers, predicate: |item| {
        return item > threshold
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "find-none")
        }
    }

    let filtered = List.filter<Int>(list: read numbers, predicate: |item| {
        return is_even(value: item)
    })
    Log.write(message: read String.from_int(value: List.len<Int>(list: read filtered)))
    Log.write(message: read String.from_int(value: filtered[0]))
    Log.write(message: read String.from_int(value: filtered[1]))

    let mapped = List.map<Int, Int>(list: read numbers, mapper: |item| {
        return item + threshold
    })
    Log.write(message: read String.from_int(value: mapped[0]))
    Log.write(message: read String.from_int(value: mapped[4]))

    let mut sorted = [3, 1, 2]
    List.sort_with<Int>(list: mut sorted, compare: |left, right| {
        return right - left
    })
    Log.write(message: read String.from_int(value: sorted[0]))
    Log.write(message: read String.from_int(value: sorted[2]))

    let sorted_words = List.sort_by<String, Int>(list: read ["bbb", "a", "cc"], key: |word| {
        return String.len(value: read word)
    }, compare: |left, right| {
        return left - right
    })
    Log.write(message: read sorted_words[0])
    Log.write(message: read sorted_words[2])

    let grouped = List.group_by<Int, String>(list: read numbers, key: |item| {
        if is_even(value: item) {
            return String.copy(value: read "even")
        }
        return String.copy(value: read "odd")
    })
    match Map.get(map: read grouped, key: read "even") {
        Some(items) => {
            Log.write(message: read String.from_int(value: List.len(list: read items)))
            Log.write(message: read String.from_int(value: items[0]))
        }
        None => {
            Log.write(message: read "even-missing")
        }
    }
    match Map.get(map: read grouped, key: read "odd") {
        Some(items) => {
            Log.write(message: read String.from_int(value: List.len(list: read items)))
            Log.write(message: read String.from_int(value: items[2]))
        }
        None => {
            Log.write(message: read "odd-missing")
        }
    }

    let folded = List.fold<Int, Acc>(list: read numbers, initial: read Acc(total: 0), folder: |state, item| {
        return Acc(total: state.total + item)
    })
    Log.write(message: read String.from_int(value: folded.total))

    match List.try_fold<Int, Acc, String>(list: read numbers, initial: read Acc(total: 0), folder: |state, item| {
        if item > 3 {
            return Err(String.copy(value: read "too-large"))
        }
        return Ok(Acc(total: state.total + item))
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value.total))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    match List.try_fold<Int, Acc, String>(list: read filtered, initial: read Acc(total: 0), folder: |state, item| {
        if item < 0 {
            return Err(String.copy(value: read "negative"))
        }
        return Ok(Acc(total: state.total + item))
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value.total))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    let flattened = List.flat_map<Int, Int>(list: read filtered, mapper: |item| {
        let values: List<Int> = [item, item + 10]
        return values
    })
    Log.write(message: read String.from_int(value: List.len<Int>(list: read flattened)))
    Log.write(message: read String.from_int(value: flattened[1]))
    Log.write(message: read String.from_int(value: flattened[3]))

    let parts = List.partition<Int>(list: read numbers, predicate: |item| {
        return item > threshold
    })
    Log.write(message: read String.from_int(value: List.len<Int>(list: read parts[0])))
    Log.write(message: read String.from_int(value: parts[0][0]))
    Log.write(message: read String.from_int(value: List.len<Int>(list: read parts[1])))
    Log.write(message: read String.from_int(value: parts[1][2]))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-list-closure.rss",
        "rsscript_parity_list_closure",
        source,
    );
}

#[test]
fn parity_map_mutating_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut table = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut table, key: read "one", value: read 1)
    Map.insert<String, Int>(map: mut table, key: read "two", value: read 2)
    match Map.insert_old<String, Int>(map: mut table, key: read "one", value: read 10) {
        Some(old) => {
            Log.write(message: read String.from_int(value: old))
        }
        None => {
            Log.write(message: read "insert-none")
        }
    }
    match Map.get<String, Int>(map: read table, key: read "one") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "missing")
        }
    }
    match Map.remove<String, Int>(map: mut table, key: read "two") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "remove-none")
        }
    }
    Log.write(message: read String.from_int(value: Map.len<String, Int>(map: read table)))
    Map.clear<String, Int>(map: mut table)
    Log.write(message: read String.from_int(value: Map.len<String, Int>(map: read table)))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-map-mutating.rss",
        "rsscript_parity_map_mutating",
        source,
    );
}

#[test]
fn parity_map_closure_intrinsics() {
    let source = r#"
struct Acc {
    total: Int
}

fn main() -> Unit {
    let mut left = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut left, key: read "a", value: read 1)
    Map.insert<String, Int>(map: mut left, key: read "b", value: read 2)

    let mapped = Map.map_values<String, Int, Int>(map: read left, mapper: |value| {
        return value + 10
    })
    match Map.get<String, Int>(map: read mapped, key: read "a") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "mapped-missing")
        }
    }

    let filtered = Map.filter<String, Int>(map: read mapped, predicate: |key, value| {
        return key == "b" && value > 10
    })
    Log.write(message: read String.from_int(value: Map.len<String, Int>(map: read filtered)))
    match Map.get<String, Int>(map: read filtered, key: read "b") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "filtered-missing")
        }
    }

    let mut single = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut single, key: read "only", value: read 8)
    Map.for_each<String, Int>(map: read single, callback: |key, value| {
        Log.write(message: read key)
        Log.write(message: read String.from_int(value: value))
        return Unit
    })

    let folded = Map.fold<String, Int, Acc>(map: read left, initial: read Acc(total: 0), folder: |state, key, value| {
        if key == "a" {
            return Acc(total: state.total + value)
        }
        return Acc(total: state.total + value + 10)
    })
    Log.write(message: read String.from_int(value: folded.total))

    match Map.try_fold<String, Int, Acc, String>(map: read left, initial: read Acc(total: 0), folder: |state, key, value| {
        if key == "b" {
            return Err(String.copy(value: read "stop-b"))
        }
        return Ok(Acc(total: state.total + value))
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value.total))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    let mut right = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut right, key: read "b", value: read 20)
    Map.insert<String, Int>(map: mut right, key: read "c", value: read 30)
    let merged = Map.merge<String, Int>(left: read left, right: read right, resolver: |left_value, right_value| {
        return left_value + right_value
    })
    match Map.get<String, Int>(map: read merged, key: read "b") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "merge-b-missing")
        }
    }
    match Map.get<String, Int>(map: read merged, key: read "c") {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "merge-c-missing")
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-map-closure.rss",
        "rsscript_parity_map_closure",
        source,
    );
}

#[test]
fn parity_set_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut set = Set<String>.new()
    if Set.is_empty<String>(set: read set) {
        Log.write(message: read "empty")
    }
    if Set.insert<String>(set: mut set, value: read "a") {
        Log.write(message: read "insert-a")
    }
    if Set.insert<String>(set: mut set, value: read "b") {
        Log.write(message: read "insert-b")
    }
    if Set.insert<String>(set: mut set, value: read "a") {
        Log.write(message: read "duplicate")
    } else {
        Log.write(message: read "duplicate-no")
    }
    if Set.contains<String>(set: read set, value: read "b") {
        Log.write(message: read "has-b")
    }
    Log.write(message: read String.from_int(value: Set.len<String>(set: read set)))
    if Set.remove<String>(set: mut set, value: read "a") {
        Log.write(message: read "removed-a")
    }
    if Set.remove<String>(set: mut set, value: read "z") {
        Log.write(message: read "removed-z")
    } else {
        Log.write(message: read "removed-z-no")
    }
    Set.for_each<String>(set: read set, callback: |value| {
        Log.write(message: read value)
        return Unit
    })

    let mut right = Set<String>.new()
    Set.insert<String>(set: mut right, value: read "b")
    Set.insert<String>(set: mut right, value: read "c")
    let union = Set.union<String>(left: read set, right: read right)
    let intersection = Set.intersection<String>(left: read set, right: read right)
    let difference = Set.difference<String>(left: read right, right: read set)
    if Set.contains<String>(set: read union, value: read "c") {
        Log.write(message: read "union-c")
    }
    Log.write(message: read String.from_int(value: Set.len<String>(set: read intersection)))
    if Set.contains<String>(set: read difference, value: read "c") {
        Log.write(message: read "diff-c")
    }
    if Set.is_subset<String>(left: read intersection, right: read union) {
        Log.write(message: read "subset")
    }
    Log.write(message: read String.from_int(value: List.len<String>(list: read Set.to_list<String>(set: read union))))
    Set.clear<String>(set: mut set)
    Log.write(message: read String.from_int(value: Set.len<String>(set: read set)))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-set.rss", "rsscript_parity_set", source);
}

#[test]
fn parity_sorted_set_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut set = SortedSet<Int>.new()
    if SortedSet.is_empty<Int>(set: read set) {
        Log.write(message: read "empty")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 3) {
        Log.write(message: read "insert-3")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 1) {
        Log.write(message: read "insert-1")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 2) {
        Log.write(message: read "insert-2")
    }
    if SortedSet.insert<Int>(set: mut set, value: read 2) {
        Log.write(message: read "duplicate")
    } else {
        Log.write(message: read "duplicate-no")
    }
    if SortedSet.contains<Int>(set: read set, value: read 1) {
        Log.write(message: read "has-1")
    }
    Log.write(message: read String.from_int(value: SortedSet.len<Int>(set: read set)))
    let values = SortedSet.to_list<Int>(set: read set)
    Log.write(message: read String.from_int(value: values[0]))
    Log.write(message: read String.from_int(value: values[1]))
    Log.write(message: read String.from_int(value: values[2]))
    if SortedSet.remove<Int>(set: mut set, value: read 2) {
        Log.write(message: read "removed-2")
    }
    if SortedSet.remove<Int>(set: mut set, value: read 9) {
        Log.write(message: read "removed-9")
    } else {
        Log.write(message: read "removed-9-no")
    }
    SortedSet.clear<Int>(set: mut set)
    Log.write(message: read String.from_int(value: SortedSet.len<Int>(set: read set)))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-sorted-set.rss",
        "rsscript_parity_sorted_set",
        source,
    );
}

#[test]
fn parity_sorted_map_intrinsics() {
    let source = r#"
fn main() -> Unit {
    let mut map = SortedMap<Int, String>.new()
    if SortedMap.is_empty<Int, String>(map: read map) {
        Log.write(message: read "empty")
    }
    SortedMap.insert<Int, String>(map: mut map, key: read 2, value: read "two")
    SortedMap.insert<Int, String>(map: mut map, key: read 1, value: read "one")
    SortedMap.insert<Int, String>(map: mut map, key: read 3, value: read "three")
    SortedMap.insert<Int, String>(map: mut map, key: read 2, value: read "TWO")
    Log.write(message: read String.from_int(value: SortedMap.len<Int, String>(map: read map)))
    if SortedMap.contains_key<Int, String>(map: read map, key: read 2) {
        Log.write(message: read "has-2")
    }
    match SortedMap.get<Int, String>(map: read map, key: read 2) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "missing")
        }
    }
    let keys = SortedMap.keys<Int, String>(map: read map)
    Log.write(message: read String.from_int(value: keys[0]))
    Log.write(message: read String.from_int(value: keys[1]))
    Log.write(message: read String.from_int(value: keys[2]))
    let values = SortedMap.values<Int, String>(map: read map)
    Log.write(message: read values[0])
    Log.write(message: read values[1])
    Log.write(message: read values[2])
    match SortedMap.remove<Int, String>(map: mut map, key: read 2) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "remove-none")
        }
    }
    match SortedMap.remove<Int, String>(map: mut map, key: read 9) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "remove-none")
        }
    }
    SortedMap.clear<Int, String>(map: mut map)
    Log.write(message: read String.from_int(value: SortedMap.len<Int, String>(map: read map)))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-sorted-map.rss",
        "rsscript_parity_sorted_map",
        source,
    );
}

#[test]
fn parity_json_builder_and_array_intrinsics() {
    common::run_with_large_stack(|| {
        let source = r#"
struct JsonAcc {
    total: Int
}

fn main() -> Result<Unit, JsonError> {
    let mut fields = List<String>.new()
    List.push<String>(list: mut fields, value: read Json.string_field(name: read "name", value: read "rss"))
    List.push<String>(list: mut fields, value: read Json.int_field(name: read "count", value: 2))
    List.push<String>(list: mut fields, value: read Json.bool_field(name: read "ok", value: true))
    List.push<String>(list: mut fields, value: read Json.raw_field(name: read "items", value: read Json.array(items: read ["1", "2"])))
    let object_text = Json.object(fields: read fields)
    Log.write(message: read object_text)
    let string_array_text = Json.string_array(items: read ["rss", "script"])
    Log.write(message: read string_array_text)

    let strings_json = Json.strings(items: read ["profile", "project", "other"])
    if Json.array_contains_string(value: read strings_json, item: read "profile")? {
        Log.write(message: read "has-profile")
    }
    if Json.array_contains_substring(value: read strings_json, text: read "roj")? {
        Log.write(message: read "has-substring")
    }
    if Json.array_contains_prefix(value: read strings_json, prefix: read "pro")? {
        Log.write(message: read "has-prefix")
    }
    let strings = Json.array_strings(value: read strings_json)?
    Log.write(message: read List.join<String>(list: read strings, separator: read "|"))

    let ints_json = Json.parse(text: read "[1,2,3]")?
    let ints = Json.array_ints(value: read ints_json)?
    Log.write(message: read String.from_int(value: ints[2]))
    let count = Json.array_count_where(value: read ints_json, predicate: |item| {
        let parsed = Json.as_int(value: read item)?
        return Ok(parsed > 1)
    })?
    Log.write(message: read String.from_int(value: count))
    let folded = Json.array_fold<JsonAcc>(value: read ints_json, initial: read JsonAcc(total: 0), folder: |state, item| {
        let parsed = Json.as_int(value: read item)?
        return Ok(JsonAcc(total: state.total + parsed))
    })?
    Log.write(message: read String.from_int(value: folded.total))
    let bools_json = Json.parse(text: read "[true,false]")?
    let bools = Json.array_bools(value: read bools_json)?
    if bools[0] {
        Log.write(message: read "bool-true")
    }

    let values = Json.values(items: read [Json.value(value: read {"n": 1}), Json.clone(value: read Json.value(value: read {"n": 2}))])
    Log.write(message: read Json.to_string(value: read values))
    let bad_strings_json = Json.parse(text: read "[1]")?
    match Json.array_strings(value: read bad_strings_json) {
        Ok(items) => {
            Log.write(message: read List.join<String>(list: read items, separator: read ","))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
        common::assert_vm_eval_matches_backend(
            "parity-json-builder-array.rss",
            "rsscript_parity_json_builder_array",
            source,
        );
    });
}

#[test]
fn parity_json_result_intrinsics() {
    let source = r#"
fn main() -> Unit {
    match Json.parse(text: read "{\"count\":3,\"ok\":true,\"items\":[1,2,3]}") {
        Ok(doc) => {
            match Json.field_int(value: read doc, name: read "count") {
                Ok(count) => {
                    Log.write(message: read String.from_int(value: count))
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.field_optional_bool(value: read doc, name: read "ok") {
                Ok(Some(flag)) => {
                    if flag {
                        Log.write(message: read "flag-true")
                    } else {
                        Log.write(message: read "flag-false")
                    }
                }
                Ok(None) => {
                    Log.write(message: read "flag-none")
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.field_optional_string(value: read doc, name: read "missing") {
                Ok(Some(text)) => {
                    Log.write(message: read text)
                }
                Ok(None) => {
                    Log.write(message: read "missing-none")
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.field(value: read doc, name: read "items") {
                Ok(items) => {
                    match Json.array_len(value: read items) {
                        Ok(len) => {
                            Log.write(message: read String.from_int(value: len))
                        }
                        Err(error) => {
                            Log.write(message: read JsonError.message(error: read error))
                        }
                    }
                    match Json.array_get(value: read items, index: 1) {
                        Ok(item) => {
                            match Json.as_int(value: read item) {
                                Ok(n) => {
                                    Log.write(message: read String.from_int(value: n))
                                }
                                Err(error) => {
                                    Log.write(message: read JsonError.message(error: read error))
                                }
                            }
                        }
                        Err(error) => {
                            Log.write(message: read JsonError.message(error: read error))
                        }
                    }
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.as_int(value: read Json.value(value: read {"text": "nope"})) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-json-result.rss",
        "rsscript_parity_json_result",
        source,
    );
}

#[test]
fn parity_char_literals_and_escapes() {
    // SH-016: `'x'` is a real `Char` value. Exercise bind, `==`, `match` on char
    // literals, and every escape (`\n \r \t \\ \' \0` and `"`) so the interpreter
    // and the AOT Rust lowering (which emits `format!("{:?}", char)`) agree on the
    // escaping — the #1 parity risk for this feature.
    let source = r#"
fn describe(c: read Char) -> String {
    match c {
        'a' => { return "vowel" }
        '\n' => { return "newline" }
        '\r' => { return "cr" }
        '\t' => { return "tab" }
        '\\' => { return "backslash" }
        '\'' => { return "quote" }
        '"' => { return "dquote" }
        '\0' => { return "nul" }
        _ => { return "other" }
    }
}

fn main() -> Unit {
    let c = 'a'
    Log.write(message: read Char.to_string(value: read c))
    Log.write(message: read String.from_int(value: Char.to_code(value: read c)))
    Log.write(message: read describe(c: read c))
    Log.write(message: read describe(c: read '\n'))
    Log.write(message: read describe(c: read '\r'))
    Log.write(message: read describe(c: read '\t'))
    Log.write(message: read describe(c: read '\\'))
    Log.write(message: read describe(c: read '\''))
    Log.write(message: read describe(c: read '"'))
    Log.write(message: read describe(c: read '\0'))
    Log.write(message: read describe(c: read 'z'))
    if 'x' == 'x' {
        Log.write(message: read "eq")
    }
    if 'x' == 'y' {
        Log.write(message: read "bad")
    } else {
        Log.write(message: read "neq")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-char.rss", "rsscript_parity_char", source);
}
