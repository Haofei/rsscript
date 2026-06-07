//! Hand-written Rust baseline for the sqlx + SQLite benchmark. Runs the exact
//! same workload as `benchmarks/sqlx-sqlite/src/main.rss` (setup of `rows` rows +
//! `limit` pooled `query_strings` calls), calling `rss_sqlx_native` directly with
//! no RSS layer in between, and prints a result line in the same format as
//! `rss bench`.
//!
//! Usage: native_baseline [limit] [rows] [iterations] [warmup]

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let limit: i64 = args.get(1).and_then(|value| value.parse().ok()).unwrap_or(500);
    let rows: i64 = args.get(2).and_then(|value| value.parse().ok()).unwrap_or(200);
    let iterations: usize = args.get(3).and_then(|value| value.parse().ok()).unwrap_or(3);
    let warmup: usize = args.get(4).and_then(|value| value.parse().ok()).unwrap_or(1);

    let path = std::env::temp_dir().join("rss-sqlx-bench.db");
    let url = format!("sqlite://{}?mode=rwc", path.display());

    let run = || {
        rss_sqlx_native::execute(&url, "drop table if exists bench_item")
            .expect("drop table");
        rss_sqlx_native::execute(&url, "create table bench_item(name text)")
            .expect("create table");
        for _ in 0..rows {
            rss_sqlx_native::execute(&url, "insert into bench_item values ('x')")
                .expect("insert");
        }
        let mut total = 0i64;
        for _ in 0..limit {
            let names = rss_sqlx_native::query_strings(&url, "select name from bench_item")
                .expect("query");
            total += names.len() as i64;
        }
        total
    };

    for _ in 0..warmup {
        run();
    }

    let mut measurements = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let _ = run();
        measurements.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let min = measurements.iter().copied().fold(f64::INFINITY, f64::min);
    let max = measurements.iter().copied().fold(0.0_f64, f64::max);
    let mean = measurements.iter().sum::<f64>() / measurements.len() as f64;
    println!(
        "bench sqlx-sqlite mode=native-rust limit={limit} rows={rows} iterations={iterations} warmup={warmup} min_ms={min:.3} mean_ms={mean:.3} max_ms={max:.3}"
    );
}
