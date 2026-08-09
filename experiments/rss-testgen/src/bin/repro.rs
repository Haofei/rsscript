//! `rss-testgen <hex-seed>` — triage tool.
//!
//! Decodes a seed (hex, or raw bytes from stdin with `-`), prints the generated
//! source, then runs it on every in-process backend and prints each result —
//! turning a libFuzzer/proptest counterexample seed into a readable repro.

use std::io::Read;

use rss_testgen::backends::all_inprocess_backends;
use rss_testgen::generate;

fn main() {
    let arg = std::env::args().nth(1);
    let seed = match arg.as_deref() {
        None => {
            eprintln!("usage: rss-testgen <hex-seed> | rss-testgen - (raw bytes on stdin)");
            std::process::exit(2);
        }
        Some("-") => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf).expect("read stdin");
            buf
        }
        Some(hex) => decode_hex(hex),
    };

    let program = generate(&seed);
    println!("=== seed ({} bytes) ===", seed.len());
    println!("=== generated source ===\n{}", program.source);
    println!("=== backends ===");
    for backend in all_inprocess_backends() {
        match backend.run_stdout("repro.rss", &program.source, &[]) {
            Ok(stdout) => println!("[{}] OK\n{}", backend.name(), indent(&stdout)),
            Err(error) => println!("[{}] ERR: {error}", backend.name()),
        }
    }
}

fn decode_hex(hex: &str) -> Vec<u8> {
    let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if !clean.len().is_multiple_of(2) {
        eprintln!("hex seed must have an even number of digits");
        std::process::exit(2);
    }
    (0..clean.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&clean[i..i + 2], 16).unwrap_or_else(|_| {
                eprintln!("invalid hex");
                std::process::exit(2);
            })
        })
        .collect()
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
