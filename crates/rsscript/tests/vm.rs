use std::fs;
use std::path::Path;
use std::process::Command;

use sha1::{Digest, Sha1};

use rsscript::{
    NativeInterpreterFn, NativeValue, lower_source_to_rust_package, reg_vm_compile_source,
    reg_vm_eval_source_main_with_args, reg_vm_eval_source_main_with_args_and_native_bindings,
    write_generated_rust_package,
};

mod common;

#[test]
fn reg_vm_runs_select_first_ready_like_backend() {
    // `select` runs both arms concurrently; the shorter sleep (1ms) wins over the
    // longer (50ms), so the scheduler's clock — not arm order — must decide.
    let source = r#"
features: async, native

async fn after(value: Int, ms: Int) -> Result<Int, TimerError> {
    await Timer.sleep(ms: ms)?
    return Ok(value)
}

fn main() -> Result<Unit, TimerError> {
    select {
        value = await after(value: 7, ms: 1)? => {
            Log.write(message: read String.from_int(value: value))
        }
        other = await after(value: 9, ms: 50)? => {
            Log.write(message: read String.from_int(value: other))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-select.rss", source, []);
}

#[test]
fn reg_vm_runs_spawned_producer_consumer_channel_like_backend() {
    // A spawned producer fills a capacity-1 channel while the consumer drains it:
    // exercises both recv-on-empty and send-on-full parking plus cross-task wakeups
    // (the producer's second send blocks until the consumer's first recv frees a slot).
    let source = r#"
features: native, local, async

async fn produce(sender: read Sender<Int>) -> Result<Unit, ChannelError> {
    local a = 10
    await Sender.send<Int>(sender: read sender, value: take a)?
    local b = 20
    await Sender.send<Int>(sender: read sender, value: take b)?
    return Ok(Unit)
}

async fn drain(receiver: read Receiver<Int>) -> Result<Int, ChannelError> {
    let first = await Receiver.recv<Int>(receiver: read receiver)?
    let second = await Receiver.recv<Int>(receiver: read receiver)?
    let a = match first {
        Some(value) => value
        None => 0
    }
    let b = match second {
        Some(value) => value
        None => 0
    }
    return Ok(a + b)
}

async fn main() -> Result<Unit, ChannelError> {
    match Channel.bounded<Int>(capacity: 1) {
        Ok(channel) => {
            let sender = Channel.sender<Int>(channel: read channel)
            local channel_value = channel
            match Channel.receiver<Int>(channel: mut channel_value) {
                Ok(receiver) => {
                    task_group {
                        async let producer = produce(sender: read sender)
                        async let consumer = drain(receiver: read receiver)
                        await producer?
                        let total = await consumer?
                        Log.write(message: read String.from_int(value: total))
                    }
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

    assert_reg_vm_matches_compiled_backend("reg-vm-producer-consumer.rss", source, []);
}

#[test]
fn reg_vm_runs_task_group_async_let_like_backend() {
    // `async let` spawns concurrent tasks; `await` joins them. The scheduler must
    // run the spawned tasks and hand their results back to the joining task.
    let source = r#"
features: async

async fn fetch_user() -> Result<String, String> {
    return Ok("user")
}

async fn fetch_profile() -> Result<String, String> {
    return Ok("profile")
}

fn main() -> Result<Unit, String> {
    task_group {
        async let user = fetch_user()
        async let profile = fetch_profile()

        let u = await user?
        let p = await profile?
        Log.write(message: read u)
        Log.write(message: read p)
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-task-group.rss", source, []);
}

#[test]
fn reg_vm_propagates_all_list_mutators_through_mut_param_like_backend() {
    // A `mut List` parameter aliases the caller's list (lowered to `&mut` by the
    // backend), so every mutator — not just `push` — must propagate. This caught
    // the divergence where `set`/`append`/`sort` cloned-and-wrote-back and so
    // silently dropped mutations made through a `mut` param.
    let source = r#"
fn mutate(xs: mut List<Int>) -> Unit {
    List.push<Int>(list: mut xs, value: read 4)
    List.set<Int>(list: mut xs, index: 0, value: read 10)
    List.append<Int>(list: mut xs, values: read List<Int>.new())
    List.sort<Int>(list: mut xs)
    return Unit
}

fn main() -> Unit {
    let mut a = List<Int>.new()
    List.push<Int>(list: mut a, value: read 3)
    List.push<Int>(list: mut a, value: read 1)
    mutate(xs: mut a)
    Log.write(message: read String.from_int(value: List.get<Int>(list: read a, index: 0)))
    Log.write(message: read String.from_int(value: List.get<Int>(list: read a, index: 2)))
    Log.write(message: read String.from_int(value: List.len<Int>(list: read a)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-mut-list-param.rss", source, []);
}

#[test]
fn reg_vm_isolates_by_value_list_params_like_backend() {
    // A non-`mut` parameter is by value/`&` in the backend (with an inserted
    // `.clone()`), so the callee owns an isolated copy. Stashing it in a struct,
    // handing it back, and mutating it must not write into the caller's original
    // list — which only holds once non-`mut` params are deep-copied on entry.
    let source = r#"
struct Box {
    items: List<Int>
}

fn keep(xs: read List<Int>) -> Box {
    return Box(items: read xs)
}

fn main() -> Unit {
    let mut a = List<Int>.new()
    List.push<Int>(list: mut a, value: read 1)
    let boxed = keep(xs: read a)
    let mut b = boxed.items
    List.push<Int>(list: mut b, value: read 9)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read a)))
    Log.write(message: read String.from_int(value: List.len<Int>(list: read b)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-by-value-list-param.rss", source, []);
}

#[test]
fn vm_runs_pure_loop_sum_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let mut index = 0
    let mut total = 0
    while index < 10 {
        total = total + index
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-loop.rss", source, []);
}

#[test]
fn vm_runs_user_function_hot_loop_like_interpreter() {
    let source = r#"
fn mix(value: Int, salt: Int) -> Int {
    let doubled = value * 2
    return doubled + salt
}

fn main() -> Unit {
    let mut index = 0
    let mut total = 0
    while index < 10 {
        total = total + mix(value: index, salt: 1)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-function-loop.rss", source, []);
}

#[test]
fn reg_vm_runs_pure_loop_sum_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let mut index = 0
    let mut total = 0
    while index < 10 {
        total = total + index
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-loop.rss", source, []);
}

#[test]
fn reg_vm_runs_user_function_hot_loop_like_interpreter() {
    let source = r#"
fn mix(value: Int, salt: Int) -> Int {
    let doubled = value * 2
    return doubled + salt
}

fn main() -> Unit {
    let mut index = 0
    let mut total = 0
    while index < 10 {
        total = total + mix(value: index, salt: 1)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-function-loop.rss", source, []);
}

#[test]
fn reg_vm_runs_if_and_args_parse_like_interpreter() {
    let source = r#"
fn bench_size(default: Int) -> Int {
    let raw = Args.get_or_default(index: 0, default: read String.from_int(value: default))
    if raw == "11" {
        return 11
    }
    return default
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: bench_size(default: 7)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-if-args.rss", source, ["11"]);
}

#[test]
fn reg_vm_runs_math_random_uuid_and_modulo_like_interpreter() {
    let source = r#"
features: native

fn float_mix(value: Float, salt: Float) -> Float {
    return value * 2.0 + salt
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: 17 % 5))
    Log.write(message: read String.from_int(value: Math.abs(value: -9)))
    Log.write(message: read String.from_int(value: Math.min(left: 4, right: 7)))
    Log.write(message: read String.from_int(value: Math.max(left: 4, right: 7)))
    Log.write(message: read String.from_int(value: Math.clamp(value: 12, min: 0, max: 10)))
    Log.write(message: read String.from_float(value: Math.abs_float(value: -2.5)))
    Log.write(message: read String.from_float(value: Math.min_float(left: 4.5, right: 7.25)))
    Log.write(message: read String.from_float(value: Math.max_float(left: 4.5, right: 7.25)))
    Log.write(message: read String.from_float(value: Math.clamp_float(value: 12.5, min: 0.5, max: 10.5)))
    Log.write(message: read String.from_float(value: Math.pow_float(base: 2.0, exponent: 3.0)))
    Log.write(message: read String.from_float(value: Math.sqrt(value: 9.0)))
    Log.write(message: read Float.to_string(value: read Math.sqrt(value: 16.0)))
    Log.write(message: read String.from_float(value: Math.cos(value: 0.0)))
    Log.write(message: read String.from_float(value: Math.exp(value: 0.0)))
    Log.write(message: read String.from_float(value: Math.log(value: 1.0)))
    Log.write(message: read String.from_float(value: Math.tanh(value: 0.0)))
    let finite = 1.5
    let infinite = 1.0 / 0.0
    let nan = 0.0 / 0.0
    Log.write(message: read String.from_bool(value: Float.is_finite(value: read finite)))
    Log.write(message: read String.from_bool(value: Float.is_infinite(value: read infinite)))
    Log.write(message: read String.from_bool(value: Float.is_nan(value: read nan)))
    match String.parse_float(value: read "12.5") {
        Some(value) => Log.write(message: read String.from_float(value: value))
        None => Log.write(message: read "float-none")
    }
    match String.parse_float(value: read "not-float") {
        Some(value) => Log.write(message: read String.from_float(value: value))
        None => Log.write(message: read "invalid-float")
    }
    Log.write(message: read String.from_int(value: Math.floor(value: 3.9)))
    Log.write(message: read String.from_int(value: Math.ceil(value: 3.1)))
    Log.write(message: read String.from_int(value: Math.round(value: 3.5)))
    Log.write(message: read String.from_float(value: 1.5 + 2.25))
    Log.write(message: read String.from_float(value: 9.0 - 2.5))
    Log.write(message: read String.from_float(value: 3.0 * 2.5))
    Log.write(message: read String.from_float(value: 7.5 / 2.5))
    Log.write(message: read String.from_float(value: float_mix(value: 1.5, salt: 0.5)))
    if 1.0 < 2.0 {
        Log.write(message: read "float-condition")
    }
    if 5.5 > 5.0 && 5.0 <= 5.0 {
        Log.write(message: read "float-compare")
    }

    let fixed = Random.int(min: 7, max: 7)
    Log.write(message: read String.from_int(value: fixed))
    Log.write(message: read String.from_int(value: Math.floor(value: Random.float())))
    let bytes = Random.bytes(len: 4)
    Log.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    let token = Random.string(len: 8)
    Log.write(message: read String.from_int(value: String.len(value: read token)))
    let maybe = Random.bool()
    if maybe {
        Log.write(message: read "bool")
    } else {
        Log.write(message: read "bool")
    }
    let id = Uuid.new_v4()
    Log.write(message: read String.from_int(value: String.len(value: read id)))
    if String.contains(value: read id, needle: read "-") {
        Log.write(message: read "uuid")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-math-random-uuid.rss", source, []);
}

#[test]
fn compiled_backend_accepts_inline_read_copy_expression_args() {
    let source = r#"
features: local

fn main() -> Unit {
    local xs = List.new<Int>()
    List.push<Int>(list: mut xs, value: read (0 - 1))
    Log.write(message: read String.from_int(value: List.get<Int>(list: read xs, index: 0)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-inline-read-copy-expr.rss", source, []);
}

#[test]
fn reg_vm_runs_common_math_string_char_and_list_helpers_like_backend() {
    let source = r#"
features: local

fn main() -> Unit {
    Log.write(message: read String.from_int(value: Math.pow(base: 3, exponent: 4)))

    Log.write(message: read String.trim_start(value: read "  left"))
    Log.write(message: read String.trim_end(value: read "right  "))
    Log.write(message: read String.pad_left(value: read "7", width: 3, fill: read "0"))
    Log.write(message: read String.pad_right(value: read "x", width: 3, fill: read "."))
    Log.write(message: read String.reverse(value: read "abc"))
    Log.write(message: read String.replace_first(value: read "one one", from: read "one", to: read "two"))
    Log.write(message: read String.from_int(value: String.count(value: read "banana", needle: read "an")))
    match String.char_at(value: read "abc", index: 1) {
        Some(value) => Log.write(message: read Char.to_string(value: read value))
        None => Log.write(message: read "missing-char")
    }
    match String.char_at(value: read "abc", index: 9) {
        Some(value) => Log.write(message: read Char.to_string(value: read value))
        None => Log.write(message: read "missing-char")
    }

    match String.char_at(value: read "a", index: 0) {
        Some(value) => {
            if Char.is_lower(value: read value) {
                Log.write(message: read "lower")
            }
        }
        None => Log.write(message: read "missing-lower")
    }
    match String.char_at(value: read "Z", index: 0) {
        Some(value) => {
            if Char.is_upper(value: read value) {
                Log.write(message: read "upper")
            }
        }
        None => Log.write(message: read "missing-upper")
    }
    match String.char_at(value: read "Q", index: 0) {
        Some(value) => Log.write(message: read Char.to_string(value: read Char.to_lower(value: read value)))
        None => Log.write(message: read "missing-to-lower")
    }
    match String.char_at(value: read "q", index: 0) {
        Some(value) => Log.write(message: read Char.to_string(value: read Char.to_upper(value: read value)))
        None => Log.write(message: read "missing-to-upper")
    }

    let values = [3, 1, 3, 2]
    Log.write(message: read String.from_int(value: List.sum(list: read values)))
    match List.min(list: read values) {
        Some(value) => Log.write(message: read String.from_int(value: value))
        None => Log.write(message: read "min-none")
    }
    match List.max(list: read values) {
        Some(value) => Log.write(message: read String.from_int(value: value))
        None => Log.write(message: read "max-none")
    }
    let deduped = List.dedup<Int>(list: read values)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read deduped)))
    Log.write(message: read String.from_int(value: deduped[0]))
    Log.write(message: read String.from_int(value: deduped[1]))
    Log.write(message: read String.from_int(value: deduped[2]))

    let nested = [[1, 2], [3], List<Int>.new()]
    let flat = List.flatten<Int>(list: read nested)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read flat)))
    Log.write(message: read String.from_int(value: flat[0]))
    Log.write(message: read String.from_int(value: flat[2]))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-common-helper-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_format_date_and_int_bit_helpers_like_backend() {
    let source = r#"
features: native

fn main() -> Unit {
    Log.write(message: read String.format(template: read "hello {}, {{}} {}", args: read ["rss", "vm"]))
    Log.write(message: read String.format(template: read "missing {}", args: read List<String>.new()))

    Log.write(message: read String.from_int(value: Int.bit_and(left: 6, right: 3)))
    Log.write(message: read String.from_int(value: Int.bit_or(left: 4, right: 1)))
    Log.write(message: read String.from_int(value: Int.bit_xor(left: 6, right: 3)))
    Log.write(message: read String.from_int(value: Int.bit_not(value: 0)))
    Log.write(message: read String.from_int(value: Int.shift_left(value: 3, bits: 2)))
    Log.write(message: read String.from_int(value: Int.shift_right(value: 16, bits: 2)))
    Log.write(message: read String.from_int(value: 6 & 3))
    Log.write(message: read String.from_int(value: 4 | 1))
    Log.write(message: read String.from_int(value: 6 ^ 3))
    Log.write(message: read String.from_int(value: 3 << 2))
    Log.write(message: read String.from_int(value: 16 >> 2))
    Log.write(message: read String.from_int(value: 1 | 2 & 3))
    Log.write(message: read String.from_bool(value: !false))
    Log.write(message: read String.from_bool(value: !(1 > 2)))
    Log.write(message: read String.from_int(value: ~0))
    Log.write(message: read String.from_int(value: ~5))

    match Date.parse_ymd(value: read "2024-02-29") {
        Some(value) => {
            Log.write(message: read Date.format_ymd(unix_ms: value))
            Log.write(message: read Date.format_iso(unix_ms: value))
            Log.write(message: read String.from_int(value: Date.year(unix_ms: value)))
            Log.write(message: read String.from_int(value: Date.month(unix_ms: value)))
            Log.write(message: read String.from_int(value: Date.day(unix_ms: value)))
            Log.write(message: read String.from_int(value: Date.hour(unix_ms: value)))
            Log.write(message: read String.from_int(value: Date.minute(unix_ms: value)))
            Log.write(message: read String.from_int(value: Date.second(unix_ms: value)))
            Log.write(message: read Date.format_ymd(unix_ms: Date.add_days(unix_ms: value, days: 1)))
            Log.write(message: read String.from_int(value: Date.days_between(start_unix_ms: value, end_unix_ms: Date.add_days(unix_ms: value, days: 3))))
            Log.write(message: read String.from_int(value: Date.add_ms(unix_ms: value, ms: 250) - value))
            Log.write(message: read Date.format_iso(unix_ms: Date.start_of_day(unix_ms: Date.add_ms(unix_ms: value, ms: 45678))))
            Log.write(message: read String.from_int(value: Date.weekday(unix_ms: value)))
            Log.write(message: read String.from_bool(value: Date.is_leap_year(year: 2024)))
            Log.write(message: read String.from_int(value: Date.days_in_month(year: 2024, month: 2)))
            Log.write(message: read String.from_bool(value: Date.is_leap_year(year: 2023)))
            Log.write(message: read String.from_int(value: Date.days_in_month(year: 2023, month: 13)))
        }
        None => Log.write(message: read "date-none")
    }
    match Date.parse_iso(value: read "2024-02-29T12:34:56.789+02:00") {
        Some(value) => {
            Log.write(message: read Date.format_iso(unix_ms: value))
            Log.write(message: read String.from_int(value: Date.hour(unix_ms: value)))
            Log.write(message: read String.from_int(value: Date.minute(unix_ms: value)))
            Log.write(message: read String.from_int(value: Date.second(unix_ms: value)))
        }
        None => Log.write(message: read "iso-none")
    }
    match Date.parse_iso(value: read "not-an-iso-date") {
        Some(value) => Log.write(message: read Date.format_iso(unix_ms: value))
        None => Log.write(message: read "invalid-iso-date")
    }
    match Date.parse_ymd(value: read "not-a-date") {
        Some(value) => Log.write(message: read Date.format_ymd(unix_ms: value))
        None => Log.write(message: read "invalid-date")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-format-date-bit-intrinsics.rss", source, []);
}

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
fn reg_vm_matches_compiled_backend_for_recent_alignment_features() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

fn main() -> Unit {
    let mut values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.push<Int>(list: mut values, value: read 3)
    values[2] = 30
    Log.write(message: read String.from_int(value: values[2]))

    let greeting = String.concat(left: read "hi ", right: read "there")
    Log.write(message: read String.from_int(value: greeting.len()))
    let n = 255
    Log.write(message: read n.to_string())
    let blank = ""
    if blank.is_empty() {
        Log.write(message: read "blank-empty")
    }

    let point = Point(x: 3, y: 4)
    match read point {
        Point { x, y } => {
            Log.write(message: read String.from_int(value: x + y))
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-compiled-recent-alignment.rss", source, []);
}

#[test]
fn reg_vm_runs_basic_args_assert_and_int_intrinsics_like_interpreter() {
    let source = r#"
features: native

fn main() -> Unit {
    let args = Args.all()
    Assert.equal_int(left: Args.count(), right: 2)
    Assert.equal_int(left: List.len<String>(list: read args), right: 2)
    Assert.equal(left: read Int.to_string(value: read 42), right: read "42")
    Assert.equal_bool(left: true, right: true)
    match Args.get(index: 0) {
        Some(value) => {
            Log.write(message: read value)
        }
        None => {
            Log.write(message: read "missing-first")
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
fn reg_vm_runs_duration_and_url_intrinsics_like_interpreter() {
    let source = r#"
features: native

fn main() -> Unit {
    let short = Duration.ms(value: 750)
    let long = Duration.seconds(value: 2)
    let total = Duration.add(left: read short, right: read long)
    Log.write(message: read String.from_int(value: Duration.as_ms(value: read short)))
    Log.write(message: read String.from_int(value: Duration.as_ms(value: read long)))
    Log.write(message: read String.from_int(value: Duration.as_ms(value: read total)))
    Log.write(message: read String.from_int(value: Duration.as_seconds(value: read total)))

    let url = Url.from_string(value: read "https://example.test/path")
    Log.write(message: read Url.to_string(url: read url))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-duration-url-intrinsics.rss", source, []);
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
fn reg_vm_runs_counter_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    local counter = Counter.new(value: 4)
    Counter.add(counter: mut counter, amount: 6)
    Log.write(message: read String.from_int(value: Counter.value(counter: read counter)))
    Counter.add(counter: mut counter, amount: 0 - 3)
    Log.write(message: read String.from_int(value: Counter.value(counter: read counter)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-counter-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_config_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let empty_rules = List<Rule>.new()
    let empty = Config.new(name: read "empty", rules: read empty_rules)
    Log.write(message: read String.from_int(value: Config.rule_count(config: read empty)))

    let rules = List<Rule>.new()
    let config = Config.new(name: read "rules", rules: read rules)
    Log.write(message: read String.from_int(value: Config.rule_count(config: read config)))

    local global = GlobalConfig.new(value: read config)
    Log.write(message: read String.from_int(value: GlobalConfig.rule_count(global: read global)))
    GlobalConfig.replace(global: mut global, value: read empty)
    Log.write(message: read String.from_int(value: GlobalConfig.rule_count(global: read global)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-config-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_cache_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    local cache = Cache.new()
    Log.write(message: read Cache.lookup(cache: read cache, key: read "missing"))
    Cache.insert(cache: mut cache, key: read "one", value: read "first")
    Cache.insert(cache: mut cache, key: read "one", value: read "second")
    Cache.insert(cache: mut cache, key: read "two", value: read "other")
    Log.write(message: read Cache.lookup(cache: read cache, key: read "one"))
    Log.write(message: read Cache.lookup(cache: read cache, key: read "two"))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-cache-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_cache_get_like_interpreter() {
    let source = r#"
features: local

fn main() -> Image {
    local cache = Cache.new()
    Cache.insert(cache: mut cache, key: read "image", value: read "cached-image")
    return Cache.get(cache: read cache)
}
"#;

    assert_reg_vm_matches_compiled_backend_return(
        "reg-vm-cache-get.rss",
        source,
        [],
        CompiledReturnHarness::Image,
    );
}

#[test]
fn reg_vm_runs_image_intrinsics_like_interpreter() {
    let root = std::env::current_dir()
        .expect("cwd should be available")
        .join("target")
        .join(format!("rss-vm-image-{}-fixture", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("image fixture dir should be created");
    let input = root.join("input.img");
    let output = root.join("output.img");
    fs::write(&input, b"pixels").expect("image fixture should write");
    let input_arg = input.display().to_string();
    let output_arg = output.display().to_string();

    let source = r#"
features: native, local, async

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
    local saved = Image.load(path: read output)?
    Image.inspect(image: read saved)
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend(
        "reg-vm-image-intrinsics.rss",
        source,
        [input_arg.as_str(), output_arg.as_str()],
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reg_vm_runs_persistent_map_intrinsics_like_interpreter() {
    let source = r#"
features: local

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

    assert_reg_vm_matches_compiled_backend("reg-vm-persistent-map-intrinsics.rss", source, []);
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
fn reg_vm_runs_hash_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let digest = Hash.sha256_string(value: read "abc")
    Log.write(message: read digest)
    let bytes = Bytes.from_string(value: read "abc")
    Log.write(message: read Hash.sha256_bytes(value: read bytes))
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
    Log.write(message: read String.from_int(value: String.len(value: read hmac)))
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
fn reg_vm_runs_deque_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    local deque = Deque<Int>.new()
    if Deque.is_empty<Int>(deque: read deque) {
        Log.write(message: read "empty")
    }
    Deque.push_back<Int>(deque: mut deque, value: read 2)
    Deque.push_front<Int>(deque: mut deque, value: read 1)
    Deque.push_back<Int>(deque: mut deque, value: read 3)
    Log.write(message: read String.from_int(value: Deque.len<Int>(deque: read deque)))
    let values = Deque.to_list<Int>(deque: read deque)
    Log.write(message: read String.from_int(value: values[0]))
    Log.write(message: read String.from_int(value: values[1]))
    Log.write(message: read String.from_int(value: values[2]))
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "front-none")
        }
    }
    match Deque.pop_back<Int>(deque: mut deque) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "back-none")
        }
    }
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "front-none")
        }
    }
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "front-none")
        }
    }
    Deque.push_back<Int>(deque: mut deque, value: read 4)
    Deque.clear<Int>(deque: mut deque)
    if Deque.is_empty<Int>(deque: read deque) {
        Log.write(message: read "cleared")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-deque-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_set_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    local set = Set<String>.new()
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
    if Set.contains<String>(set: read set, value: read "b") {
        Log.write(message: read "for-each-b")
    }

    local right = Set<String>.new()
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
    let list = Set.to_list<String>(set: read union)
    Log.write(message: read String.from_int(value: List.len<String>(list: read list)))
    if List.contains_value<String>(list: read list, value: read "b") {
        Log.write(message: read "list-b")
    }
    if List.contains_value<String>(list: read list, value: read "c") {
        Log.write(message: read "list-c")
    }
    Set.clear<String>(set: mut set)
    Log.write(message: read String.from_int(value: Set.len<String>(set: read set)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-set-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_sorted_set_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    local set = SortedSet<Int>.new()
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

    assert_reg_vm_matches_compiled_backend("reg-vm-sorted-set-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_sorted_map_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    local map = SortedMap<Int, String>.new()
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

    assert_reg_vm_matches_compiled_backend("reg-vm-sorted-map-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_sorted_map_order_and_fresh_lists_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    local strings = SortedMap<String, Int>.new()
    SortedMap.insert<String, Int>(map: mut strings, key: read "b", value: read 2)
    let first_keys = SortedMap.keys<String, Int>(map: read strings)
    let first_values = SortedMap.values<String, Int>(map: read strings)
    SortedMap.insert<String, Int>(map: mut strings, key: read "a", value: read 1)
    let sorted_keys = SortedMap.keys<String, Int>(map: read strings)
    let sorted_values = SortedMap.values<String, Int>(map: read strings)
    Log.write(message: read first_keys[0])
    Log.write(message: read String.from_int(value: first_values[0]))
    Log.write(message: read String.from_int(value: List.len<String>(list: read first_keys)))
    Log.write(message: read sorted_keys[0])
    Log.write(message: read sorted_keys[1])
    Log.write(message: read String.from_int(value: sorted_values[0]))
    Log.write(message: read String.from_int(value: sorted_values[1]))

    local bools = SortedMap<Bool, String>.new()
    SortedMap.insert<Bool, String>(map: mut bools, key: read true, value: read "true")
    SortedMap.insert<Bool, String>(map: mut bools, key: read false, value: read "false")
    let bool_values = SortedMap.values<Bool, String>(map: read bools)
    Log.write(message: read bool_values[0])
    Log.write(message: read bool_values[1])
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-sorted-map-order-fresh.rss", source, []);
}

#[test]
fn reg_vm_runs_buffer_intrinsics_like_interpreter() {
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

    assert_reg_vm_matches_compiled_backend("reg-vm-buffer-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_encoding_intrinsics_like_interpreter() {
    let source = r#"
features: native

fn main() -> Unit {
    let encoded = Base64.encode(value: read "rsscript")
    Log.write(message: read encoded)

    match Base64.decode_string(text: read encoded) {
        Ok(value) => Log.write(message: read value)
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    let bytes = String.to_bytes(value: read "hex")
    Log.write(message: read Base64.encode_bytes(value: read bytes))

    match Base64.decode(text: read "%%%") {
        Ok(value) => Log.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    let hexed = Hex.encode_string(value: read "Az")
    Log.write(message: read hexed)
    Log.write(message: read Hex.encode(value: read bytes))

    match Hex.decode(text: read hexed) {
        Ok(value) => Log.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    match Hex.decode(text: read "not-hex") {
        Ok(value) => Log.write(message: read String.from_int(value: Bytes.len(value: read value)))
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    let component = Url.encode_component(value: read "a b/é?x=1")
    Log.write(message: read component)

    match Url.decode_component(value: read component) {
        Ok(value) => Log.write(message: read value)
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    match Url.decode_component(value: read "%FF") {
        Ok(value) => Log.write(message: read value)
        Err(error) => Log.write(message: read DecodeError.message(error: read error))
    }

    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-encoding-intrinsics.rss", source, []);
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
fn reg_vm_runs_pure_path_intrinsics_like_interpreter() {
    let source = r#"
features: native

fn main() -> Unit {
    let root = Path.from_string(value: read "fixtures")
    let path = Path.join(base: read root, child: read "rsscript-path.txt")

    Log.write(message: read Path.to_string(path: read path))
    Log.write(message: read Path.to_string(path: read String.to_path(value: read "fixtures/rsscript-path.txt")))
    Log.write(message: read Path.to_string(path: read Path.normalize(path: read Path.join(base: read path, child: read ".."))))

    match Path.file_name(path: read path) {
        Some(name) => {
            Log.write(message: read name)
        }
        None => {
            Log.write(message: read "no-name")
        }
    }
    match Path.extension(path: read path) {
        Some(extension) => {
            Log.write(message: read extension)
        }
        None => {
            Log.write(message: read "no-extension")
        }
    }
    match Path.parent(path: read path) {
        Some(parent) => {
            Log.write(message: read Path.to_string(path: read parent))
        }
        None => {
            Log.write(message: read "no-parent")
        }
    }

    if Path.is_absolute(path: read Path.from_string(value: read "/tmp/rsscript")) {
        Log.write(message: read "absolute")
    }
    if Path.starts_with(path: read path, base: read root) {
        Log.write(message: read "starts")
    }
    Log.write(message: read Path.to_string(path: read Path.with_extension(path: read path, extension: read "json")))

    match Path.safe_relative(value: read "fixtures/./rsscript-path.txt") {
        Ok(safe) => {
            Log.write(message: read Path.to_string(path: read safe))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match String.safe_relative(value: read "../escape") {
        Ok(safe) => {
            Log.write(message: read Path.to_string(path: read safe))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Path.resolve_relative(root: read root, relative: read "rsscript-path.txt") {
        Ok(resolved) => {
            Log.write(message: read Path.to_string(path: read resolved))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    let from_method: Url = String.to_url(value: read "https://example.test/from-method")
    Log.write(message: read Url.to_string(url: read from_method))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-pure-path-intrinsics.rss", source, []);
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
fn reg_vm_runs_map_read_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let empty: Map<String, Int> = Map.new<String, Int>()
    if Map.is_empty<String, Int>(map: read empty) {
        Log.write(message: read "empty")
    }

    let table: Map<String, Int> = {"a" => 1, "b" => 2}
    let keys = Map.keys<String, Int>(map: read table)
    Assert.equal_int(left: List.len<String>(list: read keys), right: 2)
    Assert.equal_bool(left: List.contains_value<String>(list: read keys, value: read "a"), right: true)
    Assert.equal_bool(left: List.contains_value<String>(list: read keys, value: read "b"), right: true)

    let values = Map.values<String, Int>(map: read table)
    Assert.equal_int(left: List.len<Int>(list: read values), right: 2)
    Assert.equal_bool(left: List.contains_value<Int>(list: read values, value: read 1), right: true)
    Assert.equal_bool(left: List.contains_value<Int>(list: read values, value: read 2), right: true)
    Log.write(message: read "ok")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-read-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_list_non_closure_read_intrinsics_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let numbers: List<Int> = [3, 1, 2, 5, 4]
    if List.contains_value<Int>(list: read numbers, value: read 5) {
        Log.write(message: read "contains")
    }
    match List.last<Int>(list: read numbers) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "last-none")
        }
    }
    let reversed = List.reverse<Int>(list: read numbers)
    Log.write(message: read String.from_int(value: reversed[0]))
    let skipped = List.skip<Int>(list: read numbers, count: 2)
    Log.write(message: read String.from_int(value: skipped[0]))
    let negative_skip = List.skip<Int>(list: read numbers, count: 0 - 2)
    Log.write(message: read String.from_int(value: negative_skip[0]))
    let taken = List.take<Int>(list: read numbers, count: 3)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read taken)))
    let negative_take = List.take<Int>(list: read numbers, count: 0 - 3)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read negative_take)))
    let sliced = List.slice<Int>(list: read numbers, start: 1, len: 3)
    Log.write(message: read String.from_int(value: sliced[0]))
    Log.write(message: read String.from_int(value: sliced[2]))
    let negative_slice = List.slice<Int>(list: read numbers, start: 0 - 2, len: 2)
    Log.write(message: read String.from_int(value: negative_slice[0]))
    let beyond = List.slice<Int>(list: read numbers, start: 99, len: 2)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read beyond)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-read-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_option_result_non_closure_intrinsics_like_interpreter() {
    let source = r#"
fn maybe(found: Bool) -> Option<Int> {
    if found {
        return Some(7)
    }
    return None
}

fn checked(ok: Bool) -> Result<Int, String> {
    if ok {
        return Ok(3)
    }
    return Err("bad")
}

fn main() -> Unit {
    if Option.is_some<Int>(value: read maybe(found: true)) {
        Log.write(message: read "some")
    }
    if Option.is_none<Int>(value: read maybe(found: false)) {
        Log.write(message: read "none")
    }
    Log.write(message: read String.from_int(value: Option.unwrap_or<Int>(value: read maybe(found: false), default: read 9)))
    match Option.ok_or<Int, String>(value: read maybe(found: true), error: read "missing") {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Option.ok_or<Int, String>(value: read maybe(found: false), error: read "missing") {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    Log.write(message: read String.from_int(value: Option.unwrap_or<Int>(value: read Option.or<Int>(value: read maybe(found: false), fallback: read Some(11)), default: read 0)))

    if Result.is_ok<Int, String>(value: read checked(ok: true)) {
        Log.write(message: read "ok")
    }
    if Result.is_err<Int, String>(value: read checked(ok: false)) {
        Log.write(message: read "err")
    }
    Log.write(message: read String.from_int(value: Result.unwrap_or<Int, String>(value: read checked(ok: false), default: read 12)))
    match Result.ok<Int, String>(value: read checked(ok: true)) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "ok-none")
        }
    }
    match Result.err<Int, String>(value: read checked(ok: false)) {
        Some(error) => {
            Log.write(message: read error)
        }
        None => {
            Log.write(message: read "err-none")
        }
    }
    match Result.err_message<Int, String>(value: read checked(ok: false)) {
        Some(message) => {
            Log.write(message: read message)
        }
        None => {
            Log.write(message: read "message-none")
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-option-result-non-closure.rss", source, []);
}

#[test]
fn reg_vm_runs_option_result_closure_intrinsics_like_interpreter() {
    let source = r#"
fn maybe(found: Bool) -> Option<Int> {
    if found {
        return Some(7)
    }
    return None
}

fn checked(ok: Bool) -> Result<Int, String> {
    if ok {
        return Ok(3)
    }
    return Err("bad")
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: Option.unwrap_or_else<Int>(value: read maybe(found: true), default: || {
        return 14
    })))
    Log.write(message: read String.from_int(value: Option.unwrap_or_else<Int>(value: read maybe(found: false), default: || {
        return 15
    })))

    match Option.map<Int, Int>(value: read maybe(found: true), mapper: |item| {
        return item + 2
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "map-none")
        }
    }
    match Option.and_then<Int, Int>(value: read maybe(found: true), mapper: |item| {
        return Some(item + 5)
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "and-then-none")
        }
    }
    match Option.filter<Int>(value: read maybe(found: true), predicate: |item| {
        return item > 3
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "filter-none")
        }
    }
    match Option.filter<Int>(value: read maybe(found: true), predicate: |item| {
        return item > 10
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "filter-none")
        }
    }

    Log.write(message: read String.from_int(value: Result.unwrap_or_else<Int, String>(result: read checked(ok: true), fallback: |error| {
        return String.len(value: read error)
    })))
    Log.write(message: read String.from_int(value: Result.unwrap_or_else<Int, String>(result: read checked(ok: false), fallback: |error| {
        return String.len(value: read error)
    })))
    match Result.map<Int, String, Int>(result: read checked(ok: true), mapper: |item| {
        return item + 4
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Result.and_then<Int, String, Int>(result: read checked(ok: true), mapper: |item| {
        return Ok(item + 6)
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    match Result.map_error<Int, String, String>(result: read checked(ok: false), mapper: |error| {
        return String.concat(left: read error, right: read "!")
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-option-result-closure.rss", source, []);
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
        Log.write(message: read "ok")
        return Unit
    }
    Log.write(message: read "bad")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-logical-short-circuit.rss", source, []);
}

#[test]
fn reg_vm_runs_log_and_workspace_intrinsics_like_interpreter() {
    let source = r#"
features: native, local

fn main() -> Unit {
    Log.write(message: read "stdout line")
    Log.write_json(value: read Json.value(value: read {"stream": "stdout", "count": 1}))
    Log.error(message: read "stderr line")
    Log.error_json(value: read Json.value(value: read {"stream": "stderr", "count": 2}))
    Log.trace(event: read "parity.event", message: read "traced")

    let root = Env.run_workspace_root()
    match Workspace.resolve(root: read root, relative: read "Cargo.toml") {
        Ok(path) => {
            if Path.exists(path: read path) {
                Log.write(message: read "workspace-resolved")
            }
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-log-workspace.rss", source, []);
}

#[test]
fn reg_vm_runs_list_index_scan_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut index = 0
    local values = List<Int>.new()
    while index < 10 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < List.len<Int>(list: read values) {
        total = total + List.get<Int>(list: read values, index: index)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-index.rss", source, []);
}

#[test]
fn reg_vm_runs_array_literal_and_index_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let values: List<Int> = [2, 4, 6, 8]
    let total = values[0] + values[1] + values[2] + values[3]
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-array-index.rss", source, []);
}

#[test]
fn reg_vm_runs_for_break_continue_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let values: List<Int> = [1, 2, 3, 4, 5, 6]
    let mut total = 0
    for value in values {
        if value == 2 {
            continue
        }
        if value == 5 {
            break
        }
        total = total + value
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-for-break-continue.rss", source, []);
}

#[test]
fn reg_vm_runs_while_break_continue_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let mut index = 0
    let mut total = 0
    while index < 8 {
        index = index + 1
        if index == 2 {
            continue
        }
        if index == 6 {
            break
        }
        total = total + index
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-while-break-continue.rss", source, []);
}

#[test]
fn reg_vm_runs_map_literal_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let table: Map<String, Int> = {"one" => 1, "two" => 2, "three" => 3}
    let mut total = 0
    match Map.get<String, Int>(map: read table, key: read "one") {
        Some(value) => {
            total = total + value
        }
        None => {
            total = total - 10
        }
    }
    match Map.get<String, Int>(map: read table, key: read "three") {
        Some(value) => {
            total = total + value
        }
        None => {
            total = total - 10
        }
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-literal.rss", source, []);
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
fn reg_vm_runs_managed_struct_value_like_interpreter() {
    let source = r#"
features: local

struct Box {
    value: Int
}

fn main() -> Unit {
    local box = Box(value: 7)
    let managed = manage box
    Log.write(message: read String.from_int(value: managed.value))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-managed-struct.rss", source, []);
}

#[test]
fn reg_vm_runs_weak_intrinsics_like_compiled_backend() {
    let source = r#"
features: local

class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn main() -> Unit {
    let user = User(id: 7)
    let session = Session(owner: Weak.from(value: read user))
    let downgraded = Weak.downgrade(value: read user)
    match Weak.upgrade(value: read session.owner) {
        Some(owner) => Log.write(message: read String.from_int(value: owner.id))
        None => Log.write(message: read "missing")
    }
    match Weak.upgrade(value: read downgraded) {
        Some(owner) => Log.write(message: read String.from_int(value: owner.id))
        None => Log.write(message: read "missing")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-weak-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_capability_from_construction_like_compiled_backend() {
    let source = r#"
features: local

protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct BufferWriter {
    count: Int
}

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit {
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn main() -> Unit {
    local writer = BufferWriter(count: 1)
    local cap: Capability<Writer> = Capability<Writer>.from(value: take writer)
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-capability-from.rss", source, []);
}

#[test]
fn reg_vm_runs_native_host_bindings_like_interpreter() {
    fn host_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "HostHandle".to_string(),
            id: 7,
        })
    }

    fn host_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("{type_name}:{id}")))
    }

    fn host_echo(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::String(message)] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("host:{message}")))
    }

    let source = r#"
features: native

opaque struct HostHandle

native fn Host.open() -> HostHandle
    effects(native)

native fn Host.describe(handle: read HostHandle) -> String
    effects(native)

native fn Host.echo(message: read String) -> String
    effects(native)

fn main() -> Unit {
    let handle = Host.open()
    Log.write(message: read Host.describe(handle: read handle))
    Log.write(message: read Host.echo(message: read "native"))
    return Unit
}
"#;

    assert_reg_vm_with_native_output(
        "reg-vm-native-host.rss",
        source,
        [],
        [
            ("Host.open", host_open as NativeInterpreterFn),
            ("Host.describe", host_describe as NativeInterpreterFn),
            ("Host.echo", host_echo as NativeInterpreterFn),
        ],
        "HostHandle:7\nhost:native\n",
    );
}

#[test]
fn reg_vm_runs_receiver_native_host_bindings_like_interpreter() {
    fn alpha_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "Alpha".to_string(),
            id: 1,
        })
    }

    fn beta_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "Beta".to_string(),
            id: 2,
        })
    }

    fn alpha_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("alpha:{type_name}:{id}")))
    }

    fn beta_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("beta:{type_name}:{id}")))
    }

    let source = r#"
features: native

opaque struct Alpha
opaque struct Beta

native fn Alpha.open() -> Alpha
    effects(native)

native fn Alpha.describe(self: read Alpha) -> String
    effects(native)

native fn Beta.open() -> Beta
    effects(native)

native fn Beta.describe(self: read Beta) -> String
    effects(native)

fn main() -> Unit {
    let alpha = Alpha.open()
    let beta = Beta.open()
    Log.write(message: read alpha.describe())
    Log.write(message: read beta.describe())
    return Unit
}
"#;

    assert_reg_vm_with_native_output(
        "reg-vm-receiver-native-host.rss",
        source,
        [],
        [
            ("Alpha.open", alpha_open as NativeInterpreterFn),
            ("Alpha.describe", alpha_describe as NativeInterpreterFn),
            ("Beta.open", beta_open as NativeInterpreterFn),
            ("Beta.describe", beta_describe as NativeInterpreterFn),
        ],
        "alpha:Alpha:1\nbeta:Beta:2\n",
    );
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
fn reg_vm_runs_try_ok_like_interpreter() {
    let source = r#"
fn checked(value: Int) -> Result<Int, String> {
    return Ok(value + 1)
}

fn main() -> Result<Unit, String> {
    let value = checked(value: 4)?
    Log.write(message: read String.from_int(value: value))
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-try-ok.rss", source, []);
}

#[test]
fn reg_vm_runs_try_err_return_like_interpreter() {
    let source = r#"
fn checked(ok: Bool) -> Result<Int, String> {
    if ok {
        return Ok(7)
    }
    return Err("bad")
}

fn main() -> Result<Unit, String> {
    let value = checked(ok: false)?
    Log.write(message: read String.from_int(value: value))
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend_return(
        "reg-vm-try-err.rss",
        source,
        [],
        CompiledReturnHarness::ResultUnitString,
    );
}

#[test]
fn reg_vm_runs_list_closure_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn is_even(value: Int) -> Bool {
    let half = value / 2
    return half * 2 == value
}

fn main() -> Unit {
    let numbers: List<Int> = [1, 2, 3, 4, 5]
    Log.write(message: read String.from_int(value: List.count_where<Int>(list: read numbers, predicate: |item| {
        return item > 3
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
        return item > 3
    }) {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "find-none")
        }
    }

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

    match List.try_fold<Int, Int, String>(list: read numbers, initial: read 0, folder: |state, item| {
        if item > 3 {
            return Err(String.copy(value: read "too-large"))
        }
        return Ok(state + item)
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    match List.try_fold<Int, Int, String>(list: read [2, 4], initial: read 0, folder: |state, item| {
        if item < 0 {
            return Err(String.copy(value: read "negative"))
        }
        return Ok(state + item)
    }) {
        Ok(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    let filtered = List.filter<Int>(list: read numbers, predicate: |item| {
        return is_even(value: item)
    })
    let flattened = List.flat_map<Int, Int>(list: read filtered, mapper: |item| {
        let values: List<Int> = [item, item + 10]
        return values
    })
    Log.write(message: read String.from_int(value: List.len<Int>(list: read flattened)))
    Log.write(message: read String.from_int(value: flattened[1]))
    Log.write(message: read String.from_int(value: flattened[3]))

    let parts = List.partition<Int>(list: read numbers, predicate: |item| {
        return item > 3
    })
    Log.write(message: read String.from_int(value: List.len<Int>(list: read parts[0])))
    Log.write(message: read String.from_int(value: parts[0][0]))
    Log.write(message: read String.from_int(value: List.len<Int>(list: read parts[1])))
    Log.write(message: read String.from_int(value: parts[1][2]))

    let zip_left = [1, 2, 3]
    let zip_right = [4, 5]
    let zipped = List.zip<Int>(left: read zip_left, right: read zip_right)
    Log.write(message: read String.from_int(value: List.len(list: read zipped)))
    Log.write(message: read String.from_int(value: zipped[0][0]))
    Log.write(message: read String.from_int(value: zipped[0][1]))
    Log.write(message: read String.from_int(value: zipped[1][1]))

    let enumerate_values = [7, 8]
    let indexed = List.enumerate(list: read enumerate_values)
    Log.write(message: read String.from_int(value: indexed[0][0]))
    Log.write(message: read String.from_int(value: indexed[0][1]))
    Log.write(message: read String.from_int(value: indexed[1][0]))
    Log.write(message: read String.from_int(value: indexed[1][1]))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-closure-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_list_closure_pipeline_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut index = 0
    local values = List<Int>.new()
    while index < 10 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    let mapped = List.map<Int, Int>(
        list: read values,
        mapper: |value| {
            return value * 2 + 1
        },
    )
    let filtered = List.filter<Int>(
        list: read mapped,
        predicate: |value| {
            let half = value / 2
            return half * 2 != value
        },
    )
    let total = List.fold<Int, Int>(
        list: read filtered,
        initial: read 0,
        folder: |state, value| {
            return state + value
        },
    )

    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-closure-pipeline.rss", source, []);
}

#[test]
fn reg_vm_runs_pipeline_chain_like_interpreter() {
    let source = r#"
features: local

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

    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-pipeline-chain.rss", source, []);
}

#[test]
fn reg_vm_runs_map_insert_lookup_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut index = 0
    local table = Map<Int, Int>.new()
    while index < 10 {
        let value = index * 3
        Map.insert<Int, Int>(map: mut table, key: read index, value: read value)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < 10 {
        match Map.get<Int, Int>(map: read table, key: read index) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1
            }
        }
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-lookup.rss", source, []);
}

#[test]
fn reg_vm_runs_string_key_map_like_interpreter() {
    let source = r#"
features: local

fn key_for(value: Int) -> String {
    return String.concat(left: read "key-", right: read String.from_int(value: value))
}

fn main() -> Unit {
    let mut index = 0
    local table = Map<String, Int>.new()
    while index < 10 {
        let key = key_for(value: index)
        Map.insert<String, Int>(map: mut table, key: read key, value: read index)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < 10 {
        let key = key_for(value: index)
        match Map.get<String, Int>(map: read table, key: read key) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1
            }
        }
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-map-string-key.rss", source, []);
}

#[test]
fn vm_runs_args_parse_match_like_interpreter() {
    let source = r#"
fn bench_size(default: Int) -> Int {
    let raw = Args.get_or_default(index: 0, default: read String.from_int(value: default))
    match String.parse_int(value: read raw) {
        Some(value) => {
            return value
        }
        None => {
            return default
        }
    }
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: bench_size(default: 7)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-args-match.rss", source, ["11"]);
}

#[test]
fn vm_runs_list_index_scan_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut index = 0
    local values = List<Int>.new()
    while index < 10 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < List.len<Int>(list: read values) {
        total = total + List.get<Int>(list: read values, index: index)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-list-index.rss", source, []);
}

#[test]
fn vm_runs_map_insert_lookup_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut index = 0
    local table = Map<Int, Int>.new()
    while index < 10 {
        let value = index * 3
        Map.insert<Int, Int>(map: mut table, key: read index, value: read value)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < 10 {
        match Map.get<Int, Int>(map: read table, key: read index) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1
            }
        }
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-map-lookup.rss", source, []);
}

#[test]
fn vm_runs_string_key_map_like_interpreter() {
    let source = r#"
features: local

fn key_for(value: Int) -> String {
    return String.concat(left: read "key-", right: read String.from_int(value: value))
}

fn main() -> Unit {
    let mut index = 0
    local table = Map<String, Int>.new()
    while index < 10 {
        let key = key_for(value: index)
        Map.insert<String, Int>(map: mut table, key: read key, value: read index)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < 10 {
        let key = key_for(value: index)
        match Map.get<String, Int>(map: read table, key: read key) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1
            }
        }
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-map-string-key.rss", source, []);
}

#[test]
fn vm_runs_list_closure_pipeline_like_interpreter() {
    let source = r#"
features: local

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

    let mapped = List.map<Int, Int>(
        list: read values,
        mapper: |value| {
            return value * 2 + 1
        },
    )
    let filtered = List.filter<Int>(
        list: read mapped,
        predicate: |value| {
            let half = value / 2
            return half * 2 != value
        },
    )
    let acc = List.fold<Int, Acc>(
        list: read filtered,
        initial: read Acc(total: 0),
        folder: |state, value| {
            return Acc(total: state.total + value)
        },
    )

    Log.write(message: read String.from_int(value: acc.total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-list-closure-pipeline.rss", source, []);
}

#[test]
fn vm_runs_pipeline_chain_like_interpreter() {
    let source = r#"
features: local

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

    Log.write(message: read String.from_int(value: acc.total))
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
            Log.write(message: read applied)
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    let empty_patch = Diff.unified(old: read original, new: read original)
    match Patch.apply_text(original: read original, patch: read empty_patch) {
        Ok(applied) => {
            Assert.equal(left: read applied, right: read original)
            Log.write(message: read "empty-ok")
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    match Patch.apply_text(original: read "old\n", patch: read "--- old\n+++ new\n@@ -1,1 +1,1 @@\n-bad\n+new\n") {
        Ok(applied) => {
            Log.write(message: read applied)
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    Log.write(message: read String.from_int(value: Ord.compare<Int>(self: read 1, other: read 2)))
    Log.write(message: read String.from_int(value: Ord.compare<String>(self: read "b", other: read "a")))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-diff-patch-ord.rss", source, []);
}

#[test]
fn reg_vm_runs_environment_function_and_request_response_like_interpreter() {
    let source = r#"
features: local

fn main() -> Result<Unit, HttpError> {
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

    let request = Request.new(path: read "/users/42")
    Log.write(message: read Request.path(request: read request))
    let response = Response.ok(body: read "handled")?
    Log.write(message: read String.from_int(value: Response.status(response: read response)))
    Log.write(message: read Response.body(response: read response))
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-env-function-request-response.rss", source, []);
}

#[test]
fn reg_vm_runs_file_parse_and_config_load_like_interpreter() {
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
    let config_path = root.join("config.txt");
    let rules_path = root.join("rules.txt");
    fs::write(&json_path, r#"{"name":"rss","count":3}"#).expect("json fixture should write");
    fs::write(&toml_path, "name = \"rss\"\ncount = 4\n").expect("toml fixture should write");
    fs::write(&yaml_path, "name: rss\ncount: 5\n").expect("yaml fixture should write");
    fs::write(&config_path, "\nprimary\nignored\n").expect("config fixture should write");
    fs::write(&rules_path, "one\ntwo\n\nthree\n").expect("rules fixture should write");

    let args = [
        json_path.display().to_string(),
        toml_path.display().to_string(),
        yaml_path.display().to_string(),
        config_path.display().to_string(),
        rules_path.display().to_string(),
    ];
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    let source = r#"
fn main() -> Result<Unit, JsonError> {
    let json_path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 0), default: read "missing-json"))
    let toml_path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 1), default: read "missing-toml"))
    let yaml_path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 2), default: read "missing-yaml"))
    let config_path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 3), default: read "missing-config"))
    let rules_path = Path.from_string(value: read Option.unwrap_or<String>(value: read Args.get(index: 4), default: read "missing-rules"))

    let json = Json.parse_file(path: read json_path)?
    let json_name = Json.field(value: read json, name: read "name")?
    let json_count = Json.field(value: read json, name: read "count")?
    let json_name_text = Json.as_string(value: read json_name)?
    let json_count_int = Json.as_int(value: read json_count)?
    Log.write(message: read json_name_text)
    Log.write(message: read String.from_int(value: json_count_int))

    let toml = Toml.parse_file(path: read toml_path)?
    let toml_name = Json.field(value: read toml, name: read "name")?
    let toml_count = Json.field(value: read toml, name: read "count")?
    let toml_name_text = Json.as_string(value: read toml_name)?
    let toml_count_int = Json.as_int(value: read toml_count)?
    Log.write(message: read toml_name_text)
    Log.write(message: read String.from_int(value: toml_count_int))

    let yaml = Yaml.parse_file(path: read yaml_path)?
    let yaml_name = Json.field(value: read yaml, name: read "name")?
    let yaml_count = Json.field(value: read yaml, name: read "count")?
    let yaml_name_text = Json.as_string(value: read yaml_name)?
    let yaml_count_int = Json.as_int(value: read yaml_count)?
    Log.write(message: read yaml_name_text)
    Log.write(message: read String.from_int(value: yaml_count_int))

    match Config.load(path: read config_path) {
        Ok(config) => {
            Log.write(message: read Config.name(value: read config))
        }
        Err(error) => {
            Log.write(message: read "config-error")
        }
    }

    match RuleLoader.load_rules(path: read rules_path) {
        Ok(rules) => {
            let config = Config.new(name: read "rules", rules: read rules)
            Log.write(message: read String.from_int(value: Config.rule_count(config: read config)))
        }
        Err(error) => {
            Log.write(message: read "rules-error")
        }
    }

    match Json.parse_file(path: read Path.from_string(value: read "missing-json-file")) {
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

    assert_reg_vm_matches_compiled_backend("reg-vm-file-parse-config.rss", source, arg_refs);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reg_vm_runs_list_mutator_and_json_intrinsics_like_interpreter() {
    let source = r#"
features: local

fn main() -> Unit {
    let mut values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 3)
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.set<Int>(list: mut values, index: 1, value: read 10)
    let suffix: List<Int> = [4, 5]
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
    List.sort<Int>(list: mut values)
    Log.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    Log.write(message: read String.from_int(value: values[0]))
    Log.write(message: read String.from_int(value: values[2]))
    List.clear<Int>(list: mut values)
    if List.is_empty<Int>(list: read values) {
        Log.write(message: read "cleared")
    }

    let words: List<String> = ["b", "a"]
    let json_strings = List.to_json_strings(list: read words)
    Log.write(message: read Json.to_string(value: read json_strings))
    let json_values: List<JsonValue> = [Json.value(value: read {"n": 1}), Json.value(value: read {"n": 2})]
    let json_array = List.to_json_values(list: read json_values)
    Log.write(message: read Json.to_string(value: read json_array))

    local taken = List.take<Int>(list: read [1, 2], count: 1)
    List.consume<Int>(list: take taken)
    Log.write(message: read "consumed")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-list-mutator-json.rss", source, []);
}

#[test]
fn reg_vm_runs_map_mutator_intrinsics_like_interpreter() {
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

    assert_reg_vm_matches_compiled_backend("reg-vm-map-mutators.rss", source, []);
}

#[test]
fn reg_vm_runs_map_closure_intrinsics_like_interpreter() {
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

    assert_reg_vm_matches_compiled_backend("reg-vm-map-closure-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_cancellation_and_stream_intrinsics_like_interpreter() {
    let source = r#"
features: native, local, async

async fn main() -> Result<Unit, ChannelError> {
    local source = CancellationSource.new()
    let token = CancellationSource.token(source: read source)
    if !CancellationToken.is_cancelled(token: read token) {
        Log.write(message: read "not-cancelled")
    }
    CancellationSource.cancel(source: mut source)
    if CancellationToken.is_cancelled(token: read token) {
        Log.write(message: read "cancelled")
    }
    let second = CancellationSource.token(source: read source)
    if CancellationToken.is_cancelled(token: read second) {
        Log.write(message: read "second-cancelled")
    }

    local items = List<Int>.new()
    List.push<Int>(list: mut items, value: read 1)
    List.push<Int>(list: mut items, value: read 2)
    List.push<Int>(list: mut items, value: read 3)
    let stream: Stream<Int> = Stream.from_list<Int>(items: take items)
    match await Stream.next<Int>(stream: read stream)? {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "first-none")
        }
    }
    let remaining = Stream.collect_list<Int>(stream: read stream)?
    Log.write(message: read String.from_int(value: List.len<Int>(list: read remaining)))
    Log.write(message: read String.from_int(value: remaining[0]))
    Log.write(message: read String.from_int(value: remaining[1]))

    local empty_items = List<Int>.new()
    let empty_stream: Stream<Int> = Stream.from_list<Int>(items: take empty_items)
    match await Stream.next<Int>(stream: read empty_stream)? {
        Some(value) => {
            Log.write(message: read String.from_int(value: value))
        }
        None => {
            Log.write(message: read "empty-none")
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
features: native, local

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
features: native, local, async

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
                            Log.write(message: read "sent")
                        }
                        Err(error) => {
                            Log.write(message: read ChannelError.message(error: read error))
                        }
                    }
                    match await Receiver.recv<Int>(receiver: read receiver) {
                        Ok(maybe_item) => {
                            match maybe_item {
                                Some(item) => {
                                    Log.write(message: read String.from_int(value: item))
                                }
                                None => {
                                    Log.write(message: read "none")
                                }
                            }
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
        }
        Err(error) => {
            Log.write(message: read ChannelError.message(error: read error))
        }
    }

    let url = Url.from_string(value: read "https://example.test/api")
    match Http.get(url: read url) {
        Ok(response) => {
            Log.write(message: read HttpResponse.text(response: read response))
            Log.write(message: read String.from_int(value: Bytes.len(value: read HttpResponse.bytes(response: read response))))
        }
        Err(error) => {
            Log.write(message: read HttpError.message(error: read error))
        }
    }

    let stdout = Process.run_stdout(command: read "printf", args: read ["vm"])?
    Log.write(message: read stdout)
    let output = Process.run(command: read "printf", args: read ["ok"])?
    Log.write(message: read output.stdout)
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-runtime-facade-batch.rss", source, []);
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
features: local

fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rss-vm-tempdir"))
    let marker = Path.from_string(value: read Args.get_or_default(index: 1, default: read "target/rss-vm-tempdir/marker.txt"))

    if Path.exists(path: read root) {
        Log.write(message: read "root-exists")
    }
    if Path.is_dir(path: read root) {
        Log.write(message: read "root-dir")
    }
    if Path.exists(path: read marker) {
        Log.write(message: read "marker-exists")
    }
    if Path.is_file(path: read marker) {
        Log.write(message: read "marker-file")
    }

    with TempDir.new_in(parent: read root)? as child {
        let path = TempDir.path(dir: read child)
        if Path.is_dir(path: read path) {
            Log.write(message: read "child-dir")
        }
    }

    with TempDir.new()? as created {
        let path = TempDir.path(dir: read created)
        if Path.is_dir(path: read path) {
            Log.write(message: read "created-dir")
        }
    }

    with TempDir.new_in(parent: read root)? as kept {
        let path = TempDir.keep(dir: take kept)
        if Path.is_dir(path: read path) {
            Log.write(message: read "kept-dir")
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
features: local

fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rss-vm-file-core"))
    let path_file = Path.join(base: read root, child: read "path.txt")
    File.write_string_to_path(path: read path_file, text: read "file text")?
    if File.exists(path: read path_file) {
        Log.write(message: read "path-exists")
    }
    let file_text = File.read_string(path: read path_file)?
    Log.write(message: read file_text)

    let bytes_file = Path.join(base: read root, child: read "bytes.bin")
    File.write_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "abc"))?
    let bytes = File.read_bytes(path: read bytes_file)?
    Log.write(message: read String.from_int(value: Bytes.len(value: read bytes)))

    let handle_file = Path.join(base: read root, child: read "handle.txt")
    with File.open_write(path: read handle_file)? as writer {
        File.write(file: mut writer, data: read Bytes.from_string(value: read "ab"))?
        File.write_string(file: mut writer, text: read "cd")?
    }
    with File.open(path: read handle_file)? as reader {
        let all = File.read_all(file: mut reader)?
        Log.write(message: read String.from_int(value: Bytes.len(value: read all)))
        let empty = File.read_all(file: mut reader)?
        Log.write(message: read String.from_int(value: Bytes.len(value: read empty)))
    }
    with File.open_read(path: read handle_file)? as reader_text {
        let text_all = File.read_all_string(file: mut reader_text)?
        Log.write(message: read text_all)
    }
    with File.open_read(path: read handle_file)? as reader_into {
        local into_buffer = Buffer.new(size: 0)
        if File.read_into(file: mut reader_into, buffer: mut into_buffer)? {
            Log.write(message: read String.from_int(value: Buffer.len(buffer: read into_buffer)))
        }
        if !File.read_into(file: mut reader_into, buffer: mut into_buffer)? {
            Log.write(message: read "read-into-empty")
        }
    }

    File.remove(path: read path_file)?
    if !File.exists(path: read path_file) {
        Log.write(message: read "removed")
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
features: local

fn main() -> Result<Unit, FileError> {
    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rss-vm-directory"))
    let nested = Path.join(base: read root, child: read "nested")
    let deep = Path.join(base: read nested, child: read "deep")
    let single = Path.join(base: read root, child: read "single")

    Directory.create_all(path: read nested)?
    Directory.create_dir_all(path: read deep)?
    Directory.create(path: read single)?
    if Directory.exists(path: read root) {
        Log.write(message: read "root-exists")
    }
    if Directory.is_dir(path: read deep) {
        Log.write(message: read "deep-dir")
    }

    let path_file = Path.join(base: read nested, child: read "path.txt")
    Directory.write_string(path: read path_file, content: read "path text")?
    if Directory.is_file(path: read path_file) {
        Log.write(message: read "directory-file")
    }
    let path_text = Directory.read_string(path: read path_file)?
    Log.write(message: read path_text)
    let path_digest = Hash.sha256_file(path: read path_file)?
    Assert.equal(left: read path_digest, right: read "c6465e0abd2e3c2f5ccfe7f639ddc0f72282904663b09ddd8dffbe060be35f97")

    let metadata = Directory.metadata(path: read path_file)?
    if metadata.is_file {
        Log.write(message: read "metadata-file")
    }
    Log.write(message: read String.from_int(value: metadata.len))

    let bytes_file = Path.join(base: read nested, child: read "bytes.bin")
    File.write_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "abc"))?
    File.append_bytes(path: read bytes_file, data: read Bytes.from_string(value: read "de"))?
    File.append_string(path: read bytes_file, text: read "f")?
    let bytes = File.read_bytes(path: read bytes_file)?
    Log.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    match File.bytes_stream(path: read bytes_file, chunk_size: 2) {
        Ok(stream) => {
            match Stream.collect_list<Bytes>(stream: read stream) {
                Ok(chunks) => {
                    Log.write(message: read String.from_int(value: List.len<Bytes>(list: read chunks)))
                    Log.write(message: read String.from_int(value: Bytes.len(value: read chunks[0])))
                    Log.write(message: read String.from_int(value: Bytes.len(value: read chunks[1])))
                    Log.write(message: read String.from_int(value: Bytes.len(value: read chunks[2])))
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

    let files = Directory.list_files(path: read root)?
    if List.contains_value<String>(list: read files, value: read "nested/path.txt") {
        Log.write(message: read "listed-path")
    }
    if List.contains_value<String>(list: read files, value: read "nested/bytes.bin") {
        Log.write(message: read "listed-bytes")
    }
    let paths = Directory.list_paths(path: read nested)?
    Log.write(message: read String.from_int(value: List.len<Path>(list: read paths)))

    let copied = Path.join(base: read nested, child: read "copied.txt")
    Directory.copy_file(from: read path_file, to: read copied)?
    let copied_text = File.read_string(path: read copied)?
    Log.write(message: read copied_text)
    let renamed = Path.join(base: read nested, child: read "renamed.txt")
    Directory.rename(from: read copied, to: read renamed)?
    if File.exists(path: read renamed) {
        Log.write(message: read "renamed-exists")
    }
    Directory.remove_file(path: read renamed)?
    if !File.exists(path: read renamed) {
        Log.write(message: read "renamed-removed")
    }
    Directory.remove_dir_all(path: read single)?
    if !Path.exists(path: read single) {
        Log.write(message: read "single-removed")
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
features: async, native, local

async fn main() -> Result<Unit, FileError> {
    let current = Env.current_dir()?
    Env.set_current_dir(path: read current)?
    Env.set(name: read "RSSCRIPT_VM_IGNORED", value: read "ignored")
    if String.len(value: read Path.to_string(path: read Env.run_workspace_root())) > 0 {
        Log.write(message: read "workspace-root")
    }
    if String.len(value: read Path.to_string(path: read Env.temp_dir())) > 0 {
        Log.write(message: read "temp-dir")
    }
    match Env.home_dir() {
        Some(path) => {
            if String.len(value: read Path.to_string(path: read path)) > 0 {
                Log.write(message: read "home-dir")
            }
        }
        None => {
            Log.write(message: read "home-none")
        }
    }

    let root = Path.from_string(value: read Args.get_or_default(index: 0, default: read "target/rss-vm-env-path-file"))
    let nested = Path.join(base: read root, child: read "nested")
    Directory.create_all(path: read nested)?

    let path_file = Path.join(base: read nested, child: read "path.txt")
    Path.write_string(path: read path_file, text: read "path text")?
    let path_text = Path.read_string(path: read path_file)?
    Log.write(message: read path_text)

    let async_bytes = Path.join(base: read nested, child: read "async-bytes.bin")
    await File.write_async(path: read async_bytes, data: read Bytes.from_string(value: read "abc"))?
    let async_read = await File.read_all_async(path: read async_bytes)?
    Log.write(message: read String.from_int(value: Bytes.len(value: read async_read)))

    let async_text = Path.join(base: read nested, child: read "async-text.txt")
    await File.write_string_async(path: read async_text, text: read "async text")?
    let async_text_read = await File.read_all_string_async(path: read async_text)?
    Log.write(message: read async_text_read)

    let atomic = Path.join(base: read nested, child: read "atomic.txt")
    File.write_atomic(path: read atomic, text: read "atomic text")?
    let atomic_text = File.read_string(path: read atomic)?
    Log.write(message: read atomic_text)

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
    Log.write(message: read handle_read)

    let files = Path.list_files(path: read root)?
    if List.contains_value<String>(list: read files, value: read "nested/path.txt") {
        Log.write(message: read "path-list-file")
    }
    let paths = Path.list_paths(path: read nested)?
    Log.write(message: read String.from_int(value: List.len<Path>(list: read paths)))
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
fn reg_vm_runs_time_db_and_fallible_pipeline_intrinsics_like_interpreter() {
    let source = r#"
features: native

fn main() -> Result<Unit, String> {
    let start = Clock.now()
    let unix_ms = Clock.system_unix_ms()
    if unix_ms > 0 {
        Log.write(message: read "clock")
    }
    let elapsed = Instant.elapsed(start: read start)
    if Duration.as_ms(value: read elapsed) >= 0 {
        Log.write(message: read "elapsed")
    }

    let immediate = Deadline.after_ms(ms: 0)
    if Deadline.is_expired(deadline: read immediate) {
        Log.write(message: read "expired-now")
    }
    if Deadline.remaining_ms(deadline: read immediate) >= 0 {
        Log.write(message: read "remaining-nonnegative")
    }
    let negative = Deadline.after(duration: read Duration.ms(value: 0 - 1))
    if Deadline.is_expired(deadline: read negative) {
        Log.write(message: read "expired-negative")
    }

    let numbers = [1, 2, 3]
    let touched_numbers = Pipeline.each<Int>(pipeline: read List.pipeline<Int>(list: read numbers), action: |item| {
        Log.write(message: read String.from_int(value: item))
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
        Log.write(message: read String.from_int(value: item))
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
            Log.write(message: read String.from_int(value: List.len<Int>(list: read items)))
            Log.write(message: read String.from_int(value: items[0]))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }

    let failed = Pipeline.try_map<Int, Int, String>(pipeline: read List.pipeline<Int>(list: read numbers), mapper: |item| {
        if item == 2 {
            return Err(String.copy(value: read "stop"))
        }
        return Ok(item + 0)
    })
    let still_failed = FalliblePipeline.map<Int, Int, String>(pipeline: read failed, mapper: |item| {
        Log.write(message: read "should-not-run")
        return item + 1
    })
    match FalliblePipeline.collect<Int, String>(pipeline: read still_failed) {
        Ok(items) => {
            Log.write(message: read String.from_int(value: List.len<Int>(list: read items)))
        }
        Err(error) => {
            Log.write(message: read error)
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-time-db-fallible-pipeline.rss", source, []);
}

#[test]
fn reg_vm_runs_db_connection_intrinsics_like_interpreter() {
    let source = r#"
features: native

fn main() -> Result<Unit, DbError> {
    let url = Url.from_string(value: read "db://vm")
    with DbConnection.open(url: read url) as conn {
        DbConnection.query(conn: mut conn, sql: read "select 1")?
    }
    with DbConnection.try_open(url: read url)? as conn {
        DbConnection.query(conn: mut conn, sql: read "select 2")?
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-db-connection.rss", source, []);
}

#[test]
fn reg_vm_runs_resource_drop_cleanup_like_interpreter() {
    let source = r#"
features: local

resource FileHandle {
    fd: Int

    drop {
        Log.write(message: read "file-drop")
        OS.close(fd: fd)
    }
}

resource DbHandle {
    fd: Int

    drop {
        Log.write(message: read "db-drop")
        Db.close(fd: fd)
    }
}

fn main() -> Unit {
    with FileHandle(fd: 0) as file {
        Log.write(message: read "file-body")
        Log.write(message: read String.from_int(value: file.fd))
    }
    with DbHandle(fd: 0) as db {
        Log.write(message: read "db-body")
        Log.write(message: read String.from_int(value: db.fd))
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-resource-drop-cleanup.rss", source, []);
}

#[test]
fn reg_vm_runs_resource_pool_try_new_like_backend() {
    let source = r#"
features: local native

fn main() -> Result<Unit, DbError> {
    let url = Url.from_string(value: read "db://vm-resource-pool")
    local eager = ResourcePool<DbConnection>.try_new(
        create: || {
            return DbConnection.try_open(url: read url)
        },
        max_size: 2,
    )?
    with ResourcePool.borrow(pool: mut eager) as session {
        DbConnection.query(conn: mut session, sql: read "select eager")?
    }

    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-resource-pool-try-new.rss", source, []);
}

#[test]
fn reg_vm_runs_resource_drop_unwind_like_interpreter() {
    let source = r#"
features: local

resource Handle {
    id: Int

    drop {
        Log.write(message: read "drop")
        Log.write(message: read String.from_int(value: id))
    }
}

fn return_case() -> Unit {
    with Handle(id: 1) as handle {
        Log.write(message: read "return-body")
        Log.write(message: read String.from_int(value: handle.id))
        return Unit
    }
    Log.write(message: read "return-after")
    return Unit
}

fn break_case() -> Unit {
    let mut index = 0
    while index < 1 {
        with Handle(id: 2) as handle {
            Log.write(message: read "break-body")
            Log.write(message: read String.from_int(value: handle.id))
            break
        }
        Log.write(message: read "break-after-with")
    }
    Log.write(message: read "after-break")
    return Unit
}

fn continue_case() -> Unit {
    let mut index = 0
    while index < 2 {
        with Handle(id: index + 3) as handle {
            Log.write(message: read "continue-body")
            Log.write(message: read String.from_int(value: handle.id))
            index = index + 1
            continue
        }
        Log.write(message: read "continue-after-with")
    }
    Log.write(message: read "after-continue")
    return Unit
}

fn fail() -> Result<Unit, String> {
    return Err(String.copy(value: read "boom"))
}

fn try_case() -> Result<Unit, String> {
    with Handle(id: 5) as handle {
        Log.write(message: read "try-body")
        Log.write(message: read String.from_int(value: handle.id))
        fail()?
    }
    Log.write(message: read "try-after")
    return Ok(Unit)
}

fn main() -> Unit {
    return_case()
    break_case()
    continue_case()
    match try_case() {
        Ok(_) => {
            Log.write(message: read "try-ok")
        }
        Err(message) => {
            Log.write(message: read message)
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-resource-drop-unwind.rss", source, []);
}

#[test]
fn reg_vm_runs_receiver_methods_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let greeting = String.concat(left: read "hi ", right: read "there")
    Log.write(message: read String.from_int(value: greeting.len()))
    let n = 255
    Log.write(message: read n.to_string())
    let blank = ""
    if blank.is_empty() {
        Log.write(message: read "blank-empty")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-gap-receiver-methods.rss", source, []);
}

#[test]
fn reg_vm_runs_async_for_like_interpreter() {
    let source = r#"
features: async, local

async fn main() -> Result<Unit, ChannelError> {
    local values = [1, 2, 3]
    let stream = Stream.from_list<Int>(items: take values)
    await for value in stream {
        Log.write(message: read String.from_int(value: value))
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-async-for.rss", source, []);
}

#[test]
fn reg_vm_runs_index_assignment_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let mut values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.push<Int>(list: mut values, value: read 3)
    values[2] = 30
    Log.write(message: read String.from_int(value: values[2]))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-gap-index-assignment.rss", source, []);
}

#[test]
fn reg_vm_runs_struct_match_like_interpreter() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

fn main() -> Unit {
    let point = Point(x: 3, y: 4)
    match read point {
        Point { x, y } => {
            Log.write(message: read String.from_int(value: x + y))
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-gap-struct-match.rss", source, []);
}

#[test]
fn reg_vm_runs_guarded_match_like_compiled_backend() {
    let source = r#"
fn main() -> Unit {
    let value = Some(3)
    match value {
        Some(item) if item > 0 => {
            Log.write(message: read "positive")
        }
        Some(_) => {
            Log.write(message: read "other")
        }
        None => {
            Log.write(message: read "none")
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-guarded-match.rss", source, []);
}

#[test]
fn reg_vm_runs_literal_match_like_compiled_backend() {
    let source = r#"
fn main() -> Unit {
    let value = 1
    match value {
        1 => {
            Log.write(message: read "one")
        }
        _ => {
            Log.write(message: read "other")
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-literal-match.rss", source, []);
}

#[test]
fn reg_vm_lowers_structured_sum_variant_match_like_compiled_backend() {
    let source = r#"
sum Expr {
    Call(callee: String, arg_count: Int)
    Literal(value: String)
}

fn describe(expr: read Expr) -> Unit {
    match read expr {
        Call { callee, arg_count } if arg_count == 0 => {
            Log.write(message: read callee)
        }
        Call { callee, .. } => {
            Log.write(message: read String.concat(left: read callee, right: read ":args"))
        }
        Literal { value } => {
            Log.write(message: read value)
        }
    }
    return Unit
}

fn main() -> Unit {
    return Unit
}
"#;

    reg_vm_compile_source("reg-vm-structured-match.rss", source)
        .expect("reg VM should lower structured sum variant match");
}

#[test]
fn reg_vm_runs_match_expression_like_compiled_backend() {
    let source = r#"
sum Direction {
    North
    South
    East
    West
}

fn direction_name(d: read Direction) -> String {
    return match d {
        North => { "north" }
        South => { "south" }
        East => { "east" }
        West => { "west" }
    }
}

fn int_label(value: Int) -> String {
    return match value {
        0 => { "zero" }
        1 => { "one" }
        _ => { "many" }
    }
}

fn main() -> Unit {
    let d = North
    Log.write(message: read direction_name(d: read d))
    Log.write(message: read int_label(value: 1))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-match-expression.rss", source, []);
}

#[test]
fn reg_vm_runs_guarded_match_expression_like_compiled_backend() {
    let source = r#"
sum Direction {
    North
    South
}

fn describe(value: read Direction, enabled: Bool) -> String {
    return match value {
        North if enabled => { "enabled north" }
        North => { "north" }
        South => { "south" }
    }
}

fn main() -> Unit {
    let north = North
    let south = South
    Log.write(message: read describe(value: read north, enabled: true))
    Log.write(message: read describe(value: read north, enabled: false))
    Log.write(message: read describe(value: read south, enabled: true))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-guarded-match-expression.rss", source, []);
}

#[test]
fn reg_vm_runs_immediate_multi_arm_select_like_compiled_backend() {
    let source = r#"
features: async

async fn ready(value: Int) -> Result<Int, String> {
    return Ok(value)
}

fn main() -> Result<Unit, String> {
    select {
        value = await ready(value: 7)? => {
            Log.write(message: read String.from_int(value: value))
        }
        other = await ready(value: 9)? => {
            Log.write(message: read String.from_int(value: other))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-immediate-select.rss", source, []);
}

#[test]
fn reg_vm_runs_select_winner_by_timing_not_arm_order_like_backend() {
    // The shorter sleep is the *second* arm, so a correct first-ready select must
    // pick it (value 9) — proving the winner is decided by the scheduler clock,
    // not by arm declaration order.
    let source = r#"
features: async, native

async fn after(value: Int, ms: Int) -> Result<Int, TimerError> {
    await Timer.sleep(ms: ms)?
    return Ok(value)
}

fn main() -> Result<Unit, TimerError> {
    select {
        value = await after(value: 7, ms: 50)? => {
            Log.write(message: read String.from_int(value: value))
        }
        other = await after(value: 9, ms: 1)? => {
            Log.write(message: read String.from_int(value: other))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-select-second-arm.rss", source, []);
}

#[test]
fn reg_vm_select_does_not_run_loser_side_effects_like_backend() {
    // After `select` picks a winner, the losing arm's operation must NOT keep
    // running and producing side effects. The loser here sleeps longer and then
    // logs "loser ran"; the winner finishes first. main keeps running (a trailing
    // sleep yields the scheduler), which is exactly the window in which an
    // un-cancelled loser would still get scheduled. Expected output: "winner",
    // then "done" — never "loser ran".
    let source = r#"
features: async, native

async fn winner() -> Result<Int, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(1)
}

async fn loser() -> Result<Int, TimerError> {
    await Timer.sleep(ms: 30)?
    Log.write(message: read "loser ran")
    return Ok(2)
}

async fn main() -> Result<Unit, TimerError> {
    select {
        _ = await winner()? => {
            Log.write(message: read "winner")
        }
        _ = await loser()? => {
            Log.write(message: read "loser won")
        }
    }
    await Timer.sleep(ms: 80)?
    Log.write(message: read "done")
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-select-loser-cancel.rss", source, []);
}

#[test]
fn reg_vm_task_group_drains_unawaited_async_let_like_backend() {
    // Structured concurrency: leaving a `task_group` must drain background tasks
    // spawned by `async let` even when they are never explicitly awaited (here
    // `async let _ = background()`). The backend drains at the scope boundary, so
    // "background completed" must print before "after group".
    let source = r#"
features: async

async fn background() -> Result<Unit, String> {
    Log.write(message: read "background completed")
    return Ok(Unit)
}

fn main() -> Result<Unit, String> {
    task_group {
        async let _ = background()
    }
    Log.write(message: read "after group")
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-task-group-drain.rss", source, []);
}

fn assert_reg_vm_matches_compiled_backend<'a>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let reg = reg_vm_eval_source_main_with_args(file, source, args.iter().copied())
        .expect("reg vm should run");
    let runtime_path = common::runtime_path();
    let cache_dir = common::workspace_root().join("target/rsscript-vm-compiled-output-cache");
    fs::create_dir_all(&cache_dir).expect("compiled parity cache dir should create");
    let cache_key = compiled_output_cache_key(file, source, &args);
    let stdout_path = cache_dir.join(format!("{cache_key}.stdout"));
    let stderr_path = cache_dir.join(format!("{cache_key}.stderr"));
    let (stdout, stderr) = if stdout_path.exists() && stderr_path.exists() {
        (
            fs::read_to_string(&stdout_path).expect("cached stdout should read"),
            fs::read_to_string(&stderr_path).expect("cached stderr should read"),
        )
    } else {
        let (stdout, stderr) = run_compiled_backend(file, source, &args, &runtime_path);
        fs::write(&stdout_path, &stdout).expect("cached stdout should write");
        fs::write(&stderr_path, &stderr).expect("cached stderr should write");
        (stdout, stderr)
    };

    assert_eq!(reg.stdout, stdout);
    assert_eq!(reg.stderr, stderr);
}

fn assert_reg_vm_output<'a>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
    expected_display_value: &str,
    expected_stdout: &str,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let reg = reg_vm_eval_source_main_with_args(file, source, args.iter().copied())
        .expect("reg vm should run");

    assert_eq!(reg.value, expected_display_value);
    assert_eq!(reg.display_value, expected_display_value);
    assert_eq!(reg.stdout, expected_stdout);
    assert_eq!(reg.stderr, "");
}

#[derive(Clone, Copy)]
enum CompiledReturnHarness {
    HttpRequest,
    Image,
    JsonValue,
    ResultUnitString,
}

impl CompiledReturnHarness {
    fn cache_tag(self) -> &'static str {
        match self {
            Self::HttpRequest => "return-http-request",
            Self::Image => "return-image",
            Self::JsonValue => "return-json-value",
            Self::ResultUnitString => "return-result-unit-string",
        }
    }

    fn main_rs(self, crate_name: &str) -> String {
        match self {
            Self::HttpRequest => format!(
                concat!(
                    "fn main() {{\n",
                    "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
                    "    let value = {crate_name}::main();\n",
                    "    println!(\"__RSSCRIPT_RETURN__{{}}\", rsscript_runtime::http_request_debug_summary(&value));\n",
                    "}}\n"
                ),
                crate_name = crate_name
            ),
            Self::Image => format!(
                concat!(
                    "fn main() {{\n",
                    "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
                    "    let value = {crate_name}::main();\n",
                    "    println!(\"__RSSCRIPT_RETURN__{{}}\", rsscript_runtime::image_debug_summary(&value));\n",
                    "}}\n"
                ),
                crate_name = crate_name
            ),
            Self::JsonValue => format!(
                concat!(
                    "fn main() {{\n",
                    "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
                    "    let value = {crate_name}::main();\n",
                    "    println!(\"__RSSCRIPT_RETURN__{{}}\", rsscript_runtime::json_to_string(&value));\n",
                    "}}\n"
                ),
                crate_name = crate_name
            ),
            Self::ResultUnitString => format!(
                concat!(
                    "fn main() {{\n",
                    "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
                    "    match {crate_name}::main() {{\n",
                    "        Ok(()) => println!(\"__RSSCRIPT_RETURN__Ok {{{{ value: Unit }}}}\"),\n",
                    "        Err(error) => println!(\"__RSSCRIPT_RETURN__Err {{{{ value: {{}} }}}}\", error),\n",
                    "    }}\n",
                    "}}\n"
                ),
                crate_name = crate_name
            ),
        }
    }
}

fn assert_reg_vm_matches_compiled_backend_return<'a>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
    harness: CompiledReturnHarness,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let reg = reg_vm_eval_source_main_with_args(file, source, args.iter().copied())
        .expect("reg vm should run");
    let runtime_path = common::runtime_path();
    let cache_dir = common::workspace_root().join("target/rsscript-vm-compiled-output-cache");
    fs::create_dir_all(&cache_dir).expect("compiled parity cache dir should create");
    let cache_key = compiled_output_cache_key_with_tag(file, source, &args, harness.cache_tag());
    let stdout_path = cache_dir.join(format!("{cache_key}.stdout"));
    let stderr_path = cache_dir.join(format!("{cache_key}.stderr"));
    let (stdout, stderr) = if stdout_path.exists() && stderr_path.exists() {
        (
            fs::read_to_string(&stdout_path).expect("cached stdout should read"),
            fs::read_to_string(&stderr_path).expect("cached stderr should read"),
        )
    } else {
        let (stdout, stderr) =
            run_compiled_backend_with_return_harness(file, source, &args, &runtime_path, harness);
        fs::write(&stdout_path, &stdout).expect("cached stdout should write");
        fs::write(&stderr_path, &stderr).expect("cached stderr should write");
        (stdout, stderr)
    };

    let (compiled_stdout, compiled_return) =
        split_compiled_return_stdout(&stdout).expect("compiled return marker should exist");
    assert_eq!(reg.stdout, compiled_stdout);
    assert_eq!(reg.stderr, stderr);
    assert_eq!(reg.display_value, compiled_return);
}

fn split_compiled_return_stdout(stdout: &str) -> Option<(String, String)> {
    const MARKER: &str = "__RSSCRIPT_RETURN__";
    let marker_start = stdout.rfind(MARKER)?;
    let return_start = marker_start + MARKER.len();
    let return_end = stdout[return_start..]
        .find('\n')
        .map(|offset| return_start + offset)
        .unwrap_or(stdout.len());
    Some((
        stdout[..marker_start].to_string(),
        stdout[return_start..return_end].to_string(),
    ))
}

fn run_compiled_backend(
    file: &str,
    source: &str,
    args: &[&str],
    runtime_path: &str,
) -> (String, String) {
    let package_name = format!(
        "rsscript_{}",
        file.chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
            .trim_matches('_')
    );
    let package = lower_source_to_rust_package(file, source, &package_name, &runtime_path)
        .expect("source should lower to Rust package");
    let package_dir = common::unique_temp_dir("rsscript-reg-vm-compiled-parity");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", common::generated_target_dir())
        .env("RUSTFLAGS", "-Awarnings");
    if !args.is_empty() {
        command.arg("--").args(args);
    }
    let output = command.output().expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    (stdout, stderr)
}

fn run_compiled_backend_with_return_harness(
    file: &str,
    source: &str,
    args: &[&str],
    runtime_path: &str,
    harness: CompiledReturnHarness,
) -> (String, String) {
    let package_name = format!(
        "rsscript_{}",
        file.chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
            .trim_matches('_')
    );
    let mut package = lower_source_to_rust_package(file, source, &package_name, &runtime_path)
        .expect("source should lower to Rust package");
    let crate_name = package.package_name.replace('-', "_");
    if !package.lib_rs.contains("pub fn main(") {
        package.lib_rs = package.lib_rs.replacen("fn main(", "pub fn main(", 1);
    }
    package.main_rs = Some(harness.main_rs(&crate_name));
    let package_dir = common::unique_temp_dir("rsscript-reg-vm-compiled-parity");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", common::generated_target_dir())
        .env("RUSTFLAGS", "-Awarnings");
    if !args.is_empty() {
        command.arg("--").args(args);
    }
    let output = command.output().expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    (stdout, stderr)
}

fn compiled_output_cache_key(file: &str, source: &str, args: &[&str]) -> String {
    compiled_output_cache_key_with_tag(file, source, args, "stdout-stderr")
}

fn compiled_output_cache_key_with_tag(
    file: &str,
    source: &str,
    args: &[&str],
    tag: &str,
) -> String {
    let crate_root = common::crate_root();
    let workspace_root = common::workspace_root();
    let mut hasher = Sha1::new();
    hasher.update(b"rsscript-vm-compiled-output-v2");
    hasher.update(tag.as_bytes());
    hasher.update(file.as_bytes());
    hasher.update(source.as_bytes());
    for arg in args {
        hasher.update(b"\0arg\0");
        hasher.update(arg.as_bytes());
    }
    hash_path_if_exists(&mut hasher, &workspace_root.join("Cargo.toml"));
    hash_path_if_exists(&mut hasher, &workspace_root.join("Cargo.lock"));
    hash_path_if_exists(&mut hasher, &crate_root.join("Cargo.toml"));
    hash_path_if_exists(&mut hasher, &crate_root.join("build.rs"));
    hash_path_if_exists(&mut hasher, &crate_root.join("src/lib.rs"));
    hash_path_if_exists(&mut hasher, &crate_root.join("src/core_index.rs"));
    hash_path_if_exists(&mut hasher, &crate_root.join("src/runtime_abi.rs"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/analyzer"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/checks"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/package"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/rust_lower"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/syntax"));
    hash_tree_if_exists(&mut hasher, &workspace_root.join("crates/runtime"));
    format!("{:x}", hasher.finalize())
}

fn hash_tree_if_exists(hasher: &mut Sha1, path: &Path) {
    if !path.exists() {
        return;
    }
    if path.is_file() {
        hash_path_if_exists(hasher, path);
        return;
    }
    let mut entries = fs::read_dir(path)
        .expect("cache fingerprint directory should read")
        .map(|entry| entry.expect("cache fingerprint entry should read").path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        hash_tree_if_exists(hasher, &entry);
    }
}

fn hash_path_if_exists(hasher: &mut Sha1, path: &Path) {
    if !path.exists() || !path.is_file() {
        return;
    }
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(fs::read(path).expect("cache fingerprint file should read"));
}

fn assert_reg_vm_with_native_output<'a, const N: usize>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
    native_bindings: [(&'static str, NativeInterpreterFn); N],
    expected_stdout: &str,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let reg = reg_vm_eval_source_main_with_args_and_native_bindings(
        file,
        source,
        args.iter().copied(),
        native_bindings,
    )
    .expect("reg vm should run");

    assert_eq!(reg.value, "Unit");
    assert_eq!(reg.display_value, "Unit");
    assert_eq!(reg.native_value, Some(NativeValue::Unit));
    assert_eq!(reg.stdout, expected_stdout);
    assert_eq!(reg.stderr, "");
}
