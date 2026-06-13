//! register-VM execution: strings, bytes, and encodings
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn reg_vm_runs_interpolated_strings_like_backend() {
    let source = r#"
features: native

fn greeting(name: read String) -> fresh String {
    return $"hello {name}"
}

fn main() -> Unit {
    let name = "rss"
    Log.write(message: read $"hello {name}")
    Log.write(message: read $"literal {{}} and {greeting(name: read "vm")}")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-interpolated-strings.rss", source, []);
}

#[test]
fn reg_vm_runs_env_string_intrinsics_like_interpreter() {
    let source = r#"
features: native

fn main() -> Unit {
    match Env.get(name: read "RSSCRIPT_VM_PARITY_ENV_SHOULD_NOT_EXIST") {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "env-none")
        }
    }
    Log.write(message: read Env.get_or_default(name: read "RSSCRIPT_VM_PARITY_ENV_SHOULD_NOT_EXIST", default: read "env-default"))

    match String.env(value: read "RSSCRIPT_VM_PARITY_STRING_ENV_SHOULD_NOT_EXIST") {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "string-env-none")
        }
    }
    Log.write(message: read String.env_or(value: read "RSSCRIPT_VM_PARITY_STRING_ENV_SHOULD_NOT_EXIST", default: read "string-env-default"))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-env-string-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_char_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    match Char.from_code(value: 97) {
        Some(first) => {
            Log.write(message: read Char.to_string(value: read first))
            Log.write(message: read String.from_int(value: Char.to_code(value: read first)))
            if Char.is_alpha(value: read first) {
                Log.write(message: read "alpha")
            }
            if Char.is_alphanumeric(value: read first) {
                Log.write(message: read "alnum")
            }
            match Char.from_code(value: 98) {
                Some(second) => {
                    Log.write(message: read String.from_int(value: Char.compare(left: read first, right: read second)))
                }
                None => {
                    Log.write(message: read "bad-second")
                }
            }
        }
        None => {
            Log.write(message: read "bad-first")
        }
    }

    match Char.from_code(value: 49) {
        Some(ch) => {
            if Char.is_digit(value: read ch) {
                Log.write(message: read "digit")
            }
        }
        None => {
            Log.write(message: read "bad-digit")
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

    assert_reg_vm_matches_compiled_backend("reg-vm-char-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_bytes_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let data = Bytes.from_string(value: read "abcdef")
    let from_string = String.to_bytes(value: read "gh")
    let joined = Bytes.concat(left: read data, right: read from_string)

    Log.write(message: read String.from_int(value: Bytes.len(value: read joined)))
    if !Bytes.is_empty(value: read joined) {
        Log.write(message: read "not-empty")
    }

    let part = Bytes.slice(value: read joined, start: 2, len: 3)
    Log.write(message: read String.from_int(value: Bytes.len(value: read part)))
    Log.write(message: read Bytes.to_string(value: read part))

    let view = Bytes.view(value: read joined, start: 1, len: 4)
    Log.write(message: read String.from_int(value: BytesView.len(value: read view)))

    let sub = BytesView.slice(value: read view, start: 1, len: 2)
    Log.write(message: read String.from_int(value: Bytes.len(value: read BytesView.to_bytes(value: read sub))))

    if BytesView.starts_with(
        value: read view,
        prefix: read Bytes.view(value: read joined, start: 1, len: 2)
    ) {
        Log.write(message: read "bytes-prefix")
    }

    if BytesView.is_empty(value: read Bytes.view(value: read joined, start: 99, len: 10)) {
        Log.write(message: read "empty-view")
    }

    local disposable = Bytes.concat(left: read data, right: read Bytes.from_string(value: read "!"))
    Bytes.consume(bytes: take disposable)
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-bytes-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_regex_intrinsics_like_interpreter() {
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

    assert_reg_vm_matches_compiled_backend("reg-vm-regex-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_regex_capture_edges_like_interpreter() {
    let source = r#"
features: native, local

fn main() -> Unit {
    match Regex.compile(pattern: read "(a)?b") {
        Ok(regex) => {
            let captures = Regex.captures(regex: read regex, value: read "b")
            Log.write(message: read List.join<String>(list: read captures, separator: read "|"))
        }
        Err(error) => {
            Log.write(message: read RegexError.message(error: read error))
        }
    }
    match Regex.compile(pattern: read "([a-z]+)-(\\d+)") {
        Ok(regex) => {
            Log.write(message: read Regex.replace_all(regex: read regex, value: read "item-42 other-7", replacement: read "$1"))
        }
        Err(error) => {
            Log.write(message: read RegexError.message(error: read error))
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-regex-capture-edges.rss", source, []);
}

#[test]
fn reg_vm_runs_yaml_parse_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    match Yaml.parse(text: read "name: rss\ncount: 10\nitems:\n  - one\n  - two\n") {
        Ok(doc) => {
            match Json.field_string(value: read doc, name: read "name") {
                Ok(name) => {
                    Log.write(message: read name)
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.field_int(value: read doc, name: read "count") {
                Ok(count) => {
                    Log.write(message: read String.from_int(value: count))
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.field(value: read doc, name: read "items") {
                Ok(items) => {
                    match Json.array_strings(value: read items) {
                        Ok(values) => {
                            Log.write(message: read List.join<String>(list: read values, separator: read "|"))
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
    match Yaml.parse(text: read "name: [") {
        Ok(_) => {
            Log.write(message: read "unexpected")
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-yaml-parse.rss", source, []);
}

#[test]
fn reg_vm_runs_string_view_builder_intrinsics_like_interpreter() {
    let source = r#"
features: local

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

    let chars = String.chars(value: read "a1 \n")
    Log.write(message: read String.from_int(value: List.len<Char>(list: read chars)))
    Log.write(message: read Char.to_string(value: read chars[0]))

    local builder = StringBuilder.new()
    StringBuilder.push(builder: mut builder, value: read "rss")
    StringBuilder.push(builder: mut builder, value: read "cript")
    StringBuilder.push(builder: mut builder, value: read "-")
    StringBuilder.push(builder: mut builder, value: read String.from_int(value: 6))
    Log.write(message: read StringBuilder.finish(builder: take builder))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-string-view-builder-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_read_only_collection_and_string_intrinsics_like_interpreter() {
    let source = r#"
features: native

fn main() -> Unit {
    let words: List<String> = ["red", "green", "blue"]
    Assert.equal_bool(left: List.is_empty<String>(list: read words), right: false)
    Assert.equal(left: read List.join<String>(list: read words, separator: read "|"), right: read "red|green|blue")
    match List.first<String>(list: read words) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "empty")
        }
    }

    let table: Map<String, Int> = {"a" => 1, "b" => 2}
    Assert.equal_int(left: Map.len<String, Int>(map: read table), right: 2)
    Assert.equal_bool(left: Map.contains_key<String, Int>(map: read table, key: read "b"), right: true)
    Assert.equal_int(left: Map.get_or_default<String, Int>(map: read table, key: read "z", default: read 9), right: 9)

    Assert.equal_int(left: String.len(value: read "é"), right: 2)
    Assert.equal_bool(left: String.is_empty(value: read ""), right: true)
    Assert.equal_bool(left: String.contains(value: read "hello", needle: read "ell"), right: true)
    Assert.equal_bool(left: String.starts_with(value: read "hello", prefix: read "he"), right: true)
    Log.write(message: read "ok")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-read-only-runtime.rss", source, []);
}

#[test]
fn reg_vm_runs_string_scalar_option_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read String.trim(value: read "  Hello World  "))
    Log.write(message: read String.to_lowercase(value: read "MiXeD"))
    Log.write(message: read String.to_uppercase(value: read "MiXeD"))
    Log.write(message: read String.replace(value: read "a-b-c", from: read "-", to: read "+"))
    Log.write(message: read String.repeat(value: read "ab", count: 3))
    Log.write(message: read String.repeat(value: read "ab", count: 0 - 2))
    Log.write(message: read String.slice(value: read "aébc", start: 0, len: 3))
    Log.write(message: read String.slice(value: read "aébc", start: 2, len: 2))
    Log.write(message: read String.slice(value: read "aébc", start: 100, len: 5))
    if String.ends_with(value: read "hello", suffix: read "lo") {
        Log.write(message: read "ends-yes")
    }
    match String.index_of(value: read "hello", needle: read "l") {
        Some(index) => {
            Log.write(message: read String.from_int(value: index))
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
    match String.strip_prefix(value: read "foobar", prefix: read "bar") {
        Some(rest) => {
            Log.write(message: read rest)
        }
        None => {
            Log.write(message: read "strip-none")
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-string-scalar-option.rss", source, []);
}

#[test]
fn reg_vm_runs_string_collection_and_more_option_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read String.copy(value: read "copy-me"))
    Log.write(message: read String.from_bool(value: true))
    let parts = String.split(value: read "red,green,blue", delimiter: read ",")
    Log.write(message: read String.from_int(value: List.len<String>(list: read parts)))
    Log.write(message: read List.join<String>(list: read parts, separator: read "|"))
    Log.write(message: read String.join(parts: read parts, separator: read "/"))
    let lines = String.lines(value: read "one\ntwo\n")
    Log.write(message: read String.from_int(value: List.len<String>(list: read lines)))
    Log.write(message: read String.join(parts: read lines, separator: read "+"))

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
    match String.before(value: read "abc", delimiter: read "=") {
        Some(part) => {
            Log.write(message: read part)
        }
        None => {
            Log.write(message: read "before-none")
        }
    }
    match String.after(value: read "abc", delimiter: read "=") {
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

    assert_reg_vm_matches_compiled_backend("reg-vm-string-collection-option.rss", source, []);
}

#[test]
fn reg_vm_runs_json_pure_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let object = Json.value(value: read {"answer": 42, "ok": true})
    Log.write(message: read Json.kind(value: read object))
    if Json.is_object(value: read object) {
        Log.write(message: read "object")
    }

    let array = Json.values(items: read [
        Json.value(value: read {"n": 1}),
        Json.clone(value: read Json.value(value: read {"n": 2})),
    ])
    Log.write(message: read Json.kind(value: read array))
    if Json.is_array(value: read array) {
        Log.write(message: read "array")
    }
    Log.write(message: read Json.to_string(value: read array))

    if Json.is_null(value: read object) {
        Log.write(message: read "unexpected-null")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-json-pure-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_json_encode_decode_like_compiled_backend() {
    let source = r#"
struct AgentToolArgs derives(Clone, JsonDecode) {
    path: Option<String>
    max_results: Option<Int>
    include_hidden: Option<Bool>
}

fn main() -> Result<Unit, JsonError> {
    let encoded = Json.encode(value: read {
        "path": "src",
        "max_results": 20,
        "include_hidden": true,
    })
    Log.write(message: read encoded)
    let decoded = Json.decode_text<AgentToolArgs>(text: read encoded)?
    match decoded.path {
        Some(path) => Log.write(message: read path)
        None => Log.write(message: read "missing")
    }
    match decoded.max_results {
        Some(max_results) => Log.write(message: read String.from_int(value: max_results))
        None => Log.write(message: read "missing")
    }
    match decoded.include_hidden {
        Some(include_hidden) => Log.write(message: read String.from_bool(value: include_hidden))
        None => Log.write(message: read "missing")
    }

    let value: JsonValue = {"path": "data"}
    let decoded_value = Json.decode<AgentToolArgs>(value: read value)?
    match decoded_value.path {
        Some(path) => Log.write(message: read path)
        None => Log.write(message: read "missing")
    }
    match decoded_value.max_results {
        Some(max_results) => Log.write(message: read String.from_int(value: max_results))
        None => Log.write(message: read "missing")
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-json-encode-decode.rss", source, []);
}

#[test]
fn reg_vm_runs_json_result_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Result<Unit, JsonError> {
    match Json.parse(text: read "{\"name\":\"rss\",\"count\":3,\"ok\":true,\"obj\":{\"b\":2,\"a\":1}}") {
        Ok(doc) => {
            match Json.object_len(value: read doc) {
                Ok(n) => {
                    Log.write(message: read String.from_int(value: n))
                }
                Err(error) => {
                    Log.write(message: read JsonError.message(error: read error))
                }
            }
            match Json.object_keys(value: read doc) {
                Ok(keys) => {
                    Log.write(message: read List.join<String>(list: read keys, separator: read "|"))
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

    let items = Json.parse(text: read "[1,\"bad\"]")?
    match Json.array_len(value: read items) {
        Ok(n) => {
            Log.write(message: read String.from_int(value: n))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.array_get(value: read items, index: 0) {
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
    match Json.array_get(value: read items, index: 9) {
        Ok(item) => {
            Log.write(message: read Json.to_string(value: read item))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    let hello = Json.parse(text: read "\"hello\"")?
    match Json.as_string(value: read hello) {
        Ok(text) => {
            Log.write(message: read text)
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    let yes = Json.parse(text: read "true")?
    match Json.as_bool(value: read yes) {
        Ok(flag) => {
            if flag {
                Log.write(message: read "true")
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.json_parse(text: read "{bad") {
        Ok(value) => {
            Log.write(message: read Json.to_string(value: read value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-json-result-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_json_field_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Result<Unit, JsonError> {
    let doc = Json.parse(text: read "{\"name\":\"rss\",\"count\":3,\"ok\":true,\"none\":null,\"bad_int\":\"x\"}")?

    match Json.field(value: read doc, name: read "name") {
        Ok(value) => {
            Log.write(message: read Json.to_string(value: read value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_string(value: read doc, name: read "name") {
        Ok(value) => {
            Log.write(message: read value)
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_int(value: read doc, name: read "count") {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_bool(value: read doc, name: read "ok") {
        Ok(value) => {
            if value {
                Log.write(message: read "true")
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_optional(value: read doc, name: read "name") {
        Ok(value) => {
            match value {
                Some(item) => {
                    Log.write(message: read Json.to_string(value: read item))
                }
                None => {
                    Log.write(message: read "optional-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_optional_string(value: read doc, name: read "missing") {
        Ok(value) => {
            match value {
                Some(text) => {
                    Log.write(message: read text)
                }
                None => {
                    Log.write(message: read "missing-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_optional_int(value: read doc, name: read "none") {
        Ok(value) => {
            match value {
                Some(n) => {
                    Log.write(message: read String.from_int(value: n))
                }
                None => {
                    Log.write(message: read "null-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_optional_bool(value: read doc, name: read "ok") {
        Ok(value) => {
            match value {
                Some(flag) => {
                    if flag {
                        Log.write(message: read "optional-bool")
                    }
                }
                None => {
                    Log.write(message: read "optional-bool-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_int(value: read doc, name: read "bad_int") {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    let scalar = Json.parse(text: read "1")?
    match Json.field(value: read scalar, name: read "x") {
        Ok(value) => {
            Log.write(message: read Json.to_string(value: read value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_optional(value: read scalar, name: read "x") {
        Ok(value) => {
            match value {
                Some(item) => {
                    Log.write(message: read Json.to_string(value: read item))
                }
                None => {
                    Log.write(message: read "scalar-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.field_optional_bool(value: read scalar, name: read "x") {
        Ok(value) => {
            match value {
                Some(flag) => {
                    if flag {
                        Log.write(message: read "unexpected")
                    }
                }
                None => {
                    Log.write(message: read "runtime-diverges")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-json-field-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_json_array_conversion_intrinsics_like_interpreter() {
    let source = r#"
struct JsonAcc {
    total: Int
}

fn main() -> Result<Unit, JsonError> {
    let mixed = Json.parse(text: read "[\"profile\",\"project\",1]")?
    if Json.array_contains_string(value: read mixed, item: read "profile")? {
        Log.write(message: read "has-profile")
    }
    if Json.array_contains_substring(value: read mixed, text: read "roj")? {
        Log.write(message: read "has-substring")
    }
    if Json.array_contains_prefix(value: read mixed, prefix: read "pro")? {
        Log.write(message: read "has-prefix")
    }
    match Json.array_strings(value: read mixed) {
        Ok(items) => {
            Log.write(message: read List.join<String>(list: read items, separator: read "|"))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }

    let string_values = Json.parse(text: read "[\"a\",\"b\"]")?
    let strings = Json.array_strings(value: read string_values)?
    Log.write(message: read List.join<String>(list: read strings, separator: read ","))

    let int_values = Json.parse(text: read "[1,2]")?
    let ints = Json.array_ints(value: read int_values)?
    Log.write(message: read String.from_int(value: ints[1]))
    let count = Json.array_count_where(value: read int_values, predicate: |item| {
        let parsed = Json.as_int(value: read item)?
        return Ok(parsed > 1)
    })?
    Log.write(message: read String.from_int(value: count))
    let folded = Json.array_fold<JsonAcc>(value: read int_values, initial: read JsonAcc(total: 0), folder: |state, item| {
        let parsed = Json.as_int(value: read item)?
        return Ok(JsonAcc(total: state.total + parsed))
    })?
    Log.write(message: read String.from_int(value: folded.total))

    let bool_values = Json.parse(text: read "[true,false]")?
    let bools = Json.array_bools(value: read bool_values)?
    if bools[0] {
        Log.write(message: read "bool-true")
    }

    let empty_values = Json.parse(text: read "[]")?
    let empty_strings = Json.array_strings(value: read empty_values)?
    Log.write(message: read String.from_int(value: List.len<String>(list: read empty_strings)))

    let bad_int_values = Json.parse(text: read "[\"x\"]")?
    match Json.array_ints(value: read bad_int_values) {
        Ok(items) => {
            Log.write(message: read String.from_int(value: List.len<Int>(list: read items)))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    let bad_bool_values = Json.parse(text: read "[1]")?
    match Json.array_bools(value: read bad_bool_values) {
        Ok(items) => {
            Log.write(message: read String.from_int(value: List.len<Bool>(list: read items)))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    let scalar_value = Json.parse(text: read "1")?
    match Json.array_contains_string(value: read scalar_value, item: read "x") {
        Ok(found) => {
            if found {
                Log.write(message: read "unexpected")
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend(
        "reg-vm-json-array-conversion-intrinsics.rss",
        source,
        [],
    );
}

#[test]
fn reg_vm_runs_json_builder_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read Json.quote_string(value: read "a\"b"))

    let string_field = Json.string_field(name: read "name", value: read "rss")
    let int_field = Json.int_field(name: read "count", value: 3)
    let bool_field = Json.bool_field(name: read "ok", value: true)
    let raw_field = Json.raw_field(name: read "raw", value: read "{\"x\":1}")

    Log.write(message: read string_field)
    Log.write(message: read int_field)
    Log.write(message: read bool_field)
    Log.write(message: read raw_field)

    Log.write(message: read Json.object(fields: read [
        string_field,
        int_field,
        bool_field,
        raw_field,
    ]))
    Log.write(message: read Json.array(items: read ["1", "true", "{\"x\":1}"]))
    Log.write(message: read Json.string_array(items: read ["a", "b\"c"]))

    let values = Json.strings(items: read ["a", "b"])
    Log.write(message: read Json.to_string(value: read values))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-json-builder-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_json_path_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Result<Unit, JsonError> {
    let doc = Json.parse(text: read "{\"choices\":[{\"message\":{\"content\":\"done\",\"count\":2,\"ok\":true,\"none\":null}}],\"raw\":{\"x\":1}}")?

    match Json.at_string(value: read doc, path: read "$.choices[0].message.content") {
        Ok(text) => {
            Log.write(message: read text)
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at_int(value: read doc, path: read "choices[0].message.count") {
        Ok(n) => {
            Log.write(message: read String.from_int(value: n))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at_bool(value: read doc, path: read "choices[0].message.ok") {
        Ok(flag) => {
            if flag {
                Log.write(message: read "true")
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at_to_string(value: read doc, path: read "raw") {
        Ok(text) => {
            Log.write(message: read text)
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at(value: read doc, path: read "choices[0].message") {
        Ok(value) => {
            Log.write(message: read Json.to_string(value: read value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.value_at(value: read doc, path: read "$.raw.x") {
        Ok(value) => {
            Log.write(message: read Json.to_string(value: read value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at_optional(value: read doc, path: read "choices[0].message.none") {
        Ok(value) => {
            match value {
                Some(item) => {
                    Log.write(message: read Json.to_string(value: read item))
                }
                None => {
                    Log.write(message: read "optional-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at_optional_string(value: read doc, path: read "choices[0].message.none") {
        Ok(value) => {
            match value {
                Some(text) => {
                    Log.write(message: read text)
                }
                None => {
                    Log.write(message: read "null-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at_optional_int(value: read doc, path: read "choices[0].message.count") {
        Ok(value) => {
            match value {
                Some(n) => {
                    Log.write(message: read String.from_int(value: n))
                }
                None => {
                    Log.write(message: read "int-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at_optional_bool(value: read doc, path: read "choices[0].message.ok") {
        Ok(value) => {
            match value {
                Some(flag) => {
                    if flag {
                        Log.write(message: read "optional-bool")
                    }
                }
                None => {
                    Log.write(message: read "bool-none")
                }
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.at_optional_int(value: read doc, path: read "choices[0].message.content") {
        Ok(_) => {
            Log.write(message: read "unexpected")
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }

    let fallback_value = Json.parse(text: read "{\"fallback\":true}")?
    Log.write(message: read Json.to_string(value: read Json.at_or(value: read doc, path: read "missing.path", fallback: read fallback_value)))
    Log.write(message: read Json.at_string_or(value: read doc, path: read "missing.path", fallback: read "fallback"))
    Log.write(message: read String.from_int(value: Json.at_int_or(value: read doc, path: read "choices[9]", fallback: 7)))
    if Json.at_bool_or(value: read doc, path: read "choices[0].message.missing", fallback: true) {
        Log.write(message: read "bool-fallback")
    }
    Log.write(message: read Json.at_to_string_or(value: read doc, path: read "missing.path", fallback: read "{\"fallback\":true}"))

    match Json.at(value: read doc, path: read "choices[bad]") {
        Ok(value) => {
            Log.write(message: read Json.to_string(value: read value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-json-path-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_json_text_path_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Result<Unit, JsonError> {
    let text = "{\"profile\":{\"name\":\"rss\",\"active\":true,\"nested\":{\"x\":1},\"none\":null},\"items\":[{\"id\":1},{\"id\":2}]}"

    match Json.string_at(text: read text, path: read "profile.name") {
        Ok(value) => {
            Log.write(message: read value)
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.int_at(text: read text, path: read "items[1].id") {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.bool_at(text: read text, path: read "profile.active") {
        Ok(value) => {
            if value {
                Log.write(message: read "true")
            }
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }
    match Json.to_string_at(text: read text, path: read "profile.nested") {
        Ok(value) => {
            Log.write(message: read value)
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }

    Log.write(message: read Json.string_at_or(text: read text, path: read "profile.none", fallback: read "string-fallback"))
    Log.write(message: read Json.json_string_at_or(text: read text, path: read "missing.path", fallback: read "json-string-fallback"))
    Log.write(message: read String.from_int(value: Json.int_at_or(text: read text, path: read "items[9].id", fallback: 123)))
    Log.write(message: read String.from_int(value: Json.json_int_at_or(text: read "{bad", path: read "x", fallback: 124)))
    if Json.bool_at_or(text: read text, path: read "profile.missing", fallback: true) {
        Log.write(message: read "bool-fallback")
    }
    if Json.json_bool_at_or(text: read text, path: read "profile.name", fallback: true) {
        Log.write(message: read "json-bool-type-fallback")
    }
    Log.write(message: read Json.to_string_at_or(text: read text, path: read "items[99]", fallback: read "json-fallback"))

    match Json.string_at(text: read text, path: read "items[bad]") {
        Ok(_) => {
            Log.write(message: read "unexpected")
        }
        Err(error) => {
            Log.write(message: read JsonError.message(error: read error))
        }
    }

    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-json-text-path-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_csv_row_intrinsics_like_interpreter() {
    let root = std::env::current_dir()
        .expect("cwd should be available")
        .join("target")
        .join(format!("rss-vm-csv-{}-fixture", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("csv fixture dir should be created");
    let csv_path = root.join("data.csv");
    fs::write(&csv_path, "name,amount\nalpha,10\nbeta,20\n").expect("csv fixture should write");
    let csv_arg = csv_path.display().to_string();

    let source = r#"
features: local

fn main() -> Result<Unit, CsvError> {
    let path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 0), default: read "missing.csv"))
    local buffer = RowBuffer.new(size: 4096)
    with Csv.open_read(path: read path)? as file {
        Csv.read_into(file: mut file, buffer: mut buffer)?
    }

    let row = Csv.parse_row(buffer: read buffer)?
    let name = Row.field_string(row: read row, index: 0)?
    let amount = Row.field_string(row: read row, index: 1)?
    Log.write(message: read name)
    Log.write(message: read amount)

    match Row.field_string(row: read row, index: 5) {
        Ok(value) => {
            Log.write(message: read value)
        }
        Err(error) => {
            Log.write(message: read "field-error")
        }
    }

    match Csv.rows(path: read path, buffer_size: 16) {
        Ok(stream) => {
            match Stream.collect_list<Row>(stream: read stream) {
                Ok(rows) => {
                    Log.write(message: read String.from_int(value: List.len<Row>(list: read rows)))
                    let first = Row.field_string(row: read rows[0], index: 0)?
                    let second = Row.field_string(row: read rows[1], index: 1)?
                    Log.write(message: read first)
                    Log.write(message: read second)
                }
                Err(error) => {
                    Log.write(message: read ChannelError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Log.write(message: read ChannelError.message(error: read error))
        }
    }

    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend(
        "reg-vm-csv-row-intrinsics.rss",
        source,
        [csv_arg.as_str()],
    );
    let _ = fs::remove_dir_all(&root);
}
