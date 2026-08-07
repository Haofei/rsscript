//! Spec §5A — register-VM execution: control flow
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn reg_vm_runs_select_first_ready_like_backend() {
    // `select` runs both arms concurrently; the shorter sleep (1ms) wins over the
    // longer (50ms), so the scheduler's clock — not arm order — must decide.
    let source = r#"

async fn after(value: Int, ms: Int) -> Result<Int, TimerError> {
    await Timer.sleep(ms: ms)?
    return Ok(value)
}

fn main() -> Result<Unit, TimerError> {
    select {
        value = await after(value: 7, ms: 1)? => {
            Output.write(message: read String.from_int(value: value))
        }
        other = await after(value: 9, ms: 50)? => {
            Output.write(message: read String.from_int(value: other))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-select.rss", source, []);
}

#[test]
fn reg_vm_runs_if_expression_like_compiled_backend() {
    let source = r#"
fn choose(flag: Bool) -> Int {
    return if flag {
        7
    } else {
        11
    }
}

fn main() -> Unit {
    Output.write(message: String.from_int(value: choose(flag: true)))
    Output.write(message: String.from_int(value: choose(flag: false)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-if-expression.rss", source, []);
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
    Output.write(message: read String.from_int(value: total))
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
    Output.write(message: read String.from_int(value: total))
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
    Output.write(message: read String.from_int(value: total))
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
    Output.write(message: read String.from_int(value: total))
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
    Output.write(message: read String.from_int(value: bench_size(default: 7)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-if-args.rss", source, ["11"]);
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
    Output.write(message: read String.from_int(value: values[2]))

    let greeting = String.concat(left: read "hi ", right: read "there")
    Output.write(message: read String.from_int(value: greeting.len()))
    let n = 255
    Output.write(message: read n.to_string())
    let blank = ""
    if blank.is_empty() {
        Output.write(message: read "blank-empty")
    }

    let point = Point(x: 3, y: 4)
    match read point {
        Point { x, y } => {
            Output.write(message: read String.from_int(value: x + y))
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-compiled-recent-alignment.rss", source, []);
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
    Output.write(message: read String.from_int(value: total))
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
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-while-break-continue.rss", source, []);
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
    Output.write(message: read String.from_int(value: value))
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
    Output.write(message: read String.from_int(value: bench_size(default: 7)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("vm-args-match.rss", source, ["11"]);
}

#[test]
fn reg_vm_runs_async_for_like_interpreter() {
    let source = r#"

async fn main() -> Result<Unit, ChannelError> {
    local values = [1, 2, 3]
    let stream = Stream.from_list<Int>(items: take values)
    await for value in stream {
        Output.write(message: read String.from_int(value: value))
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-async-for.rss", source, []);
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
            Output.write(message: read String.from_int(value: x + y))
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
            Output.write(message: read "positive")
        }
        Some(_) => {
            Output.write(message: read "other")
        }
        None => {
            Output.write(message: read "none")
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
            Output.write(message: read "one")
        }
        _ => {
            Output.write(message: read "other")
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
            Output.write(message: read callee)
        }
        Call { callee, .. } => {
            Output.write(message: read String.concat(left: read callee, right: read ":args"))
        }
        Literal { value } => {
            Output.write(message: read value)
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
    Output.write(message: read direction_name(d: read d))
    Output.write(message: read int_label(value: 1))
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
    Output.write(message: read describe(value: read north, enabled: true))
    Output.write(message: read describe(value: read north, enabled: false))
    Output.write(message: read describe(value: read south, enabled: true))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-guarded-match-expression.rss", source, []);
}

#[test]
fn reg_vm_runs_immediate_multi_arm_select_like_compiled_backend() {
    let source = r#"

async fn ready(value: Int) -> Result<Int, String> {
    return Ok(value)
}

fn main() -> Result<Unit, String> {
    select {
        value = await ready(value: 7)? => {
            Output.write(message: read String.from_int(value: value))
        }
        other = await ready(value: 9)? => {
            Output.write(message: read String.from_int(value: other))
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

async fn after(value: Int, ms: Int) -> Result<Int, TimerError> {
    await Timer.sleep(ms: ms)?
    return Ok(value)
}

fn main() -> Result<Unit, TimerError> {
    select {
        value = await after(value: 7, ms: 50)? => {
            Output.write(message: read String.from_int(value: value))
        }
        other = await after(value: 9, ms: 1)? => {
            Output.write(message: read String.from_int(value: other))
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

async fn winner() -> Result<Int, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(1)
}

async fn loser() -> Result<Int, TimerError> {
    await Timer.sleep(ms: 30)?
    Output.write(message: read "loser ran")
    return Ok(2)
}

async fn main() -> Result<Unit, TimerError> {
    select {
        _ = await winner()? => {
            Output.write(message: read "winner")
        }
        _ = await loser()? => {
            Output.write(message: read "loser won")
        }
    }
    await Timer.sleep(ms: 80)?
    Output.write(message: read "done")
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-select-loser-cancel.rss", source, []);
}
