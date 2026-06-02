use std::path::Path;
use std::process::ExitCode;

use rsscript::{
    check_package_dir, diff_package_dirs, format_diagnostics_human, format_package_check_human,
    format_package_check_json, format_package_diff_human, format_package_diff_json,
    format_package_publish_human, format_package_publish_json, format_package_review_human,
    format_package_review_json, publish_package_dry_run_with_registry, review_package_dir,
};

use super::{print_usage, required_flag_value};

pub(crate) fn run_package(args: &[String]) -> ExitCode {
    let command = match parse_package_args(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            return ExitCode::from(2);
        }
    };
    match command {
        PackageCommand::Ci { json, path } => run_package_check(json, path),
        PackageCommand::Check { json, path } => run_package_check(json, path),
        PackageCommand::Review { json, path } => run_package_review(json, path),
        PackageCommand::Diff {
            json,
            old_path,
            new_path,
        } => run_package_diff(json, old_path, new_path),
        PackageCommand::Publish {
            json,
            dry_run,
            path,
            registry,
        } => run_package_publish(json, dry_run, path, registry),
    }
}

#[derive(Debug)]
pub(crate) enum PackageCommand<'a> {
    Check {
        json: bool,
        path: &'a str,
    },
    Review {
        json: bool,
        path: &'a str,
    },
    Diff {
        json: bool,
        old_path: &'a str,
        new_path: &'a str,
    },
    Ci {
        json: bool,
        path: &'a str,
    },
    Publish {
        json: bool,
        dry_run: bool,
        path: &'a str,
        registry: Option<&'a str>,
    },
}

fn parse_package_args(args: &[String]) -> Result<PackageCommand<'_>, String> {
    let mut json = false;
    let mut dry_run = false;
    let mut registry_path = None;
    let mut words = Vec::new();
    let mut paths = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--registry" {
            index += 1;
            registry_path = Some(required_flag_value(args, index, "--registry")?);
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if matches!(arg.as_str(), "review" | "diff" | "ci" | "publish") {
            words.push(arg.as_str());
        } else {
            paths.push(arg.as_str());
        }
        index += 1;
    }

    if dry_run && !matches!(words.as_slice(), ["publish"]) {
        return Err("`--dry-run` is only supported for `rss pkg publish`.".to_owned());
    }
    if registry_path.is_some() && !matches!(words.as_slice(), ["publish"]) {
        return Err("`--registry` is only supported for `rss pkg publish`.".to_owned());
    }
    if matches!(words.as_slice(), ["publish"]) && !dry_run {
        return Err("`rss pkg publish` currently requires `--dry-run`.".to_owned());
    }

    match (words.as_slice(), paths.as_slice()) {
        ([], []) => Ok(PackageCommand::Check { json, path: "." }),
        ([], [path]) => Ok(PackageCommand::Check { json, path }),
        (["review"], []) => Ok(PackageCommand::Review { json, path: "." }),
        (["review"], [path]) => Ok(PackageCommand::Review { json, path }),
        (["diff"], [old_path, new_path]) => Ok(PackageCommand::Diff {
            json,
            old_path,
            new_path,
        }),
        (["ci"], []) => Ok(PackageCommand::Ci { json, path: "." }),
        (["ci"], [path]) => Ok(PackageCommand::Ci { json, path }),
        (["publish"], []) => Ok(PackageCommand::Publish {
            json,
            dry_run,
            path: ".",
            registry: registry_path,
        }),
        (["publish"], [path]) => Ok(PackageCommand::Publish {
            json,
            dry_run,
            path,
            registry: registry_path,
        }),
        _ => Err("invalid package arguments.".to_string()),
    }
}

pub(crate) fn run_package_check(json: bool, path: &str) -> ExitCode {
    let check = match check_package_dir(Path::new(path)) {
        Ok(check) => check,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_package_check_json(&check));
    } else {
        print!("{}", format_package_check_human(&check));
        if !check.diagnostics.is_empty() {
            print!("{}", format_diagnostics_human(&check.diagnostics));
        }
    }

    if check.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_package_review(json: bool, path: &str) -> ExitCode {
    let review = match review_package_dir(Path::new(path)) {
        Ok(review) => review,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_package_review_json(&review));
    } else {
        print!("{}", format_package_review_human(&review));
        if !review.diagnostics.is_empty() {
            print!("{}", format_diagnostics_human(&review.diagnostics));
        }
    }

    if review
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_package_diff(json: bool, old_path: &str, new_path: &str) -> ExitCode {
    let diff = match diff_package_dirs(Path::new(old_path), Path::new(new_path)) {
        Ok(diff) => diff,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_package_diff_json(&diff));
    } else {
        print!("{}", format_package_diff_human(&diff));
    }
    ExitCode::SUCCESS
}

fn run_package_publish(json: bool, dry_run: bool, path: &str, registry: Option<&str>) -> ExitCode {
    if !dry_run {
        eprintln!("rss pkg publish currently requires --dry-run");
        return ExitCode::from(2);
    }
    let registry_path = registry.map(Path::new);
    let publish = match publish_package_dry_run_with_registry(Path::new(path), registry_path) {
        Ok(publish) => publish,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_package_publish_json(&publish));
    } else {
        print!("{}", format_package_publish_human(&publish));
    }

    if publish.ready {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_package_args_defaults_to_current_package_check() {
        let command = super::parse_package_args(&[]).expect("empty pkg command should parse");

        match command {
            super::PackageCommand::Check { json, path } => {
                assert!(!json);
                assert_eq!(path, ".");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_path_as_check_target() {
        let values = args(&["examples/code-agent"]);
        let command = super::parse_package_args(&values).expect("path check should parse");

        match command {
            super::PackageCommand::Check { json, path } => {
                assert!(!json);
                assert_eq!(path, "examples/code-agent");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_review_diff_ci_and_publish() {
        let values = args(&["review", "--json", "package"]);
        let command = super::parse_package_args(&values).expect("review should parse");
        match command {
            super::PackageCommand::Review { json, path } => {
                assert!(json);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }

        let values = args(&["diff", "--json", "old", "new"]);
        let command = super::parse_package_args(&values).expect("diff should parse");
        match command {
            super::PackageCommand::Diff {
                json,
                old_path,
                new_path,
            } => {
                assert!(json);
                assert_eq!(old_path, "old");
                assert_eq!(new_path, "new");
            }
            other => panic!("unexpected package command: {other:?}"),
        }

        let values = args(&["ci", "--json", "package"]);
        let command = super::parse_package_args(&values).expect("ci should parse");
        match command {
            super::PackageCommand::Ci { json, path } => {
                assert!(json);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }

        let values = args(&["publish", "--dry-run", "--registry", "registry", "package"]);
        let command = super::parse_package_args(&values).expect("publish should parse");
        match command {
            super::PackageCommand::Publish {
                json,
                dry_run,
                path,
                registry,
            } => {
                assert!(!json);
                assert!(dry_run);
                assert_eq!(path, "package");
                assert_eq!(registry, Some("registry"));
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_rejects_invalid_flags() {
        let values = args(&["publish", "--wat", "package"]);
        let error = super::parse_package_args(&values).expect_err("unknown flag should fail");
        assert_eq!(error, "unknown argument `--wat`.");

        let values = args(&["review", "--dry-run"]);
        let error = super::parse_package_args(&values).expect_err("dry-run should fail");
        assert_eq!(
            error,
            "`--dry-run` is only supported for `rss pkg publish`."
        );

        let values = args(&["review", "--registry", "registry"]);
        let error = super::parse_package_args(&values).expect_err("registry should fail");
        assert_eq!(
            error,
            "`--registry` is only supported for `rss pkg publish`."
        );

        let values = args(&["publish", "package"]);
        let error = super::parse_package_args(&values).expect_err("publish should require dry-run");
        assert_eq!(error, "`rss pkg publish` currently requires `--dry-run`.");
    }
}
