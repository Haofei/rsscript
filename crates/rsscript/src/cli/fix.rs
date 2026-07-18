use std::fs;
use std::process::ExitCode;

use rsscript::{
    Diagnostic, FixEdit, analyze_source_with_interfaces, default_read_migration_edits,
    standard_package_interfaces,
};
use serde_json::json;

use super::{print_usage, read_interface_sources, required_flag_value};

#[derive(Debug)]
struct FixOptions<'a> {
    json: bool,
    write: bool,
    default_read: bool,
    path: Option<&'a str>,
    interfaces: Vec<&'a str>,
}

fn parse_fix_args(args: &[String]) -> Result<FixOptions<'_>, String> {
    let mut json = false;
    let mut write = false;
    let mut default_read = false;
    let mut path = None;
    let mut interfaces = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--json" => json = true,
            "--write" => write = true,
            "--default-read" => default_read = true,
            "--interface" => {
                index += 1;
                interfaces.push(required_flag_value(args, index, "--interface")?);
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument `{other}`."));
            }
            other if path.is_none() => path = Some(other),
            other => return Err(format!("unexpected extra path `{other}`.")),
        }
        index += 1;
    }
    Ok(FixOptions {
        json,
        write,
        default_read,
        path,
        interfaces,
    })
}

/// A concrete edit ready to apply, paired with the diagnostic code it resolves.
struct PlannedEdit {
    code: String,
    title: String,
    edit: FixEdit,
}

fn plan_edits(diagnostics: &[Diagnostic]) -> Vec<PlannedEdit> {
    let mut planned = Vec::new();
    for diagnostic in diagnostics {
        for fix in &diagnostic.fixes {
            if fix.applicability != "machine-applicable" {
                continue;
            }
            let Some(edit) = &fix.edit else { continue };
            planned.push(PlannedEdit {
                code: diagnostic.code.clone(),
                title: fix.title.clone(),
                edit: edit.clone(),
            });
        }
    }
    planned
}

/// Apply edits to `source`, returning the rewritten text and the edits actually
/// applied. Edits are applied bottom-to-top (latest position first) so earlier
/// edits never shift the positions of later ones. Overlapping edits on the same
/// line are conservatively skipped (the first one in source order wins).
fn apply_edits(source: &str, edits: &[PlannedEdit]) -> (String, Vec<usize>, Vec<usize>) {
    // Index edits, then order by (line, column) descending.
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by(|&a, &b| {
        let (ea, eb) = (&edits[a].edit.span, &edits[b].edit.span);
        eb.line.cmp(&ea.line).then(eb.column.cmp(&ea.column))
    });

    let mut lines: Vec<Vec<char>> = source.lines().map(|line| line.chars().collect()).collect();
    let trailing_newline = source.ends_with('\n');

    // Track applied character ranges per line to reject overlaps.
    let mut claimed: Vec<(usize, usize, usize)> = Vec::new(); // (line, start, end)
    let mut applied = Vec::new();
    let mut skipped = Vec::new();

    for &i in &order {
        let span = &edits[i].edit.span;
        if span.line == 0 || span.line > lines.len() {
            skipped.push(i);
            continue;
        }
        let line = &mut lines[span.line - 1];
        let start = span.column.saturating_sub(1);
        if start > line.len() {
            skipped.push(i);
            continue;
        }
        let end = (start + span.length).min(line.len());
        let overlaps = claimed.iter().any(|&(l, s, e)| {
            if l != span.line {
                return false;
            }
            if start == end {
                // Insertion: only conflicts if strictly inside a replaced range.
                start > s && start < e
            } else {
                // Replacement: conflicts if the character ranges intersect.
                start < e && end > s
            }
        });
        if overlaps {
            skipped.push(i);
            continue;
        }
        let replacement: Vec<char> = edits[i].edit.replacement.chars().collect();
        line.splice(start..end, replacement);
        claimed.push((span.line, start, end));
        applied.push(i);
    }

    let mut rebuilt = lines
        .into_iter()
        .map(|line| line.into_iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    if trailing_newline {
        rebuilt.push('\n');
    }
    (rebuilt, applied, skipped)
}

fn analyze(path: &str, source: &str, interfaces: &[(&str, &str)]) -> Vec<Diagnostic> {
    let mut combined = standard_package_interfaces().to_vec();
    combined.extend(interfaces.iter().copied());
    analyze_source_with_interfaces(path, source, &combined)
}

pub(crate) fn run_fix(args: &[String]) -> ExitCode {
    let options = match parse_fix_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let Some(path) = options.path else {
        print_usage();
        return ExitCode::from(2);
    };
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {path}: {error}");
            return ExitCode::from(2);
        }
    };
    let interfaces = match read_interface_sources(&options.interfaces) {
        Ok(interfaces) => interfaces,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let interface_refs: Vec<(&str, &str)> = interfaces
        .iter()
        .map(|interface| (interface.path.as_str(), interface.contents.as_str()))
        .collect();

    let diagnostics = analyze(path, &source, &interface_refs);
    let mut planned = plan_edits(&diagnostics);
    if options.default_read {
        planned.extend(
            default_read_migration_edits(path, &source, &interface_refs)
                .into_iter()
                .map(|edit| PlannedEdit {
                    code: "default-read".to_string(),
                    title: "apply default-read simplification".to_string(),
                    edit,
                }),
        );
    }
    let (rewritten, applied, skipped) = apply_edits(&source, &planned);

    if options.write
        && !applied.is_empty()
        && let Err(error) = fs::write(path, &rewritten)
    {
        eprintln!("failed to write {path}: {error}");
        return ExitCode::from(2);
    }

    if options.json {
        let applied_diagnostic_fixes = applied
            .iter()
            .filter(|&&index| planned[index].code != "default-read")
            .count();
        let fixes: Vec<_> = applied
            .iter()
            .map(|&i| {
                let edit = &planned[i];
                json!({
                    "code": edit.code,
                    "title": edit.title,
                    "line": edit.edit.span.line,
                    "column": edit.edit.span.column,
                    "replacement": edit.edit.replacement,
                })
            })
            .collect();
        println!(
            "{}",
            json!({
                "ok": true,
                "path": path,
                "written": options.write && !applied.is_empty(),
                "applied": fixes,
                "skipped": skipped.len(),
                "remaining_diagnostics": diagnostics.len().saturating_sub(applied_diagnostic_fixes),
            })
        );
        return ExitCode::SUCCESS;
    }

    if planned.is_empty() {
        println!("{path}: no machine-applicable fixes");
        return ExitCode::SUCCESS;
    }
    for &i in &applied {
        let edit = &planned[i];
        println!(
            "  {}:{}:{}  [{}] {}",
            path, edit.edit.span.line, edit.edit.span.column, edit.code, edit.title
        );
    }
    if options.write {
        println!("applied {} fix(es) to {path}", applied.len());
    } else {
        println!(
            "{} fix(es) applicable; re-run with `--write` to apply",
            applied.len()
        );
    }
    if !skipped.is_empty() {
        println!(
            "{} overlapping fix(es) skipped (re-run after applying)",
            skipped.len()
        );
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fix_args_accepts_default_read_migration() {
        let args = vec![
            "--default-read".to_string(),
            "--write".to_string(),
            "example.rss".to_string(),
        ];
        let options = parse_fix_args(&args).expect("default-read migration should parse");

        assert!(options.default_read);
        assert!(options.write);
        assert_eq!(options.path, Some("example.rss"));
    }
}
