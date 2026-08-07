//! eval≡lowered parity: env/random/time/db/image/cache intrinsics
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parity_args_intrinsics() {
    let source = r#"

fn main() -> Unit {
    let args = Args.all()
    Output.write(message: read String.from_int(value: Args.count()))
    Output.write(message: read List.join<String>(list: read args, separator: read "|"))
    match Args.get(index: 0) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "first-none")
        }
    }
    match Args.get(index: 2) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "third-none")
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
    Output.write(message: read Args.get_or_default(index: 1, default: read "fallback"))
    Output.write(message: read Args.get_or_default(index: 99, default: read "fallback"))
    Output.write(message: read Args.get_or_default(index: 0 - 1, default: read "negative-fallback"))
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
fn parity_clock_and_instant_intrinsics() {
    let source = r#"

fn main() -> Unit {
    let unix = Clock.system_unix_ms()
    if unix > 0 {
        Output.write(message: read "unix-positive")
    }
    let start = Clock.now()
    let elapsed = Instant.elapsed(start: read start)
    let elapsed_ms = Duration.as_ms(value: read elapsed)
    Assert.equal_int(left: elapsed_ms, right: elapsed_ms)
    Output.write(message: read "elapsed-ok")
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("parity-clock.rss", "rsscript_parity_clock", source);
}
