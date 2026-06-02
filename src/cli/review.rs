use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rsscript::{
    Diagnostic, ReviewMap, analyze_source, analyze_sources_with_interfaces,
    format_diagnostics_human, format_diagnostics_json, format_review_human, format_review_json,
    format_review_map_human, format_review_map_json, review_map_sources, review_package_dir,
    review_sources,
};

use super::{is_package_directory, print_diagnostics, print_usage};

pub(crate) fn run_review(args: &[String]) -> ExitCode {
    let command = match parse_review_args(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            return ExitCode::from(2);
        }
    };
    match command {
        ReviewCommand::Diff {
            json,
            old_path,
            new_path,
        } => run_review_diff(json, old_path, new_path),
        ReviewCommand::Map { json, path } => run_review_map(json, path),
    }
}

#[derive(Debug)]
pub(crate) enum ReviewCommand<'a> {
    Diff {
        json: bool,
        old_path: &'a str,
        new_path: &'a str,
    },
    Map {
        json: bool,
        path: &'a str,
    },
}

fn parse_review_args(args: &[String]) -> Result<ReviewCommand<'_>, String> {
    let mut json = false;
    let mut command = None;
    let mut paths = Vec::new();

    for arg in args {
        if arg == "--json" {
            json = true;
        } else if arg == "--diff" || arg == "--map" {
            if command.is_some() {
                return Err(format!("unexpected review command `{arg}`."));
            }
            command = Some(arg.as_str());
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else {
            paths.push(arg.as_str());
        }
    }

    match (command, paths.as_slice()) {
        (Some("--map"), [path]) => Ok(ReviewCommand::Map { json, path }),
        (Some("--diff") | None, [old_path, new_path]) => Ok(ReviewCommand::Diff {
            json,
            old_path,
            new_path,
        }),
        _ => Err("invalid review arguments.".to_string()),
    }
}

fn run_review_diff(json: bool, old_path: &str, new_path: &str) -> ExitCode {
    let old_source = match fs::read_to_string(old_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {old_path}: {error}");
            return ExitCode::from(2);
        }
    };
    let new_source = match fs::read_to_string(new_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {new_path}: {error}");
            return ExitCode::from(2);
        }
    };

    let old_diagnostics = analyze_source(old_path, &old_source);
    let new_diagnostics = analyze_source(new_path, &new_source);
    let has_errors = old_diagnostics
        .iter()
        .chain(new_diagnostics.iter())
        .any(|diagnostic| diagnostic.severity.is_error());
    if has_errors {
        if json {
            let mut diagnostics = old_diagnostics;
            diagnostics.extend(new_diagnostics);
            println!("{}", format_diagnostics_json(&diagnostics));
        } else {
            print!("{}", format_diagnostics_human(&old_diagnostics));
            print!("{}", format_diagnostics_human(&new_diagnostics));
        }
        return ExitCode::from(1);
    }

    let findings = review_sources(old_path, &old_source, new_path, &new_source);
    if json {
        println!("{}", format_review_json(&findings));
    } else {
        print!("{}", format_review_human(&findings));
    }
    ExitCode::SUCCESS
}

fn run_review_map(json: bool, path: &str) -> ExitCode {
    let (map, diagnostics) = match review_map_for_path(path) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        print_diagnostics(json, &diagnostics);
        return ExitCode::from(1);
    }

    if json {
        println!("{}", format_review_map_json(&map));
    } else {
        print!("{}", format_review_map_human(&map));
    }
    ExitCode::SUCCESS
}

fn review_map_for_path(path: &str) -> Result<(ReviewMap, Vec<Diagnostic>), String> {
    if is_package_directory(path) {
        let review = review_package_dir(Path::new(path))?;
        return Ok((review.review_map, review.diagnostics));
    }

    let sources = match read_review_map_sources(path) {
        Ok(sources) => sources,
        Err(error) => return Err(error),
    };
    let source_refs = sources
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let diagnostics = analyze_sources_with_interfaces(source_refs.as_slice(), &[]);
    let map = review_map_sources(source_refs);
    Ok((map, diagnostics))
}
struct ReviewMapSource {
    path: String,
    contents: String,
}

fn read_review_map_sources(path: &str) -> Result<Vec<ReviewMapSource>, String> {
    let path = Path::new(path);
    if path.is_file() {
        return read_review_map_file(path).map(|source| vec![source]);
    }
    if !path.is_dir() {
        return Err(format!(
            "review map path is not a file or directory: {}",
            path.display()
        ));
    }

    let mut files = Vec::new();
    collect_rsscript_files(path, &mut files)?;
    files.sort();
    files
        .into_iter()
        .map(|file| read_review_map_file(&file))
        .collect()
}

fn collect_rsscript_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", directory.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_rsscript_files(&path, files)?;
        } else if is_rsscript_source_path(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_rsscript_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
}

fn read_review_map_file(path: &Path) -> Result<ReviewMapSource, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(ReviewMapSource {
        path: path.display().to_string(),
        contents,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn review_map_for_path_uses_package_review_environment() {
        let dep = unique_temp_dir("review-map-package-dep");
        fs::create_dir_all(dep.join("interface")).expect("dependency interface dir should create");
        fs::write(
            dep.join("rsspkg.toml"),
            r#"[package]
name = "rss-review-map-dep"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]
"#,
        )
        .expect("dependency manifest should write");
        fs::write(
            dep.join("interface/lib.rssi"),
            r#"features: native

native fn Dep.echo(message: read String) -> String
    effects(native)
"#,
        )
        .expect("dependency interface should write");

        let root = unique_temp_dir("review-map-package-root");
        fs::create_dir_all(root.join("interface")).expect("root interface dir should create");
        fs::create_dir_all(root.join("src")).expect("root source dir should create");
        fs::write(
            root.join("rsspkg.toml"),
            format!(
                r#"[package]
name = "rss-review-map-root"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[dependencies]
rss-review-map-dep = {{ path = "{}" }}
"#,
                toml_path(&dep)
            ),
        )
        .expect("root manifest should write");
        fs::write(
            root.join("interface/lib.rssi"),
            "pub fn Api.run(message: read String) -> String\n",
        )
        .expect("root interface should write");
        fs::write(
            root.join("src/main.rss"),
            r#"features: native

pub fn Api.run(message: read String) -> String {
    return Dep.echo(message: read message)
}
"#,
        )
        .expect("root source should write");

        let (map, diagnostics) = super::review_map_for_path(root.to_str().expect("utf-8 path"))
            .expect("package review map should load");
        fs::remove_dir_all(root).expect("root temp package should clean up");
        fs::remove_dir_all(dep).expect("dependency temp package should clean up");

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(map.summary.unknown.functions, 0);
        assert!(map.files.iter().any(|file| {
            file.regions.iter().any(|region| {
                region.function == "Api.run"
                    && region
                        .reasons
                        .iter()
                        .any(|reason| reason == "native call `Dep.echo`")
            })
        }));
    }

    #[test]
    fn parse_review_args_rejects_unknown_flags() {
        let values = args(&["--map", "--wat", "package"]);
        let error = super::parse_review_args(&values).expect_err("unknown flag should fail");

        assert_eq!(error, "unknown argument `--wat`.");
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("temp directory should create");
        path
    }

    fn toml_path(path: &std::path::Path) -> String {
        path.display().to_string().replace('\\', "\\\\")
    }
}
