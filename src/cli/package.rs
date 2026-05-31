use std::fs;
use std::path::Path;
use std::process::ExitCode;

use rsscript::{
    check_package_dir, diff_package_dirs, diff_package_locks, format_diagnostics_human,
    format_package_check_human, format_package_check_json, format_package_check_reir_json,
    format_package_diff_human, format_package_diff_json, format_package_lock_diff_human,
    format_package_lock_diff_json, format_package_lock_diff_reir_json, format_package_lock_json,
    format_package_lock_reir_json_with_path, format_package_lock_toml,
    format_package_metadata_human, format_package_metadata_json, format_package_metadata_reir_json,
    format_package_publish_human, format_package_publish_json, format_package_publish_reir_json,
    format_package_review_human, format_package_review_json, format_package_review_reir_diff_json,
    format_package_review_reir_json, format_package_tree_human, format_package_tree_json,
    format_package_tree_reir_json, format_package_vendor_human, format_package_vendor_json,
    format_package_vendor_reir_json, lock_package_dir, package_metadata, package_metadata_verify,
    package_tree, publish_package_dry_run_with_registry, review_package_dir, vendor_package_dir,
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
        PackageCommand::Check { json, reir, path } => run_package_check(json, reir, path),
        PackageCommand::Review { json, reir, path } => run_package_review(json, reir, path),
        PackageCommand::ReviewUpdate {
            json,
            reir,
            old_lock_path,
            new_lock_path,
        } => run_package_review_update(json, reir, old_lock_path, new_lock_path),
        PackageCommand::Lock { json, reir, path } => run_package_lock(json, reir, path),
        PackageCommand::Tree { json, reir, path } => run_package_tree(json, reir, path),
        PackageCommand::Publish {
            json,
            reir,
            dry_run,
            path,
            registry,
        } => run_package_publish(json, reir, dry_run, path, registry),
        PackageCommand::Vendor {
            json,
            reir,
            dry_run,
            path,
        } => run_package_vendor(json, reir, dry_run, path),
        PackageCommand::Metadata {
            json,
            reir,
            dry_run,
            verify,
            path,
        } => run_package_metadata(json, reir, dry_run, verify, path),
        PackageCommand::Diff {
            json,
            reir,
            old_path,
            new_path,
        } => run_package_diff(json, reir, old_path, new_path),
        PackageCommand::ReirDiff {
            json,
            fail_on_change,
            baseline_path,
            current_path,
        } => run_package_reir_diff(json, fail_on_change, baseline_path, current_path),
    }
}
#[derive(Debug)]
pub(crate) enum PackageCommand<'a> {
    Check {
        json: bool,
        reir: bool,
        path: &'a str,
    },
    Review {
        json: bool,
        reir: bool,
        path: &'a str,
    },
    ReviewUpdate {
        json: bool,
        reir: bool,
        old_lock_path: &'a str,
        new_lock_path: &'a str,
    },
    Lock {
        json: bool,
        reir: bool,
        path: &'a str,
    },
    Tree {
        json: bool,
        reir: bool,
        path: &'a str,
    },
    Publish {
        json: bool,
        reir: bool,
        dry_run: bool,
        path: &'a str,
        registry: Option<&'a str>,
    },
    Vendor {
        json: bool,
        reir: bool,
        dry_run: bool,
        path: &'a str,
    },
    Metadata {
        json: bool,
        reir: bool,
        dry_run: bool,
        verify: bool,
        path: &'a str,
    },
    Diff {
        json: bool,
        reir: bool,
        old_path: &'a str,
        new_path: &'a str,
    },
    ReirDiff {
        json: bool,
        fail_on_change: bool,
        baseline_path: &'a str,
        current_path: &'a str,
    },
}

fn parse_package_args(args: &[String]) -> Result<PackageCommand<'_>, String> {
    let mut json = false;
    let mut reir = false;
    let mut dry_run = false;
    let mut verify = false;
    let mut fail_on_change = false;
    let mut words = Vec::new();
    let mut from_path = None;
    let mut registry_path = None;
    let mut to_path = None;
    let mut paths = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--reir" {
            reir = true;
        } else if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--verify" {
            verify = true;
        } else if arg == "--fail-on-change" {
            fail_on_change = true;
        } else if arg == "--from" {
            index += 1;
            from_path = Some(required_flag_value(args, index, "--from")?);
        } else if arg == "--registry" {
            index += 1;
            registry_path = Some(required_flag_value(args, index, "--registry")?);
        } else if arg == "--to" {
            index += 1;
            to_path = Some(required_flag_value(args, index, "--to")?);
        } else if matches!(
            arg.as_str(),
            "--native-abi" | "--update-plan" | "--deny-unknown" | "--deny-high-risk"
        ) {
            return Err(format!(
                "`{arg}` is a planned package-manager extension and is not supported by this build."
            ));
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if words.is_empty()
            && matches!(
                arg.as_str(),
                "init"
                    | "add"
                    | "remove"
                    | "update"
                    | "audit-surface"
                    | "semver-check"
                    | "compare"
                    | "explain"
                    | "why"
                    | "clean"
            )
        {
            return Err(format!(
                "`rss pkg {arg}` is a planned package-manager command and is not supported by this build."
            ));
        } else if matches!(
            arg.as_str(),
            "check"
                | "review"
                | "update"
                | "lock"
                | "tree"
                | "publish"
                | "vendor"
                | "metadata"
                | "reir"
                | "diff"
        ) {
            words.push(arg.as_str());
        } else {
            paths.push(arg.as_str());
        }
        index += 1;
    }

    if reir
        && !matches!(
            words.as_slice(),
            ["check"]
                | ["review"]
                | ["review", "update"]
                | ["diff"]
                | ["lock"]
                | ["tree"]
                | ["publish"]
                | ["metadata"]
                | ["vendor"]
        )
    {
        return Err(
            "`--reir` is only supported for `rss pkg check`, `rss pkg review`, `rss pkg diff`, `rss pkg lock`, `rss pkg tree`, `rss pkg publish`, `rss pkg vendor`, `rss pkg metadata`, and `rss pkg review update`."
                .to_owned(),
        );
    }
    if json && reir {
        return Err("`--json` and `--reir` select different output formats.".to_owned());
    }
    if dry_run && verify {
        return Err("`--dry-run` and `--verify` select different metadata modes.".to_owned());
    }
    if dry_run && !matches!(words.as_slice(), ["publish"] | ["vendor"] | ["metadata"]) {
        return Err(
            "`--dry-run` is only supported for `rss pkg publish`, `rss pkg vendor`, and `rss pkg metadata`."
                .to_owned(),
        );
    }
    if verify && !matches!(words.as_slice(), ["metadata"]) {
        return Err("`--verify` is only supported for `rss pkg metadata`.".to_owned());
    }
    if registry_path.is_some() && !matches!(words.as_slice(), ["publish"]) {
        return Err("`--registry` is only supported for `rss pkg publish`.".to_owned());
    }
    if matches!(words.as_slice(), ["publish"]) && !dry_run {
        return Err("`rss pkg publish` currently requires `--dry-run`.".to_owned());
    }
    if fail_on_change && !matches!(words.as_slice(), ["reir", "diff"]) {
        return Err("`--fail-on-change` is only supported for `rss pkg reir diff`.".to_owned());
    }

    match (words.as_slice(), paths.as_slice(), from_path, to_path) {
        (["check"], [], None, None) => Ok(PackageCommand::Check {
            json,
            reir,
            path: ".",
        }),
        (["check"], [path], None, None) => Ok(PackageCommand::Check { json, reir, path }),
        (["review"], [], None, None) => Ok(PackageCommand::Review {
            json,
            reir,
            path: ".",
        }),
        (["review"], [path], None, None) => Ok(PackageCommand::Review { json, reir, path }),
        (["review", "update"], [], Some(old_lock_path), Some(new_lock_path)) => {
            Ok(PackageCommand::ReviewUpdate {
                json,
                reir,
                old_lock_path,
                new_lock_path,
            })
        }
        (["lock"], [path], None, None) => Ok(PackageCommand::Lock { json, reir, path }),
        (["tree"], [], None, None) => Ok(PackageCommand::Tree {
            json,
            reir,
            path: ".",
        }),
        (["tree"], [path], None, None) => Ok(PackageCommand::Tree { json, reir, path }),
        (["publish"], [], None, None) => Ok(PackageCommand::Publish {
            json,
            reir,
            dry_run,
            path: ".",
            registry: registry_path,
        }),
        (["publish"], [path], None, None) => Ok(PackageCommand::Publish {
            json,
            reir,
            dry_run,
            path,
            registry: registry_path,
        }),
        (["vendor"], [], None, None) => Ok(PackageCommand::Vendor {
            json,
            reir,
            dry_run,
            path: ".",
        }),
        (["vendor"], [path], None, None) => Ok(PackageCommand::Vendor {
            json,
            reir,
            dry_run,
            path,
        }),
        (["metadata"], [], None, None) => Ok(PackageCommand::Metadata {
            json,
            reir,
            dry_run,
            verify,
            path: ".",
        }),
        (["metadata"], [path], None, None) => Ok(PackageCommand::Metadata {
            json,
            reir,
            dry_run,
            verify,
            path,
        }),
        (["diff"], [old_path, new_path], None, None) => Ok(PackageCommand::Diff {
            json,
            reir,
            old_path,
            new_path,
        }),
        (["reir", "diff"], [], Some(baseline_path), Some(current_path)) => {
            Ok(PackageCommand::ReirDiff {
                json,
                fail_on_change,
                baseline_path,
                current_path,
            })
        }
        (["reir", "diff"], [baseline_path, current_path], None, None) => {
            Ok(PackageCommand::ReirDiff {
                json,
                fail_on_change,
                baseline_path,
                current_path,
            })
        }
        _ => Err("invalid package arguments.".to_string()),
    }
}
pub(crate) fn run_package_check(json: bool, reir: bool, path: &str) -> ExitCode {
    let check = match check_package_dir(Path::new(path)) {
        Ok(check) => check,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if reir {
        println!("{}", format_package_check_reir_json(&check));
    } else if json {
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

fn run_package_review(json: bool, reir: bool, path: &str) -> ExitCode {
    let review = match review_package_dir(Path::new(path)) {
        Ok(review) => review,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_package_review_json(&review));
    } else if reir {
        println!("{}", format_package_review_reir_json(&review));
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

fn run_package_lock(json: bool, reir: bool, path: &str) -> ExitCode {
    let lock = match lock_package_dir(Path::new(path)) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if reir {
        println!(
            "{}",
            format_package_lock_reir_json_with_path(&lock, &Path::new(path).join("rsspkg.lock"))
        );
    } else if json {
        println!("{}", format_package_lock_json(&lock));
    } else {
        print!("{}", format_package_lock_toml(&lock));
    }
    ExitCode::SUCCESS
}

fn run_package_review_update(
    json: bool,
    reir: bool,
    old_lock_path: &str,
    new_lock_path: &str,
) -> ExitCode {
    let diff = match diff_package_locks(Path::new(old_lock_path), Path::new(new_lock_path)) {
        Ok(diff) => diff,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if reir {
        println!("{}", format_package_lock_diff_reir_json(&diff));
    } else if json {
        println!("{}", format_package_lock_diff_json(&diff));
    } else {
        print!("{}", format_package_lock_diff_human(&diff));
    }
    ExitCode::SUCCESS
}

fn run_package_tree(json: bool, reir: bool, path: &str) -> ExitCode {
    let tree = match package_tree(Path::new(path)) {
        Ok(tree) => tree,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if reir {
        println!("{}", format_package_tree_reir_json(&tree));
    } else if json {
        println!("{}", format_package_tree_json(&tree));
    } else {
        print!("{}", format_package_tree_human(&tree));
    }
    ExitCode::SUCCESS
}

fn run_package_publish(
    json: bool,
    reir: bool,
    dry_run: bool,
    path: &str,
    registry: Option<&str>,
) -> ExitCode {
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

    if reir {
        println!("{}", format_package_publish_reir_json(&publish));
    } else if json {
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

fn run_package_vendor(json: bool, reir: bool, dry_run: bool, path: &str) -> ExitCode {
    let vendor = match vendor_package_dir(Path::new(path), dry_run) {
        Ok(vendor) => vendor,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if reir {
        println!("{}", format_package_vendor_reir_json(&vendor));
    } else if json {
        println!("{}", format_package_vendor_json(&vendor));
    } else {
        print!("{}", format_package_vendor_human(&vendor));
    }

    if vendor.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_package_metadata(
    json: bool,
    reir: bool,
    dry_run: bool,
    verify: bool,
    path: &str,
) -> ExitCode {
    let metadata = if verify {
        package_metadata_verify(Path::new(path))
    } else {
        package_metadata(Path::new(path), dry_run)
    };
    let metadata = match metadata {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if reir {
        println!("{}", format_package_metadata_reir_json(&metadata));
    } else if json {
        println!("{}", format_package_metadata_json(&metadata));
    } else {
        print!("{}", format_package_metadata_human(&metadata));
        if !metadata.metadata.diagnostics.is_empty() {
            print!(
                "{}",
                format_diagnostics_human(&metadata.metadata.diagnostics)
            );
        }
    }

    if metadata.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_package_diff(json: bool, reir: bool, old_path: &str, new_path: &str) -> ExitCode {
    if reir {
        let old_review = match review_package_dir(Path::new(old_path)) {
            Ok(review) => review,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
        };
        let new_review = match review_package_dir(Path::new(new_path)) {
            Ok(review) => review,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
        };
        println!(
            "{}",
            format_package_review_reir_diff_json(&old_review, &new_review)
        );
        return if new_review
            .diagnostics
            .iter()
            .chain(old_review.diagnostics.iter())
            .any(|diagnostic| diagnostic.severity.is_error())
        {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        };
    }

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

fn run_package_reir_diff(
    json: bool,
    fail_on_change: bool,
    baseline_path: &str,
    current_path: &str,
) -> ExitCode {
    let baseline = match read_reir_bundle(baseline_path) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let current = match read_reir_bundle(current_path) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    let diff = reir::compute_diff(&baseline, &current);
    if json {
        println!(
            "{}",
            serde_json::to_string(&diff).expect("REIR diff should serialize")
        );
    } else {
        print!("{}", reir::format_diff_human(&diff));
    }
    if fail_on_change && !diff.items.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn read_reir_bundle(path: &str) -> Result<reir::Bundle, String> {
    let json = fs::read_to_string(path)
        .map_err(|error| format!("failed to read REIR bundle {path}: {error}"))?;
    reir::Bundle::from_json(&json)
        .map_err(|error| format!("failed to parse REIR bundle {path}: {error}"))
}

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_package_args_rejects_missing_flag_values() {
        let values = args(&["review", "update", "--from", "old.lock", "--to"]);
        let error = super::parse_package_args(&values).expect_err("missing to should fail");

        assert_eq!(error, "missing value for `--to`.");
    }

    #[test]
    fn parse_package_args_rejects_unknown_flags() {
        let values = args(&["publish", "--wat", "package"]);
        let error = super::parse_package_args(&values).expect_err("unknown flag should fail");

        assert_eq!(error, "unknown argument `--wat`.");
    }

    #[test]
    fn parse_package_args_rejects_planned_extension_flags() {
        let values = args(&["check", "--native-abi", "package"]);
        let error = super::parse_package_args(&values).expect_err("planned flag should not parse");

        assert_eq!(
            error,
            "`--native-abi` is a planned package-manager extension and is not supported by this build."
        );

        let values = args(&["diff", "--update-plan", "--json", "old", "new"]);
        let error = super::parse_package_args(&values).expect_err("planned flag should not parse");

        assert_eq!(
            error,
            "`--update-plan` is a planned package-manager extension and is not supported by this build."
        );
    }

    #[test]
    fn parse_package_args_rejects_planned_commands() {
        let values = args(&["audit-surface", "--json"]);
        let error =
            super::parse_package_args(&values).expect_err("planned command should not parse");

        assert_eq!(
            error,
            "`rss pkg audit-surface` is a planned package-manager command and is not supported by this build."
        );

        let values = args(&["compare", "rss-json", "rss-fast-json"]);
        let error =
            super::parse_package_args(&values).expect_err("planned command should not parse");

        assert_eq!(
            error,
            "`rss pkg compare` is a planned package-manager command and is not supported by this build."
        );
    }

    #[test]
    fn parse_package_args_accepts_review_reir_output() {
        let values = args(&["review", "--reir", "package"]);
        let command = super::parse_package_args(&values).expect("review --reir should parse");

        match command {
            super::PackageCommand::Review { json, reir, path } => {
                assert!(!json);
                assert!(reir);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_check_reir_output() {
        let values = args(&["check", "--reir", "package"]);
        let command = super::parse_package_args(&values).expect("check --reir should parse");

        match command {
            super::PackageCommand::Check { json, reir, path } => {
                assert!(!json);
                assert!(reir);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_diff_reir_output() {
        let values = args(&["diff", "--reir", "old-package", "new-package"]);
        let command = super::parse_package_args(&values).expect("diff --reir should parse");

        match command {
            super::PackageCommand::Diff {
                json,
                reir,
                old_path,
                new_path,
            } => {
                assert!(!json);
                assert!(reir);
                assert_eq!(old_path, "old-package");
                assert_eq!(new_path, "new-package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_lock_reir_output() {
        let values = args(&["lock", "--reir", "package"]);
        let command = super::parse_package_args(&values).expect("lock --reir should parse");

        match command {
            super::PackageCommand::Lock { json, reir, path } => {
                assert!(!json);
                assert!(reir);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_review_update_reir_output() {
        let values = args(&[
            "review", "update", "--reir", "--from", "old.lock", "--to", "new.lock",
        ]);
        let command =
            super::parse_package_args(&values).expect("review update --reir should parse");

        match command {
            super::PackageCommand::ReviewUpdate {
                json,
                reir,
                old_lock_path,
                new_lock_path,
            } => {
                assert!(!json);
                assert!(reir);
                assert_eq!(old_lock_path, "old.lock");
                assert_eq!(new_lock_path, "new.lock");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_tree_reir_output() {
        let values = args(&["tree", "--reir", "package"]);
        let command = super::parse_package_args(&values).expect("tree --reir should parse");

        match command {
            super::PackageCommand::Tree { json, reir, path } => {
                assert!(!json);
                assert!(reir);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_publish_reir_output() {
        let values = args(&["publish", "--dry-run", "--reir", "package"]);
        let command = super::parse_package_args(&values).expect("publish --reir should parse");

        match command {
            super::PackageCommand::Publish {
                json,
                reir,
                dry_run,
                path,
                registry,
            } => {
                assert!(!json);
                assert!(reir);
                assert!(dry_run);
                assert_eq!(path, "package");
                assert_eq!(registry, None);
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_publish_registry_output_path() {
        let values = args(&[
            "publish",
            "--dry-run",
            "--reir",
            "--registry",
            "registry",
            "package",
        ]);
        let command = super::parse_package_args(&values).expect("publish --registry should parse");

        match command {
            super::PackageCommand::Publish {
                json,
                reir,
                dry_run,
                path,
                registry,
            } => {
                assert!(!json);
                assert!(reir);
                assert!(dry_run);
                assert_eq!(path, "package");
                assert_eq!(registry, Some("registry"));
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_rejects_publish_without_dry_run() {
        let values = args(&["publish", "--reir", "package"]);
        let error =
            super::parse_package_args(&values).expect_err("publish without dry-run should fail");

        assert_eq!(error, "`rss pkg publish` currently requires `--dry-run`.");
    }

    #[test]
    fn parse_package_args_rejects_dry_run_for_unsupported_commands() {
        let values = args(&["check", "--dry-run", "package"]);
        let error = super::parse_package_args(&values).expect_err("check --dry-run should fail");

        assert_eq!(
            error,
            "`--dry-run` is only supported for `rss pkg publish`, `rss pkg vendor`, and `rss pkg metadata`."
        );
    }

    #[test]
    fn parse_package_args_rejects_verify_for_non_metadata_commands() {
        let values = args(&["check", "--verify", "package"]);
        let error = super::parse_package_args(&values).expect_err("check --verify should fail");

        assert_eq!(
            error,
            "`--verify` is only supported for `rss pkg metadata`."
        );
    }

    #[test]
    fn parse_package_args_rejects_registry_for_non_publish_commands() {
        let values = args(&["review", "--registry", "registry", "package"]);
        let error = super::parse_package_args(&values).expect_err("review --registry should fail");

        assert_eq!(
            error,
            "`--registry` is only supported for `rss pkg publish`."
        );
    }

    #[test]
    fn parse_package_args_accepts_vendor_reir_output() {
        let values = args(&["vendor", "--dry-run", "--reir", "package"]);
        let command = super::parse_package_args(&values).expect("vendor --reir should parse");

        match command {
            super::PackageCommand::Vendor {
                json,
                reir,
                dry_run,
                path,
            } => {
                assert!(!json);
                assert!(reir);
                assert!(dry_run);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_reir_artifact_diff_with_flags() {
        let values = args(&[
            "reir", "diff", "--json", "--from", "old.json", "--to", "new.json",
        ]);
        let command = super::parse_package_args(&values).expect("reir artifact diff should parse");

        match command {
            super::PackageCommand::ReirDiff {
                json,
                fail_on_change,
                baseline_path,
                current_path,
            } => {
                assert!(json);
                assert!(!fail_on_change);
                assert_eq!(baseline_path, "old.json");
                assert_eq!(current_path, "new.json");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_reir_artifact_diff_with_paths() {
        let values = args(&["reir", "diff", "old.json", "new.json"]);
        let command =
            super::parse_package_args(&values).expect("reir artifact diff paths should parse");

        match command {
            super::PackageCommand::ReirDiff {
                json,
                fail_on_change,
                baseline_path,
                current_path,
            } => {
                assert!(!json);
                assert!(!fail_on_change);
                assert_eq!(baseline_path, "old.json");
                assert_eq!(current_path, "new.json");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_reir_artifact_diff_fail_on_change() {
        let values = args(&[
            "reir",
            "diff",
            "--fail-on-change",
            "--from",
            "old.json",
            "--to",
            "new.json",
        ]);
        let command =
            super::parse_package_args(&values).expect("reir diff fail-on-change should parse");

        match command {
            super::PackageCommand::ReirDiff {
                json,
                fail_on_change,
                baseline_path,
                current_path,
            } => {
                assert!(!json);
                assert!(fail_on_change);
                assert_eq!(baseline_path, "old.json");
                assert_eq!(current_path, "new.json");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_rejects_fail_on_change_for_non_reir_diff() {
        let values = args(&["diff", "--fail-on-change", "old-package", "new-package"]);
        let error =
            super::parse_package_args(&values).expect_err("unsupported fail-on-change should fail");

        assert_eq!(
            error,
            "`--fail-on-change` is only supported for `rss pkg reir diff`."
        );
    }

    #[test]
    fn parse_package_args_accepts_metadata_verify() {
        let values = args(&["metadata", "--verify", "--json", "package"]);
        let command = super::parse_package_args(&values).expect("metadata --verify should parse");

        match command {
            super::PackageCommand::Metadata {
                json,
                reir,
                dry_run,
                verify,
                path,
            } => {
                assert!(json);
                assert!(!reir);
                assert!(!dry_run);
                assert!(verify);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_metadata_reir_output() {
        let values = args(&["metadata", "--verify", "--reir", "package"]);
        let command = super::parse_package_args(&values).expect("metadata --reir should parse");

        match command {
            super::PackageCommand::Metadata {
                json,
                reir,
                dry_run,
                verify,
                path,
            } => {
                assert!(!json);
                assert!(reir);
                assert!(!dry_run);
                assert!(verify);
                assert_eq!(path, "package");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_rejects_metadata_dry_run_verify_mix() {
        let values = args(&["metadata", "--dry-run", "--verify", "package"]);
        let error =
            super::parse_package_args(&values).expect_err("mixed metadata modes should fail");

        assert_eq!(
            error,
            "`--dry-run` and `--verify` select different metadata modes."
        );
    }

    #[test]
    fn parse_package_args_rejects_reir_json_mix() {
        let values = args(&["review", "--json", "--reir", "package"]);
        let error = super::parse_package_args(&values).expect_err("mixed outputs should fail");

        assert_eq!(
            error,
            "`--json` and `--reir` select different output formats."
        );
    }

    #[test]
    fn parse_package_args_rejects_reir_for_non_review_commands() {
        let values = args(&["reir", "diff", "--reir", "old.json", "new.json"]);
        let error = super::parse_package_args(&values).expect_err("--reir reir diff should fail");

        assert_eq!(
            error,
            "`--reir` is only supported for `rss pkg check`, `rss pkg review`, `rss pkg diff`, `rss pkg lock`, `rss pkg tree`, `rss pkg publish`, `rss pkg vendor`, `rss pkg metadata`, and `rss pkg review update`."
        );
    }
}
