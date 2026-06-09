use std::env;
use std::error::Error;
use std::time::Instant;

const DEFAULT_OBJECTS: usize = 6;

fn main() -> Result<(), Box<dyn Error>> {
    let endpoint = rss_s3_demo_native::default_endpoint();
    let objects = env::var("RSS_S3_DEMO_OBJECTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_OBJECTS);
    let body = rss_s3_demo_native::benchmark_payload("2026-05-31 sync benchmark payload");
    let started = Instant::now();

    for index in 0..objects {
        let key = format!("reports/sync-{index}.json");
        rss_s3_demo_native::put_object_blocking(&endpoint, "reports-prod", &key, &body)?;
    }

    let elapsed = started.elapsed();
    println!(
        "sync client uploaded {objects} objects x {} bytes in {} ms",
        body.len(),
        elapsed.as_millis()
    );
    Ok(())
}
