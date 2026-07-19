//! Process-isolated tests for JIT environment configuration.
#![cfg(feature = "native-jit")]

#[test]
fn rss_jit_osr_threshold_env_overrides_auto_trigger_fire_point() {
    let source = "\
fn hot(limit: Int, seed: Int) -> Int {
    Log.write(message: \"begin\")
    let mut i = 0
    let mut total = seed
    while i < limit {
        total = total + i * 3 - i / 2 + 7
        i = i + 1
    }
    Log.write(message: String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: String.from_int(value: hot(limit: 600, seed: 0)))
    return Unit
}
";

    let osr_entries = |threshold: Option<&str>| -> u64 {
        unsafe {
            std::env::remove_var("RSS_JIT_OSR");
            match threshold {
                Some(threshold) => std::env::set_var("RSS_JIT_OSR_THRESHOLD", threshold),
                None => std::env::remove_var("RSS_JIT_OSR_THRESHOLD"),
            }
        }
        let executable = rsscript::reg_vm_compile_source("rss-osr-threshold-probe.rss", source)
            .expect("threshold probe compiles");
        executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("native run with stats")
            .1
            .osr_entries
    };

    assert_eq!(osr_entries(None), 0);
    assert!(osr_entries(Some("100")) > 0);
}
