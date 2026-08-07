//! register-VM execution: arithmetic and uncategorized
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn reg_vm_runs_format_date_and_int_bit_helpers_like_backend() {
    let source = r#"

fn main(args: read List<String>) -> Unit {
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

fn main(args: read List<String>) -> Unit {
    let args = Arguments.all(args: read args)
    Assert.equal_int(left: Arguments.count(args: read args), right: 2)
    Assert.equal_int(left: List.len<String>(list: read args), right: 2)
    Assert.equal(left: read Int.to_string(value: read 42), right: read "42")
    Assert.equal_bool(left: true, right: true)
    match Arguments.get(args: read args, index: 0) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "missing-first")
        }
    }
    match Arguments.get(args: read args, index: 99) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "missing-none")
        }
    }
    match Arguments.get(args: read args, index: 0 - 1) {
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

fn main(args: read List<String>) -> Unit {
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

fn main(args: read List<String>) -> Unit {
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

fn main(args: read List<String>) -> Unit {
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

fn main(args: read List<String>) -> Unit {
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

fn main(args: read List<String>) -> Unit {
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
fn reg_vm_runs_object_literal_like_interpreter() {
    let source = r#"
fn main(args: read List<String>) -> JsonValue {
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

fn main(args: read List<String>) -> Result<Unit, String> {
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

fn main(args: read List<String>) -> Unit {
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

fn main(args: read List<String>) -> Unit {
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
fn main(args: read List<String>) -> Unit {
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
fn reg_vm_runs_cancellation_and_stream_intrinsics_like_interpreter() {
    let source = r#"

async fn main(args: read List<String>) -> Result<Unit, ChannelError> {
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
fn reg_vm_runs_receiver_methods_like_interpreter() {
    let source = r#"
fn main(args: read List<String>) -> Unit {
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
