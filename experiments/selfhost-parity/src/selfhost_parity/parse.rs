// Checker stdout parsing helpers, split from `checker.rs` for module-size
// partitioning. `include!`d into the selfhost_parity module like its siblings.

fn parse_checker_output(stdout: &str) -> Result<Vec<String>, String> {
    let mut codes = Vec::new();
    let mut clean_count = 0usize;
    for line in stdout.lines() {
        let code = line.trim();
        if code.is_empty() {
            continue;
        }
        if code == "CLEAN" {
            clean_count += 1;
        } else if is_target_code(code) {
            codes.push(code.to_string());
        } else {
            return Err(format!(
                "rss checker emitted an unknown diagnostic line: {line:?}"
            ));
        }
    }
    if clean_count > 1 {
        return Err("rss checker emitted duplicate CLEAN verdicts".to_string());
    }
    if clean_count == 1 && !codes.is_empty() {
        return Err("rss checker emitted CLEAN together with diagnostics".to_string());
    }
    if clean_count == 0 && codes.is_empty() {
        return Err("rss checker emitted no verdict".to_string());
    }
    codes.sort();
    codes.dedup();
    Ok(codes)
}

fn parse_checker_records(stdout: &str) -> Result<Vec<SelfhostDiagnosticRecord>, String> {
    let mut records = Vec::new();
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.as_slice() == ["CLEAN"] {
        return Ok(Vec::new());
    }
    if lines.contains(&"CLEAN") {
        return Err("rss checker emitted CLEAN together with structured diagnostics".to_string());
    }
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        let [code, line, column, length] = fields.as_slice() else {
            return Err(format!("malformed structured diagnostic: {line:?}"));
        };
        if !is_target_code(code) {
            return Err(format!("unknown structured diagnostic code: {code:?}"));
        }
        let parse_number = |name: &str, value: &str| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid diagnostic {name}: {value:?}"))
        };
        records.push(SelfhostDiagnosticRecord {
            code: (*code).to_string(),
            line: parse_number("line", line)?,
            column: parse_number("column", column)?,
            length: parse_number("length", length)?,
        });
    }
    if records.is_empty() {
        return Err("rss checker emitted no structured diagnostics".to_string());
    }
    records.sort();
    Ok(records)
}

type CheckerWorkerResponse = Result<String, String>;
type CheckerWorkerRequest = (String, std::sync::mpsc::Sender<CheckerWorkerResponse>);

struct CheckerWorkerPool {
    workers: Vec<std::sync::mpsc::Sender<CheckerWorkerRequest>>,
    next: std::sync::atomic::AtomicUsize,
}
