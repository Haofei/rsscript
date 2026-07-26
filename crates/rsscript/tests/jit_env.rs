//! Process-isolated tests for JIT environment configuration.
#![cfg(feature = "native-jit")]

#[test]
fn rss_jit_osr_threshold_is_deterministic_interpreted_work() {
    let heavy_source = "\
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
    Log.write(message: String.from_int(value: hot(limit: 8, seed: 0)))
    return Unit
}
";
    let tiny_source = "\
fn hot(limit: Int) -> Int {
    Log.write(message: \"begin\")
    let mut i = 0
    while i < limit {
        i = i + 1
    }
    Log.write(message: String.from_int(value: i))
    return i
}

fn main() -> Unit {
    Log.write(message: String.from_int(value: hot(limit: 14)))
    return Unit
}
";

    let run = |name: &str, source: &str, threshold: Option<&str>| {
        unsafe {
            std::env::remove_var("RSS_JIT_OSR");
            match threshold {
                Some(threshold) => std::env::set_var("RSS_JIT_OSR_THRESHOLD", threshold),
                None => std::env::remove_var("RSS_JIT_OSR_THRESHOLD"),
            }
        }
        let executable =
            rsscript::reg_vm_compile_source(name, source).expect("threshold probe compiles");
        executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("native run with stats")
    };

    let (reference, default_stats) = run("rss-osr-heavy-reference.rss", heavy_source, None);
    assert_eq!(default_stats.osr_entries, 0);

    let (tiered, heavy_stats) = run("rss-osr-heavy-tiered.rss", heavy_source, Some("100"));
    assert!(
        heavy_stats.osr_entries > 0,
        "the heavier body must reach 100 work units within eight backedges: {heavy_stats:?}"
    );
    assert_eq!(
        tiered, reference,
        "work-based OSR timing must not change program output"
    );

    let (_, tiny_stats) = run("rss-osr-tiny.rss", tiny_source, Some("100"));
    assert_eq!(
        tiny_stats.osr_entries, 0,
        "the tiny body must stay below 100 work units after more backedges: {tiny_stats:?}"
    );
}
