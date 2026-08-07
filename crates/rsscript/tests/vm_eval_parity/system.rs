//! eval≡lowered parity: env/random/time/db/image/cache intrinsics
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parity_args_intrinsics() {
    let source = r#"

fn main(args: read List<String>) -> Unit {
    let args = Arguments.all(args: read args)
    Output.write(message: read String.from_int(value: Arguments.count(args: read args)))
    Output.write(message: read List.join<String>(list: read args, separator: read "|"))
    match Arguments.get(args: read args, index: 0) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "first-none")
        }
    }
    match Arguments.get(args: read args, index: 2) {
        Some(value) => {
            Output.write(message: read value)
        }
        None => {
            Output.write(message: read "third-none")
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
    Output.write(message: read Arguments.get_or_default(args: read args, index: 1, default: read "fallback"))
    Output.write(message: read Arguments.get_or_default(args: read args, index: 99, default: read "fallback"))
    Output.write(message: read Arguments.get_or_default(args: read args, index: 0 - 1, default: read "negative-fallback"))
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
