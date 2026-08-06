// ---------------------------------------------------------------------------
// AST-dump PARITY — the rss producer (`selfhost/astdump.rss`) vs the oracle.
//
// Step 2: the rss recursive-descent parser streams the canonical AST dump; the
// harness compares it byte-for-byte against `ast_oracle_dump`. The curated
// `samples/ast/*.rss` set is a non-ignored fast gate, while
// `ast_parity_corpus` is the full equality gate over the discovered corpus.
// ---------------------------------------------------------------------------

fn compile_astdump() -> Result<RegVmExecutable, String> {
    compile_selfhost_tool("astdump.rss", "astdump")
}

/// Run the precompiled rss AST-dump producer; its stdout IS the dump.
///
/// The producer always emits the RICHEST span suffix (` @line:col:len`) on every
/// node head (mirroring the lexer producer, which always emits `line:col:len`).
/// Here we project each line down to the active AST tier so the byte-exact
/// comparison against the (tier-gated) oracle holds: tier 0 drops the suffix
/// entirely, tier 1 keeps ` @line:col`, tier 2 keeps ` @line:col:len`. Lines
/// without a numeric ` @L:C:N` suffix (synthetic labels) are passed through.
fn run_astdump(exe: &RegVmExecutable, source: &str) -> Result<String, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss astdump failed to run: {e:?}"))?;
    let t = ast_tier();
    if t == 2 {
        return Ok(output.stdout);
    }
    let mut out = String::with_capacity(output.stdout.len());
    for line in output.stdout.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        out.push_str(&project_ast_line(body, t));
        out.push_str(nl);
    }
    Ok(out)
}

/// Project a producer line's ` @line:col:len` suffix down to `tier` (1 → keep
/// `line:col`, 0 → drop the suffix). Only a trailing, strictly-numeric
/// ` @d+:d+:d+` is treated as a span (so a payload containing `@` is untouched).
fn project_ast_line(line: &str, tier: u8) -> String {
    if let Some(at) = line.rfind(" @") {
        let suffix = &line[at + 2..];
        let parts: Vec<&str> = suffix.split(':').collect();
        if parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        {
            let head = &line[..at];
            return match tier {
                0 => head.to_string(),
                _ => format!("{head} @{}:{}", parts[0], parts[1]),
            };
        }
    }
    line.to_string()
}

/// Curated AST-parity samples under `selfhost/samples/ast/`, sorted for stable order.
fn ast_sample_files() -> Vec<PathBuf> {
    let dir = selfhost_dir().join("samples/ast");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rss"))
        .collect();
    files.sort();
    files
}

/// Step-2 gate (non-ignored): the rss producer matches the oracle byte-for-byte
/// on the tiny sample — the end-to-end proof that the streaming producer, the
/// dump format, and the oracle all agree.
#[test]
fn ast_parity_tiny_sample() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let oracle = ast_oracle_dump("samples/tiny.rss", &source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, &source).expect("rss astdump should run");
    assert_eq!(
        actual, oracle,
        "AST parity mismatch on tiny.rss\n--- oracle ---\n{oracle}\n--- rss ---\n{actual}"
    );
}

#[test]
fn astdump_shared_module_and_use_items_match_oracle() {
    let source =
        "module demo.core\nuse demo.util.*\nuse demo.io as io\nfn run() -> Unit { return Unit }\n";
    let oracle = ast_oracle_dump("shared-items.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(
        actual, oracle,
        "AST parity mismatch while rendering shared module/use items\n--- oracle ---\n{oracle}\n--- rss ---\n{actual}"
    );
}

/// Step-2 gate (non-ignored): the rss producer matches the oracle byte-for-byte
/// on every curated sample. This is the fast inner-loop gate; keep the samples
/// within the producer's supported core so it stays green as coverage grows.
#[test]
fn ast_parity_samples() {
    let exe = compile_astdump().expect("rss astdump should compile");
    let mut mismatches: Vec<String> = Vec::new();
    let files = ast_sample_files();
    // Guard against a vacuous pass: a missing/unreadable samples dir would make
    // `files` empty and this test green while covering nothing.
    assert!(
        files.len() >= AST_SAMPLE_MIN,
        "expected >= {AST_SAMPLE_MIN} curated AST samples under selfhost/samples/ast/, found {} \
         (missing or unreadable dir makes this test pass vacuously)",
        files.len()
    );
    for file in &files {
        let rel = file
            .strip_prefix(selfhost_dir())
            .unwrap_or(file)
            .display()
            .to_string();
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        let oracle = ast_oracle_dump(&rel, &source);
        match run_astdump(&exe, &source) {
            Err(e) => mismatches.push(format!("{rel}: run error: {e}")),
            Ok(actual) => {
                if actual != oracle {
                    mismatches.push(format!(
                        "{rel}: mismatch\n--- oracle ---\n{oracle}\n--- rss ---\n{actual}"
                    ));
                }
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "AST parity failed on {} of {} samples:\n{}",
        mismatches.len(),
        files.len(),
        mismatches.join("\n\n")
    );
}

/// Minimum number of curated AST samples that must exist (guards `ast_parity_samples`
/// against a vacuous pass if the samples dir goes missing).
const AST_SAMPLE_MIN: usize = 6;

// RegVmExecutable contains Rc state. Concurrently compiling several large
// producers has proved less reliable than one reusable producer, while a single
// worker makes the full gate's timing and failure location deterministic.
const AST_CORPUS_WORKERS: usize = 1;

/// Step-2 corpus gate (ignored by default): the rss AST producer reproduces the
/// Rust AST oracle byte-for-byte for every discovered `.rss` file.
///
/// RUNTIME: each worker compiles one private rss producer (the executable uses
/// non-thread-safe Rc state) and reuses it over a size-descending share of the
/// corpus. Giant inputs start first to avoid a long single-worker tail. A debug
/// build is still slow; run the gate in release:
/// `cargo test -p rsscript-engine --release --lib selfhost_parity::ast_parity_corpus -- --ignored --nocapture`.
/// The fast inner-loop gate is `ast_parity_samples` (non-ignored, curated subset).
#[test]
#[ignore]
fn ast_parity_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
    // Start expensive inputs first so a giant self-hosted tool does not become
    // the sole straggler after every other worker has drained the small files.
    // Paths break equal-size ties to keep scheduling deterministic.
    let mut sized_files = files
        .into_iter()
        .map(|path| {
            let len = std::fs::metadata(&path)
                .unwrap_or_else(|e| panic!("cannot stat {}: {e}", path.display()))
                .len();
            (len, path)
        })
        .collect::<Vec<_>>();
    sized_files.sort_by(|(left_len, left), (right_len, right)| {
        right_len.cmp(left_len).then_with(|| left.cmp(right))
    });
    let files = sized_files
        .into_iter()
        .map(|(_, path)| path)
        .collect::<Vec<_>>();
    let total = files.len();
    let workers = AST_CORPUS_WORKERS.min(total.max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let completed = std::sync::atomic::AtomicUsize::new(0);
    type AstWorkerResult = (
        usize,
        Vec<String>,
        Vec<String>,
        std::time::Duration,
        Vec<(std::time::Duration, String)>,
    );
    let partials: Vec<AstWorkerResult> = std::thread::scope(|scope| {
        let handles: Vec<_> =
            (0..workers)
                .map(|worker| {
                    let (root, files, next, completed) = (&root, &files, &next, &completed);
                    scope.spawn(move || {
                    let worker_started = std::time::Instant::now();
                    let compile_started = std::time::Instant::now();
                    let exe = compile_astdump().expect("rss astdump should compile");
                    eprintln!(
                        "[ast] worker {worker}: producer compiled in {:?}",
                        compile_started.elapsed()
                    );
                    let mut ok = 0usize;
                    let mut run_failures = Vec::new();
                    let mut mismatches = Vec::new();
                    let mut slowest: Vec<(std::time::Duration, String)> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= files.len() {
                            break;
                        }
                        let file = &files[i];
                        let rel = file
                            .strip_prefix(root)
                            .unwrap_or(file)
                            .display()
                            .to_string();
                        let file_started = std::time::Instant::now();
                        let source = match std::fs::read_to_string(file) {
                            Ok(source) => source,
                            Err(e) => {
                                run_failures.push(format!("{rel}: unreadable: {e}"));
                                completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                continue;
                            }
                        };
                        if source.len() >= 40 * 1024 {
                            eprintln!(
                                "[ast] worker {worker}: starting large input {rel} ({} bytes)",
                                source.len()
                            );
                        }
                        let oracle_started = std::time::Instant::now();
                        let oracle = ast_oracle_dump(&rel, &source);
                        if source.len() >= 40 * 1024 {
                            eprintln!(
                                "[ast] worker {worker}: Rust oracle for {rel} in {:?} ({} lines, {} bytes)",
                                oracle_started.elapsed(),
                                oracle.lines().count(),
                                oracle.len(),
                            );
                        }
                        let producer_started = std::time::Instant::now();
                        match run_astdump(&exe, &source) {
                            Err(e) => run_failures.push(format!("{rel}: {e}")),
                            Ok(actual) if actual == oracle => ok += 1,
                            Ok(actual) if mismatches.len() < 10 => {
                                let first_diff = oracle
                                    .lines()
                                    .zip(actual.lines())
                                    .find(|(o, a)| o != a)
                                    .map(|(o, a)| format!("\n    oracle: {o:?}\n    rss:    {a:?}"))
                                    .unwrap_or_else(|| {
                                        format!(
                                            "\n    (line count: oracle {} vs rss {})",
                                            oracle.lines().count(),
                                            actual.lines().count()
                                        )
                                    });
                                mismatches.push(format!("{rel}{first_diff}"));
                            }
                            Ok(_) => {}
                        }
                        let elapsed = file_started.elapsed();
                        if source.len() >= 40 * 1024 {
                            eprintln!(
                                "[ast] worker {worker}: RSS producer for {rel} ({} bytes) in {:?}; total {:?}",
                                source.len(),
                                producer_started.elapsed(),
                                elapsed,
                            );
                        }
                        slowest.push((elapsed, rel));
                        slowest.sort_by(|(left, _), (right, _)| right.cmp(left));
                        slowest.truncate(5);
                        let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        if done % 64 == 0 || done == files.len() {
                            eprintln!("[ast] progress: {done}/{} files", files.len());
                        }
                    }
                    (ok, run_failures, mismatches, worker_started.elapsed(), slowest)
                })
                })
                .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let mut ok = 0usize;
    let mut run_failures = Vec::new();
    let mut sample_mismatches = Vec::new();
    let mut slowest = Vec::new();
    for (partial_ok, partial_failures, partial_mismatches, elapsed, worker_slowest) in partials {
        ok += partial_ok;
        run_failures.extend(partial_failures);
        sample_mismatches.extend(partial_mismatches);
        eprintln!("[ast] worker finished in {elapsed:?}");
        slowest.extend(worker_slowest);
    }
    run_failures.sort();
    sample_mismatches.sort();
    sample_mismatches.truncate(10);
    slowest.sort_by(|(left, _), (right, _)| right.cmp(left));
    slowest.truncate(10);
    eprintln!(
        "\n=== ast_parity_corpus ===\n  files: {total}\n  checked: {}\n  byte-exact: {ok}\n  \
         run-failures: {}\n",
        total - run_failures.len(),
        run_failures.len()
    );
    for failure in &run_failures {
        eprintln!("[run-fail] {failure}");
    }
    for rel in &sample_mismatches {
        eprintln!("[mismatch] {rel}");
    }
    for (elapsed, rel) in &slowest {
        eprintln!("[slow] {elapsed:?} {rel}");
    }
    // The producer must never crash on corpus input — unsupported constructs are
    // expected to mismatch (partial/`unknown-*` output), not error. A run-failure
    // is a real regression even if `ok` still clears the floor.
    assert_eq!(
        run_failures.len(),
        0,
        "rss AST producer had {} run-failures over {total} corpus files \
         (it must degrade to a mismatch, never crash)",
        run_failures.len()
    );
    assert_eq!(
        total - run_failures.len(),
        total,
        "some corpus files were not readable or were not checked"
    );
    assert_eq!(
        ok,
        total,
        "AST dump must match every corpus file; first mismatches:\n{}",
        sample_mismatches.join("\n\n")
    );
}
