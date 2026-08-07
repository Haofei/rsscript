//! register-VM execution: arithmetic and uncategorized
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn reg_vm_runs_math_random_uuid_and_modulo_like_interpreter() {
    let source = r#"

fn float_mix(value: Float, salt: Float) -> Float {
    return value * 2.0 + salt
}

fn main() -> Unit {
    Output.write(message: read String.from_int(value: 17 % 5))
    Output.write(message: read String.from_int(value: Math.abs(value: -9)))
    Output.write(message: read String.from_int(value: Math.min(left: 4, right: 7)))
    Output.write(message: read String.from_int(value: Math.max(left: 4, right: 7)))
    Output.write(message: read String.from_int(value: Math.clamp(value: 12, min: 0, max: 10)))
    Output.write(message: read String.from_float(value: Math.abs_float(value: -2.5)))
    Output.write(message: read String.from_float(value: Math.min_float(left: 4.5, right: 7.25)))
    Output.write(message: read String.from_float(value: Math.max_float(left: 4.5, right: 7.25)))
    Output.write(message: read String.from_float(value: Math.clamp_float(value: 12.5, min: 0.5, max: 10.5)))
    Output.write(message: read String.from_float(value: Math.pow_float(base: 2.0, exponent: 3.0)))
    Output.write(message: read String.from_float(value: Math.sqrt(value: 9.0)))
    Output.write(message: read Float.to_string(value: read Math.sqrt(value: 16.0)))
    Output.write(message: read String.from_float(value: Math.cos(value: 0.0)))
    Output.write(message: read String.from_float(value: Math.exp(value: 0.0)))
    Output.write(message: read String.from_float(value: Math.log(value: 1.0)))
    Output.write(message: read String.from_float(value: Math.tanh(value: 0.0)))
    let finite = 1.5
    let infinite = 1.0 / 0.0
    let nan = 0.0 / 0.0
    Output.write(message: read String.from_bool(value: Float.is_finite(value: read finite)))
    Output.write(message: read String.from_bool(value: Float.is_infinite(value: read infinite)))
    Output.write(message: read String.from_bool(value: Float.is_nan(value: read nan)))
    match String.parse_float(value: read "12.5") {
        Some(value) => Output.write(message: read String.from_float(value: value))
        None => Output.write(message: read "float-none")
    }
    match String.parse_float(value: read "not-float") {
        Some(value) => Output.write(message: read String.from_float(value: value))
        None => Output.write(message: read "invalid-float")
    }
    Output.write(message: read String.from_int(value: Math.floor(value: 3.9)))
    Output.write(message: read String.from_int(value: Math.ceil(value: 3.1)))
    Output.write(message: read String.from_int(value: Math.round(value: 3.5)))
    Output.write(message: read String.from_float(value: 1.5 + 2.25))
    Output.write(message: read String.from_float(value: 9.0 - 2.5))
    Output.write(message: read String.from_float(value: 3.0 * 2.5))
    Output.write(message: read String.from_float(value: 7.5 / 2.5))
    Output.write(message: read String.from_float(value: float_mix(value: 1.5, salt: 0.5)))
    if 1.0 < 2.0 {
        Output.write(message: read "float-condition")
    }
    if 5.5 > 5.0 && 5.0 <= 5.0 {
        Output.write(message: read "float-compare")
    }

    let fixed = Random.int(min: 7, max: 7)
    Output.write(message: read String.from_int(value: fixed))
    Output.write(message: read String.from_int(value: Math.floor(value: Random.float())))
    let bytes = Random.bytes(len: 4)
    Output.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    let token = Random.string(len: 8)
    Output.write(message: read String.from_int(value: String.len(value: read token)))
    let maybe = Random.bool()
    if maybe {
        Output.write(message: read "bool")
    } else {
        Output.write(message: read "bool")
    }
    let id = Uuid.new_v4()
    Output.write(message: read String.from_int(value: String.len(value: read id)))
    if String.contains(value: read id, needle: read "-") {
        Output.write(message: read "uuid")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-math-random-uuid.rss", source, []);
}

#[test]
fn reg_vm_runs_format_date_and_int_bit_helpers_like_backend() {
    let source = r#"

fn main() -> Unit {
    Output.write(message: read String.format(template: read "hello {}, {{}} {}", args: read ["rss", "vm"]))
    Output.write(message: read String.format(template: read "missing {}", args: read List<String>.new()))

    Output.write(message: read String.from_int(value: Int.bit_and(left: 6, right: 3)))
    Output.write(message: read String.from_int(value: Int.bit_or(left: 4, right: 1)))
    Output.write(message: read String.from_int(value: Int.bit_xor(left: 6, right: 3)))
    Output.write(message: read String.from_int(value: Int.bit_not(value: 0)))
    Output.write(message: read String.from_int(value: Int.shift_left(value: 3, bits: 2)))
    Output.write(message: read String.from_int(value: Int.shift_right(value: 16, bits: 2)))
    Output.write(message: read String.from_int(value: 6 & 3))
    Output.write(message: read String.from_int(value: 4 | 1))
    Output.write(message: read String.from_int(value: 6 ^ 3))
    Output.write(message: read String.from_int(value: 3 << 2))
    Output.write(message: read String.from_int(value: 16 >> 2))
    Output.write(message: read String.from_int(value: 1 | 2 & 3))
    Output.write(message: read String.from_bool(value: !false))
    Output.write(message: read String.from_bool(value: !(1 > 2)))
    Output.write(message: read String.from_int(value: ~0))
    Output.write(message: read String.from_int(value: ~5))

    match Date.parse_ymd(value: read "2024-02-29") {
        Some(value) => {
            Output.write(message: read Date.format_ymd(unix_ms: value))
            Output.write(message: read Date.format_iso(unix_ms: value))
            Output.write(message: read String.from_int(value: Date.year(unix_ms: value)))
            Output.write(message: read String.from_int(value: Date.month(unix_ms: value)))
            Output.write(message: read String.from_int(value: Date.day(unix_ms: value)))
            Output.write(message: read String.from_int(value: Date.hour(unix_ms: value)))
            Output.write(message: read String.from_int(value: Date.minute(unix_ms: value)))
            Output.write(message: read String.from_int(value: Date.second(unix_ms: value)))
            Output.write(message: read Date.format_ymd(unix_ms: Date.add_days(unix_ms: value, days: 1)))
            Output.write(message: read String.from_int(value: Date.days_between(start_unix_ms: value, end_unix_ms: Date.add_days(unix_ms: value, days: 3))))
            Output.write(message: read String.from_int(value: Date.add_ms(unix_ms: value, ms: 250) - value))
            Output.write(message: read Date.format_iso(unix_ms: Date.start_of_day(unix_ms: Date.add_ms(unix_ms: value, ms: 45678))))
            Output.write(message: read String.from_int(value: Date.weekday(unix_ms: value)))
            Output.write(message: read String.from_bool(value: Date.is_leap_year(year: 2024)))
            Output.write(message: read String.from_int(value: Date.days_in_month(year: 2024, month: 2)))
            Output.write(message: read String.from_bool(value: Date.is_leap_year(year: 2023)))
            Output.write(message: read String.from_int(value: Date.days_in_month(year: 2023, month: 13)))
        }
        None => Output.write(message: read "date-none")
    }
    match Date.parse_iso(value: read "2024-02-29T12:34:56.789+02:00") {
        Some(value) => {
            Output.write(message: read Date.format_iso(unix_ms: value))
            Output.write(message: read String.from_int(value: Date.hour(unix_ms: value)))
            Output.write(message: read String.from_int(value: Date.minute(unix_ms: value)))
            Output.write(message: read String.from_int(value: Date.second(unix_ms: value)))
        }
        None => Output.write(message: read "iso-none")
    }
    match Date.parse_iso(value: read "not-an-iso-date") {
        Some(value) => Output.write(message: read Date.format_iso(unix_ms: value))
        None => Output.write(message: read "invalid-iso-date")
    }
    match Date.parse_ymd(value: read "not-a-date") {
        Some(value) => Output.write(message: read Date.format_ymd(unix_ms: value))
        None => Output.write(message: read "invalid-date")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-format-date-bit-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_basic_args_assert_and_int_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let args = Args.all()
    Assert.equal_int(left: Args.count(), right: 2)
    Assert.equal_int(left: List.len<String>(list: read args), right: 2)
    Assert.equal(left: read Int.to_string(value: read 42), right: read "42")
    Assert.equal_bool(left: true, right: true)
    match Args.get(index: 0) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "missing-first")
        }
    }
    match Args.get(index: 99) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "missing-none")
        }
    }
    match Args.get(index: 0 - 1) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "negative-none")
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend(
        "reg-vm-basic-args-assert-int.rss",
        source,
        ["first", "second"],
    );
}

#[test]
fn reg_vm_runs_duration_and_url_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let short = Duration.ms(value: 750)
    let long = Duration.seconds(value: 2)
    let total = Duration.add(left: read short, right: read long)
    Output.write(message: read String.from_int(value: Duration.as_ms(value: read short)))
    Output.write(message: read String.from_int(value: Duration.as_ms(value: read long)))
    Output.write(message: read String.from_int(value: Duration.as_ms(value: read total)))
    Output.write(message: read String.from_int(value: Duration.as_seconds(value: read total)))

    let url = Url.from_string(value: read "https://example.test/path")
    Output.write(message: read Url.to_string(url: read url))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-duration-url-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_hash_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let digest = Hash.sha256_string(value: read "abc")
    Output.write(message: read digest)
    let bytes = Bytes.from_string(value: read "abc")
    Output.write(message: read Hash.sha256_bytes(value: read bytes))
    Assert.equal(left: read digest, right: read Hash.sha256_bytes(value: read bytes))
    Assert.equal(
        left: read Hash.sha256_string(value: read "é"),
        right: read Hash.sha256_bytes(value: read Bytes.from_string(value: read "é"))
    )
    local uints = List.new<Int>()
    List.push<Int>(list: mut uints, value: read 97)
    List.push<Int>(list: mut uints, value: read 98)
    List.push<Int>(list: mut uints, value: read 99)
    let uint_bytes = Bytes.from_uints(values: read uints)
    let roundtrip = Bytes.to_uints(value: read uint_bytes)
    Assert.equal_int(left: List.get<Int>(list: read roundtrip, index: 0), right: 97)
    Assert.equal_int(left: List.get<Int>(list: read roundtrip, index: 1), right: 98)
    Assert.equal_int(left: List.get<Int>(list: read roundtrip, index: 2), right: 99)
    let sha3_224 = Bytes.to_uints(value: read Hash.sha3_224_bytes(value: read uint_bytes))
    Assert.equal_int(left: List.len<Int>(list: read sha3_224), right: 28)
    Assert.equal_int(left: List.get<Int>(list: read sha3_224, index: 0), right: 230)
    Assert.equal_int(left: List.get<Int>(list: read sha3_224, index: 1), right: 66)
    let sha3_256 = Bytes.to_uints(value: read Hash.sha3_256_bytes(value: read uint_bytes))
    Assert.equal_int(left: List.len<Int>(list: read sha3_256), right: 32)
    Assert.equal_int(left: List.get<Int>(list: read sha3_256, index: 0), right: 58)
    Assert.equal_int(left: List.get<Int>(list: read sha3_256, index: 1), right: 152)
    let shake = Bytes.to_uints(value: read Hash.shake128_bytes(value: read uint_bytes, out_len: 16))
    Assert.equal_int(left: List.len<Int>(list: read shake), right: 16)
    Assert.equal_int(left: List.get<Int>(list: read shake, index: 0), right: 88)
    Assert.equal_int(left: List.get<Int>(list: read shake, index: 1), right: 129)
    let hmac = Hmac.sha256_string(key: read "key", value: read "abc")
    Output.write(message: read String.from_int(value: String.len(value: read hmac)))
    Assert.equal(
        left: read hmac,
        right: read Hmac.sha256_bytes(
            key: read Bytes.from_string(value: read "key"),
            value: read Bytes.from_string(value: read "abc")
        )
    )
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-hash-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_encoding_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let encoded = Base64.encode(value: read "rsscript")
    Output.write(message: read encoded)

    match Base64.decode_string(text: read encoded) {
        Ok(value) => Output.write(message: read value)
        Err(error) => Output.write(message: read DecodeError.message(error: read error))
    }

    let bytes = String.to_bytes(value: read "hex")
    Output.write(message: read Base64.encode_bytes(value: read bytes))

    match Base64.decode(text: read "%%%") {
        Ok(value) => Output.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Output.write(message: read DecodeError.message(error: read error))
    }

    let hexed = Hex.encode_string(value: read "Az")
    Output.write(message: read hexed)
    Output.write(message: read Hex.encode(value: read bytes))

    match Hex.decode(text: read hexed) {
        Ok(value) => Output.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Output.write(message: read DecodeError.message(error: read error))
    }

    match Hex.decode(text: read "not-hex") {
        Ok(value) => Output.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Output.write(message: read DecodeError.message(error: read error))
    }

    match Hex.decode(text: read "1f8b08000000000002ff4b4c4a0600c241243503000000") {
        Ok(gzipped) => {
            match Gzip.decompress_bytes(value: read gzipped) {
                Ok(value) => {
                    Output.write(message: read String.from_int(value: Bytes.len(value: read value)))
                    Output.write(message: read Hex.encode(value: read value))
                }
                Err(error) => Output.write(message: read DecodeError.message(error: read error))
            }
        }
        Err(error) => Output.write(message: read DecodeError.message(error: read error))
    }

    let bad_gzip = String.to_bytes(value: read "not gzip")
    match Gzip.decompress_bytes(value: read bad_gzip) {
        Ok(value) => Output.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Output.write(message: read DecodeError.message(error: read error))
    }

    let component = Url.encode_component(value: read "a b/é?x=1")
    Output.write(message: read component)

    match Url.decode_component(value: read component) {
        Ok(value) => Output.write(message: read value)
        Err(error) => Output.write(message: read DecodeError.message(error: read error))
    }

    match Url.decode_component(value: read "%FF") {
        Ok(value) => Output.write(message: read value)
        Err(error) => Output.write(message: read DecodeError.message(error: read error))
    }

    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-encoding-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_pure_path_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let root = Path.from_string(value: read "fixtures")
    let path = Path.join(base: read root, child: read "rsscript-path.txt")

    Output.write(message: read Path.to_string(path: read path))
    Output.write(message: read Path.to_string(path: read String.to_path(value: read "fixtures/rsscript-path.txt")))
    Output.write(message: read Path.to_string(path: read Path.normalize(path: read Path.join(base: read path, child: read ".."))))

    match Path.file_name(path: read path) {
        Some(name) => {
            Output.write(message: read name)
        }
        None => {
            Output.write(message: read "no-name")
        }
    }
    match Path.extension(path: read path) {
        Some(extension) => {
            Output.write(message: read extension)
        }
        None => {
            Output.write(message: read "no-extension")
        }
    }
    match Path.parent(path: read path) {
        Some(parent) => {
            Output.write(message: read Path.to_string(path: read parent))
        }
        None => {
            Output.write(message: read "no-parent")
        }
    }

    if Path.is_absolute(path: read Path.from_string(value: read "/tmp/rsscript")) {
        Output.write(message: read "absolute")
    }
    if Path.starts_with(path: read path, base: read root) {
        Output.write(message: read "starts")
    }
    Output.write(message: read Path.to_string(path: read Path.with_extension(path: read path, extension: read "json")))

    match Path.safe_relative(value: read "fixtures/./rsscript-path.txt") {
        Ok(safe) => {
            Output.write(message: read Path.to_string(path: read safe))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }
    match String.safe_relative(value: read "../escape") {
        Ok(safe) => {
            Output.write(message: read Path.to_string(path: read safe))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }
    match Path.resolve_relative(root: read root, relative: read "rsscript-path.txt") {
        Ok(resolved) => {
            Output.write(message: read Path.to_string(path: read resolved))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    let from_method: Url = String.to_url(value: read "https://example.test/from-method")
    Output.write(message: read Url.to_string(url: read from_method))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-pure-path-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_logical_short_circuit_like_interpreter() {
    let source = r#"
fn zero() -> Int {
    return String.len(value: read "")
}

fn explode() -> Bool {
    return 1 / zero() == 0
}

fn main() -> Unit {
    let left = false && explode()
    let right = true || explode()
    if left == false && right == true {
        Output.write(message: read "ok")
        return Unit
    }
    Output.write(message: read "bad")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-logical-short-circuit.rss", source, []);
}

#[test]
fn reg_vm_runs_log_and_workspace_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    Output.write(message: read "stdout line")
    Output.write_json(value: read Json.value(value: read {"stream": "stdout", "count": 1}))
    Output.error(message: read "stderr line")
    Output.error_json(value: read Json.value(value: read {"stream": "stderr", "count": 2}))
    Output.trace(event: read "parity.event", message: read "traced")

    let root = Env.run_workspace_root()
    match Workspace.resolve(root: read root, relative: read "Cargo.toml") {
        Ok(path) => {
            if Path.exists(path: read path) {
                Output.write(message: read "workspace-resolved")
            }
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-log-workspace.rss", source, []);
}

#[test]
fn reg_vm_runs_object_literal_like_interpreter() {
    let source = r#"
fn main() -> JsonValue {
    return {"ok": true, "name": "rss", "count": 3, "tags": ["agent", "json"]}
}
"#;

    assert_reg_vm_matches_compiled_backend_return(
        "reg-vm-object-literal.rss",
        source,
        [],
        CompiledReturnHarness::JsonValue,
    );
}

#[test]
fn reg_vm_runs_try_ok_like_interpreter() {
    let source = r#"
fn checked(value: Int) -> Result<Int, String> {
    return Ok(value + 1)
}

fn main() -> Result<Unit, String> {
    let value = checked(value: 4)?
    Output.write(message: read String.from_int(value: value))
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-try-ok.rss", source, []);
}

#[test]
fn reg_vm_runs_pipeline_chain_like_interpreter() {
    let source = r#"

fn main() -> Unit {
    let mut index = 0
    local values = List<Int>.new()
    while index < 10 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    let pipeline = Pipeline.map<Int, Int>(
        pipeline: read Pipeline.filter<Int>(
            pipeline: read List.pipeline<Int>(list: read values),
            predicate: |value| {
                let half = value / 2
                return half * 2 == value
            },
        ),
        mapper: |value| {
            return value * 3 + 1
        },
    )
    let collected = Pipeline.collect<Int>(pipeline: read pipeline)
    let total = List.fold<Int, Int>(
        list: read collected,
        initial: read 0,
        folder: |state, value| {
            return state + value
        },
    )

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-pipeline-chain.rss", source, []);
}

#[test]
fn vm_runs_pipeline_chain_like_interpreter() {
    let source = r#"

struct Acc {
    total: Int
}

fn main() -> Unit {
    let mut index = 0
    local values = List<Int>.new()
    while index < 10 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    let pipeline = Pipeline.map<Int, Int>(
        pipeline: read Pipeline.filter<Int>(
            pipeline: read List.pipeline<Int>(list: read values),
            predicate: |value| {
                let half = value / 2
                return half * 2 == value
            },
        ),
        mapper: |value| {
            return value * 3 + 1
        },
    )
    let collected = Pipeline.collect<Int>(pipeline: read pipeline)
    let acc = List.fold<Int, Acc>(
        list: read collected,
        initial: read Acc(total: 0),
        folder: |state, value| {
            return Acc(total: state.total + value)
        },
    )

    Output.write(message: read String.from_int(value: acc.total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-pipeline-chain.rss", source, []);
}

#[test]
fn reg_vm_runs_diff_patch_ord_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let original = "one\ntwo\nthree\n"
    let changed = "one\n2\nthree\n"
    let patch = Diff.unified(old: read original, new: read changed)
    match Patch.apply_text(original: read original, patch: read patch) {
        Ok(applied) => {
            Output.write(message: read applied)
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    let empty_patch = Diff.unified(old: read original, new: read original)
    match Patch.apply_text(original: read original, patch: read empty_patch) {
        Ok(applied) => {
            Assert.equal(left: read applied, right: read original)
            Output.write(message: read "empty-ok")
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    match Patch.apply_text(original: read "old\n", patch: read "--- old\n+++ new\n@@ -1,1 +1,1 @@\n-bad\n+new\n") {
        Ok(applied) => {
            Output.write(message: read applied)
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    Output.write(message: read String.from_int(value: Ord.compare<Int>(self: read 1, other: read 2)))
    Output.write(message: read String.from_int(value: Ord.compare<String>(self: read "b", other: read "a")))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-diff-patch-ord.rss", source, []);
}

#[test]
fn reg_vm_runs_file_parse_intrinsics_like_interpreter() {
    let root = std::env::current_dir()
        .expect("cwd should be available")
        .join("target")
        .join(format!(
            "rss-vm-file-parse-{}-{}",
            std::process::id(),
            "fixtures"
        ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("fixture dir should be created");

    let json_path = root.join("data.json");
    let toml_path = root.join("data.toml");
    let yaml_path = root.join("data.yaml");
    fs::write(&json_path, r#"{"name":"rss","count":3}"#).expect("json fixture should write");
    fs::write(&toml_path, "name = \"rss\"\ncount = 4\n").expect("toml fixture should write");
    fs::write(&yaml_path, "name: rss\ncount: 5\n").expect("yaml fixture should write");

    let args = [
        json_path.display().to_string(),
        toml_path.display().to_string(),
        yaml_path.display().to_string(),
    ];
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    let source = r#"
fn main() -> Result<Unit, JsonError> {
    let json_path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 0), default: read "missing-json"))
    let toml_path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 1), default: read "missing-toml"))
    let yaml_path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 2), default: read "missing-yaml"))

    let json = Json.parse_file(path: read json_path)?
    let json_name = Json.field(value: read json, name: read "name")?
    let json_count = Json.field(value: read json, name: read "count")?
    let json_name_text = Json.as_string(value: read json_name)?
    let json_count_int = Json.as_int(value: read json_count)?
    Output.write(message: read json_name_text)
    Output.write(message: read String.from_int(value: json_count_int))

    let toml = Toml.parse_file(path: read toml_path)?
    let toml_name = Json.field(value: read toml, name: read "name")?
    let toml_count = Json.field(value: read toml, name: read "count")?
    let toml_name_text = Json.as_string(value: read toml_name)?
    let toml_count_int = Json.as_int(value: read toml_count)?
    Output.write(message: read toml_name_text)
    Output.write(message: read String.from_int(value: toml_count_int))

    let yaml = Yaml.parse_file(path: read yaml_path)?
    let yaml_name = Json.field(value: read yaml, name: read "name")?
    let yaml_count = Json.field(value: read yaml, name: read "count")?
    let yaml_name_text = Json.as_string(value: read yaml_name)?
    let yaml_count_int = Json.as_int(value: read yaml_count)?
    Output.write(message: read yaml_name_text)
    Output.write(message: read String.from_int(value: yaml_count_int))

    match Json.parse_file(path: read Path.from_string(value: read "missing-json-file")) {
        Ok(value) => {
            Output.write(message: read Json.to_string(value: read value))
        }
        Err(error) => {
            Output.write(message: read JsonError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-file-parse-config.rss", source, arg_refs);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reg_vm_runs_cancellation_and_stream_intrinsics_like_interpreter() {
    let source = r#"

async fn main() -> Result<Unit, ChannelError> {
    local source = CancellationSource.new()
    let token = CancellationSource.token(source: read source)
    if !CancellationToken.is_cancelled(token: read token) {
        Output.write(message: read "not-cancelled")
    }
    CancellationSource.cancel(source: mut source)
    if CancellationToken.is_cancelled(token: read token) {
        Output.write(message: read "cancelled")
    }
    let second = CancellationSource.token(source: read source)
    if CancellationToken.is_cancelled(token: read second) {
        Output.write(message: read "second-cancelled")
    }

    local items = List<Int>.new()
    List.push<Int>(list: mut items, value: read 1)
    List.push<Int>(list: mut items, value: read 2)
    List.push<Int>(list: mut items, value: read 3)
    let stream: Stream<Int> = Stream.from_list<Int>(items: take items)
    match await Stream.next<Int>(stream: read stream)? {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "first-none")
        }
    }
    let remaining = Stream.collect_list<Int>(stream: read stream)?
    Output.write(message: read String.from_int(value: List.len<Int>(list: read remaining)))
    Output.write(message: read String.from_int(value: remaining[0]))
    Output.write(message: read String.from_int(value: remaining[1]))

    local empty_items = List<Int>.new()
    let empty_stream: Stream<Int> = Stream.from_list<Int>(items: take empty_items)
    match await Stream.next<Int>(stream: read empty_stream)? {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "empty-none")
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_output(
        "reg-vm-cancellation-stream.rss",
        source,
        [],
        "Ok { value: Unit }",
        "not-cancelled\ncancelled\nsecond-cancelled\n1\n2\n2\n3\nempty-none\n",
    );
}

#[test]
fn reg_vm_runs_http_request_builder_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> HttpRequest {
    let url = Url.from_string(value: read "https://example.test/api")
    local base = HttpRequest.json(url: read url, body: read "{\"ok\":true}")
    local timed = HttpRequest.with_timeout(request: take base, timeout_ms: 250)
    local retry = HttpRequest.with_retry(request: take timed, attempts: 3, backoff_ms: 50)
    local with_header = HttpRequest.with_header(request: take retry, name: read "X-Test", value: read "rss")
    let final_request = HttpRequest.with_header(request: take with_header, name: read "X-Trace", value: read "1")
    return final_request
}
"#;

    assert_reg_vm_matches_compiled_backend_return(
        "reg-vm-http-request-builder.rss",
        source,
        [],
        CompiledReturnHarness::HttpRequest,
    );
}

#[test]
fn reg_vm_runs_runtime_facade_batch_like_interpreter() {
    let source = r#"

async fn main() -> Result<Unit, String> {
    match Channel.bounded<Int>(capacity: 1) {
        Ok(channel) => {
            let sender = Channel.sender<Int>(channel: read channel)
            local channel_value = channel
            local receiver_result = Channel.receiver<Int>(channel: mut channel_value)
            match receiver_result {
                Ok(receiver) => {
                    local value = 41 + 1
                    match await Sender.send<Int>(sender: read sender, value: take value) {
                        Ok(_) => {
                            Output.write(message: read "sent")
                        }
                        Err(error) => {
                            Output.write(message: read ChannelError.message(error: read error))
                        }
                    }
                    match await Receiver.recv<Int>(receiver: read receiver) {
                        Ok(maybe_item) => {
                            match maybe_item {
                                Some(item) => {
                                    Output.write(message: read String.from_int(value: item))
                                }
                                None => {
                                    Output.write(message: read "none")
                                }
                            }
                        }
                        Err(error) => {
                            Output.write(message: read ChannelError.message(error: read error))
                        }
                    }
                }
                Err(error) => {
                    Output.write(message: read ChannelError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }

    let url = Url.from_string(value: read "https://example.test/api")
    match Http.get(url: read url) {
        Ok(response) => {
            Output.write(message: read HttpResponse.text(response: read response))
            Output.write(message: read String.from_int(value: Bytes.len(value: read HttpResponse.bytes(response: read response))))
        }
        Err(error) => {
            Output.write(message: read HttpError.message(error: read error))
        }
    }

    let stdout = Process.run_stdout(command: read "printf", args: read ["vm"])?
    Output.write(message: read stdout)
    let output = Process.run(command: read "printf", args: read ["ok"])?
    Output.write(message: read output.stdout)
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-runtime-facade-batch.rss", source, []);
}

#[test]
fn reg_vm_runs_tempdir_and_path_fs_intrinsics_like_interpreter() {
    let root = std::env::current_dir()
        .expect("cwd should be available")
        .join("target")
        .join(format!("rss-vm-tempdir-{}-fixture", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("tempdir fixture dir should be created");
    let file_path = root.join("marker.txt");
    fs::write(&file_path, "marker").expect("marker file should write");
    let root_arg = root.display().to_string();
    let file_arg = file_path.display().to_string();

    let source = r#"

fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rss-vm-tempdir"))
    let marker = Path.from_string(value: read Args.get_or_default(index: 1, default: read "target/rss-vm-tempdir/marker.txt"))

    if Path.exists(path: read root) {
        Output.write(message: read "root-exists")
    }
    if Path.is_dir(path: read root) {
        Output.write(message: read "root-dir")
    }
    if Path.exists(path: read marker) {
        Output.write(message: read "marker-exists")
    }
    if Path.is_file(path: read marker) {
        Output.write(message: read "marker-file")
    }

    with TempDir.new_in(parent: read root)? as child {
        let path = TempDir.path(dir: read child)
        if Path.is_dir(path: read path) {
            Output.write(message: read "child-dir")
        }
    }

    with TempDir.new()? as created {
        let path = TempDir.path(dir: read created)
        if Path.is_dir(path: read path) {
            Output.write(message: read "created-dir")
        }
    }

    with TempDir.new_in(parent: read root)? as kept {
        let path = TempDir.keep(dir: take kept)
        if Path.is_dir(path: read path) {
            Output.write(message: read "kept-dir")
        }
    }

    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend(
        "reg-vm-tempdir-path-fs.rss",
        source,
        [root_arg.as_str(), file_arg.as_str()],
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reg_vm_runs_file_core_intrinsics_like_interpreter() {
    let root = std::env::current_dir()
        .expect("cwd should be available")
        .join("target")
        .join(format!("rss-vm-file-core-{}-fixture", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("file fixture dir should be created");
    let root_arg = root.display().to_string();

    let source = r#"

fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rss-vm-file-core"))
    let path_file = Path.join(base: read root, child: read "path.txt")
    File.write_string_to_path(path: read path_file, text: read "file text")?
    if File.exists(path: read path_file) {
        Output.write(message: read "path-exists")
    }
    let file_text = File.read_string(path: read path_file)?
    Output.write(message: read file_text)

    let bytes_file = Path.join(base: read root, child: read "bytes.bin")
    File.write_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "abc"))?
    let bytes = File.read_bytes(path: read bytes_file)?
    Output.write(message: read String.from_int(value: Bytes.len(value: read bytes)))

    let handle_file = Path.join(base: read root, child: read "handle.txt")
    with File.open_write(path: read handle_file)? as writer {
        File.write(file: mut writer, data: read Bytes.from_string(value: read "ab"))?
        File.write_string(file: mut writer, text: read "cd")?
    }
    with File.open(path: read handle_file)? as reader {
        let all = File.read_all(file: mut reader)?
        Output.write(message: read String.from_int(value: Bytes.len(value: read all)))
        let empty = File.read_all(file: mut reader)?
        Output.write(message: read String.from_int(value: Bytes.len(value: read empty)))
    }
    with File.open_read(path: read handle_file)? as reader_text {
        let text_all = File.read_all_string(file: mut reader_text)?
        Output.write(message: read text_all)
    }
    with File.open_read(path: read handle_file)? as reader_into {
        local into_buffer = Buffer.new(size: 0)
        if File.read_into(file: mut reader_into, buffer: mut into_buffer)? {
            Output.write(message: read String.from_int(value: Buffer.len(buffer: read into_buffer)))
        }
        if !File.read_into(file: mut reader_into, buffer: mut into_buffer)? {
            Output.write(message: read "read-into-empty")
        }
    }

    File.remove(path: read path_file)?
    if !File.exists(path: read path_file) {
        Output.write(message: read "removed")
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-file-core.rss", source, [root_arg.as_str()]);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reg_vm_runs_directory_and_file_stream_intrinsics_like_interpreter() {
    let root = std::env::current_dir()
        .expect("cwd should be available")
        .join("target")
        .join(format!("rss-vm-directory-{}-fixture", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("directory fixture root should be created");
    let root_arg = root.display().to_string();

    let source = r#"

fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rss-vm-directory"))
    let nested = Path.join(base: read root, child: read "nested")
    let deep = Path.join(base: read nested, child: read "deep")
    let single = Path.join(base: read root, child: read "single")

    Directory.create_all(path: read nested)?
    Directory.create_dir_all(path: read deep)?
    Directory.create(path: read single)?
    if Directory.exists(path: read root) {
        Output.write(message: read "root-exists")
    }
    if Directory.is_dir(path: read deep) {
        Output.write(message: read "deep-dir")
    }

    let path_file = Path.join(base: read nested, child: read "path.txt")
    Directory.write_string(path: read path_file, content: read "path text")?
    if Directory.is_file(path: read path_file) {
        Output.write(message: read "directory-file")
    }
    let path_text = Directory.read_string(path: read path_file)?
    Output.write(message: read path_text)
    let path_digest = Hash.sha256_file(path: read path_file)?
    Assert.equal(left: read path_digest, right: read "c6465e0abd2e3c2f5ccfe7f639ddc0f72282904663b09ddd8dffbe060be35f97")

    let metadata = Directory.metadata(path: read path_file)?
    if metadata.is_file {
        Output.write(message: read "metadata-file")
    }
    Output.write(message: read String.from_int(value: metadata.len))

    let bytes_file = Path.join(base: read nested, child: read "bytes.bin")
    File.write_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "abc"))?
    File.append_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "de"))?
    File.append_string(path: read bytes_file, text: read "f")?
    let bytes = File.read_bytes(path: read bytes_file)?
    Output.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    match File.bytes_stream(path: read bytes_file, chunk_size: 2) {
        Ok(stream) => {
            match Stream.collect_list<Bytes>(stream: read stream) {
                Ok(chunks) => {
                    Output.write(message: read String.from_int(value: List.len<Bytes>(list: read chunks)))
                    Output.write(message: read String.from_int(value: Bytes.len(value: read chunks[0])))
                    Output.write(message: read String.from_int(value: Bytes.len(value: read chunks[1])))
                    Output.write(message: read String.from_int(value: Bytes.len(value: read chunks[2])))
                }
                Err(error) => {
                    Output.write(message: read ChannelError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }

    let files = Directory.list_files(path: read root)?
    if List.contains_value<String>(list: read files, value: read "nested/path.txt") {
        Output.write(message: read "listed-path")
    }
    if List.contains_value<String>(list: read files, value: read "nested/bytes.bin") {
        Output.write(message: read "listed-bytes")
    }
    let paths = Directory.list_paths(path: read nested)?
    Output.write(message: read String.from_int(value: List.len<Path>(list: read paths)))

    let copied = Path.join(base: read nested, child: read "copied.txt")
    Directory.copy_file(from: read path_file, to: read copied)?
    let copied_text = File.read_string(path: read copied)?
    Output.write(message: read copied_text)
    let renamed = Path.join(base: read nested, child: read "renamed.txt")
    Directory.rename(from: read copied, to: read renamed)?
    if File.exists(path: read renamed) {
        Output.write(message: read "renamed-exists")
    }
    Directory.remove_file(path: read renamed)?
    if !File.exists(path: read renamed) {
        Output.write(message: read "renamed-removed")
    }
    Directory.remove_dir_all(path: read single)?
    if !Path.exists(path: read single) {
        Output.write(message: read "single-removed")
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend(
        "reg-vm-directory-file-stream.rss",
        source,
        [root_arg.as_str()],
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reg_vm_runs_env_path_and_file_extra_intrinsics_like_interpreter() {
    let root = std::env::current_dir()
        .expect("cwd should be available")
        .join("target")
        .join(format!(
            "rss-vm-env-path-file-{}-fixture",
            std::process::id()
        ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("env/path/file fixture root should be created");
    let root_arg = root.display().to_string();

    let source = r#"

async fn main() -> Result<Unit, FileError> {
    let current = Env.current_dir()?
    Env.set_current_dir(path: read current)?
    Env.set(name: read "RSSCRIPT_VM_IGNORED", value: read "ignored")
    if String.len(value: read Path.to_string(path: read Env.run_workspace_root())) > 0 {
        Output.write(message: read "workspace-root")
    }
    if String.len(value: read Path.to_string(path: read Env.temp_dir())) > 0 {
        Output.write(message: read "temp-dir")
    }
    match Env.home_dir() {
        Some(path) => {
            if String.len(value: read Path.to_string(path: read path)) > 0 {
                Output.write(message: read "home-dir")
            }
        }
        None => {
            Output.write(message: read "home-none")
        }
    }

    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rss-vm-env-path-file"))
    let nested = Path.join(base: read root, child: read "nested")
    Directory.create_all(path: read nested)?

    let path_file = Path.join(base: read nested, child: read "path.txt")
    Path.write_string(path: read path_file, text: read "path text")?
    let path_text = Path.read_string(path: read path_file)?
    Output.write(message: read path_text)

    let async_bytes = Path.join(base: read nested, child: read "async-bytes.bin")
    await File.write_async(path: read async_bytes, data: read Bytes.from_string(value: read "abc"))?
    let async_read = await File.read_all_async(path: read async_bytes)?
    Output.write(message: read String.from_int(value: Bytes.len(value: read async_read)))

    let async_text = Path.join(base: read nested, child: read "async-text.txt")
    await File.write_string_async(path: read async_text, text: read "async text")?
    let async_text_read = await File.read_all_string_async(path: read async_text)?
    Output.write(message: read async_text_read)

    let atomic = Path.join(base: read nested, child: read "atomic.txt")
    File.write_atomic(path: read atomic, text: read "atomic text")?
    let atomic_text = File.read_string(path: read atomic)?
    Output.write(message: read atomic_text)

    let handle_file = Path.join(base: read nested, child: read "handle-extra.txt")
    with File.open_write(path: read handle_file)? as writer {
        File.write_bytes_view(file: mut writer, data: read Bytes.view(value: read Bytes.from_string(value: read "view"), start: 1, len: 2))?
        local empty_buffer = Buffer.new(size: 0)
        File.write_buffer(file: mut writer, buffer: read empty_buffer)?
        let empty_view = Buffer.view(buffer: read empty_buffer, start: 0, len: 0)
        File.write_buffer_view(file: mut writer, buffer: read empty_view)?
    }
    let handle_read_path = Path.join(base: read nested, child: read "handle-extra.txt")
    let handle_read = File.read_string(path: read handle_read_path)?
    Output.write(message: read handle_read)

    let files = Path.list_files(path: read root)?
    if List.contains_value<String>(list: read files, value: read "nested/path.txt") {
        Output.write(message: read "path-list-file")
    }
    let paths = Path.list_paths(path: read nested)?
    Output.write(message: read String.from_int(value: List.len<Path>(list: read paths)))
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend(
        "reg-vm-env-path-file-extra.rss",
        source,
        [root_arg.as_str()],
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reg_vm_runs_time_and_fallible_pipeline_intrinsics_like_interpreter() {
    let source = r#"

fn main() -> Result<Unit, String> {
    let start = Clock.now()
    let unix_ms = Clock.system_unix_ms()
    if unix_ms > 0 {
        Output.write(message: read "clock")
    }
    let elapsed = Instant.elapsed(start: read start)
    if Duration.as_ms(value: read elapsed) >= 0 {
        Output.write(message: read "elapsed")
    }

    let immediate = Deadline.after_ms(ms: 0)
    if Deadline.is_expired(deadline: read immediate) {
        Output.write(message: read "expired-now")
    }
    if Deadline.remaining_ms(deadline: read immediate) >= 0 {
        Output.write(message: read "remaining-nonnegative")
    }
    let negative = Deadline.after(duration: read Duration.ms(value: 0 - 1))
    if Deadline.is_expired(deadline: read negative) {
        Output.write(message: read "expired-negative")
    }

    let numbers = [1, 2, 3]
    let touched_numbers = Pipeline.each<Int>(pipeline: read List.pipeline<Int>(list: read numbers), action: |item| {
        Output.write(message: read String.from_int(value: item))
        return Unit
    })
    let ok_pipeline = Pipeline.try_map<Int, Int, String>(pipeline: read touched_numbers, mapper: |item| {
        if item < 0 {
            return Err(String.copy(value: read "negative"))
        }
        return Ok(item + 1)
    })
    let mapped = FalliblePipeline.map<Int, Int, String>(pipeline: read ok_pipeline, mapper: |item| {
        return item + 10
    })
    let filtered = FalliblePipeline.filter<Int, String>(pipeline: read mapped, predicate: |item| {
        return item > 11
    })
    let touched = FalliblePipeline.each<Int, String>(pipeline: read filtered, action: |item| {
        Output.write(message: read String.from_int(value: item))
        return Unit
    })
    let final_pipeline = FalliblePipeline.try_map<Int, Int, String>(pipeline: read touched, mapper: |item| {
        if item < 0 {
            return Err(String.copy(value: read "negative"))
        }
        return Ok(item + 1)
    })
    match FalliblePipeline.collect<Int, String>(pipeline: read final_pipeline) {
        Ok(items) => {
            Output.write(message: read String.from_int(value: List.len<Int>(list: read items)))
            Output.write(message: read String.from_int(value: items[0]))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    let failed = Pipeline.try_map<Int, Int, String>(pipeline: read List.pipeline<Int>(list: read numbers), mapper: |item| {
        if item == 2 {
            return Err(String.copy(value: read "stop"))
        }
        return Ok(item + 0)
    })
    let still_failed = FalliblePipeline.map<Int, Int, String>(pipeline: read failed, mapper: |item| {
        Output.write(message: read "should-not-run")
        return item + 1
    })
    match FalliblePipeline.collect<Int, String>(pipeline: read still_failed) {
        Ok(items) => {
            Output.write(message: read String.from_int(value: List.len<Int>(list: read items)))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-time-fallible-pipeline.rss", source, []);
}

#[test]
fn reg_vm_runs_receiver_methods_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let greeting = String.concat(left: read "hi ", right: read "there")
    Output.write(message: read String.from_int(value: greeting.len()))
    let n = 255
    Output.write(message: read n.to_string())
    let blank = ""
    if blank.is_empty() {
        Output.write(message: read "blank-empty")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-gap-receiver-methods.rss", source, []);
}
