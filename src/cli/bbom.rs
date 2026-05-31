use std::fs;
use std::path::Path;
use std::process::ExitCode;

use rsscript::bbom::{
    MergePolicy, behavior_bom_sources, capability_delta, evaluate_policy, format_bbom_human,
    format_bbom_json, format_capability_delta_human, format_capability_delta_json,
    format_policy_result_human,
};

#[derive(Debug, Clone, Copy)]
enum BbomCommand {
    Delta,
    Policy,
}

impl BbomCommand {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "delta" => Some(Self::Delta),
            "policy" => Some(Self::Policy),
            _ => None,
        }
    }
}

pub(crate) fn run_bbom(args: &[String]) -> ExitCode {
    if let Some(command) = args.first().and_then(|arg| BbomCommand::parse(arg)) {
        return match command {
            BbomCommand::Delta => run_bbom_delta(&args[1..]),
            BbomCommand::Policy => run_bbom_policy(&args[1..]),
        };
    }

    let mut json = false;
    let mut path = None;

    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            _ if !arg.starts_with('-') && path.is_none() => path = Some(arg.as_str()),
            _ => {
                eprintln!("unexpected argument: {arg}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(path) = path else {
        eprintln!("usage: rss bbom [--json] <file.rss | directory>");
        eprintln!("       rss bbom delta [--json] --from <old.rss> --to <new.rss>");
        eprintln!("       rss bbom policy [--json] --from <old.rss> --to <new.rss>");
        return ExitCode::from(2);
    };

    let sources = if Path::new(path).is_dir() {
        match read_bbom_dir_sources(path) {
            Ok(sources) => sources,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
        }
    } else {
        match fs::read_to_string(path) {
            Ok(source) => vec![(path.to_string(), source)],
            Err(error) => {
                eprintln!("failed to read {path}: {error}");
                return ExitCode::from(2);
            }
        }
    };

    let source_refs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(f, s)| (f.as_str(), s.as_str()))
        .collect();
    let bom = behavior_bom_sources(source_refs);

    if json {
        println!("{}", format_bbom_json(&bom));
    } else {
        print!("{}", format_bbom_human(&bom));
    }

    ExitCode::SUCCESS
}

fn run_bbom_delta(args: &[String]) -> ExitCode {
    let mut json = false;
    let mut from_path = None;
    let mut to_path = None;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--from" => from_path = iter.next().map(String::as_str),
            "--to" => to_path = iter.next().map(String::as_str),
            _ => {
                eprintln!("unexpected argument: {arg}");
                return ExitCode::from(2);
            }
        }
    }

    let (Some(from_path), Some(to_path)) = (from_path, to_path) else {
        eprintln!("usage: rss bbom delta [--json] --from <old.rss> --to <new.rss>");
        return ExitCode::from(2);
    };

    let old_source = match fs::read_to_string(from_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read {from_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let new_source = match fs::read_to_string(to_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read {to_path}: {e}");
            return ExitCode::from(2);
        }
    };

    let old_bom = behavior_bom_sources(vec![(from_path, &old_source)]);
    let new_bom = behavior_bom_sources(vec![(to_path, &new_source)]);
    let delta = capability_delta(&old_bom, &new_bom);

    if json {
        println!("{}", format_capability_delta_json(&delta));
    } else {
        print!("{}", format_capability_delta_human(&delta));
    }

    ExitCode::SUCCESS
}

fn run_bbom_policy(args: &[String]) -> ExitCode {
    let mut json = false;
    let mut from_path = None;
    let mut to_path = None;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--from" => from_path = iter.next().map(String::as_str),
            "--to" => to_path = iter.next().map(String::as_str),
            _ => {
                eprintln!("unexpected argument: {arg}");
                return ExitCode::from(2);
            }
        }
    }

    let (Some(from_path), Some(to_path)) = (from_path, to_path) else {
        eprintln!("usage: rss bbom policy [--json] --from <old.rss> --to <new.rss>");
        return ExitCode::from(2);
    };

    let old_source = match fs::read_to_string(from_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read {from_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let new_source = match fs::read_to_string(to_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read {to_path}: {e}");
            return ExitCode::from(2);
        }
    };

    let old_bom = behavior_bom_sources(vec![(from_path, &old_source)]);
    let new_bom = behavior_bom_sources(vec![(to_path, &new_source)]);
    let delta = capability_delta(&old_bom, &new_bom);
    let policy = MergePolicy::default();
    let result = evaluate_policy(&policy, &delta, &new_bom);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result)
                .expect("policy result JSON serialization should not fail")
        );
    } else {
        print!("{}", format_policy_result_human(&result));
    }

    if result.pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn read_bbom_dir_sources(dir: &str) -> Result<Vec<(String, String)>, String> {
    let mut sources = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("failed to read directory {dir}: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("directory entry error: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("rss") {
            let content = fs::read_to_string(&path)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            sources.push((path.to_string_lossy().to_string(), content));
        }
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(sources)
}
