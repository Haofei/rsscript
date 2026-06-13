//! register-VM execution: closures and higher-order calls
#![allow(unused_imports, dead_code)]
use super::*;

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
