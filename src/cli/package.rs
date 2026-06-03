use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rsscript::{
    check_package_dir, diff_package_dirs, format_diagnostics_human, format_package_check_human,
    format_package_check_json, format_package_diff_human, format_package_diff_json,
    format_package_lock_json, format_package_lock_reir_json, format_package_lock_toml,
    format_package_metadata_human, format_package_metadata_json, format_package_metadata_reir_json,
    format_package_publish_human, format_package_publish_json, format_package_review_human,
    format_package_review_json, format_package_tree_human, format_package_tree_json,
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
        PackageCommand::Lock { json, reir, path } => run_package_lock(json, reir, path),
        PackageCommand::Tree { json, reir, path } => run_package_tree(json, reir, path),
        PackageCommand::Metadata {
            json,
            reir,
            verify,
            dry_run,
            path,
        } => run_package_metadata(json, reir, verify, dry_run, path),
        PackageCommand::Vendor {
            json,
            reir,
            dry_run,
            path,
        } => run_package_vendor(json, reir, dry_run, path),
        PackageCommand::Add { dependency } => run_package_add(dependency),
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
    Metadata {
        json: bool,
        reir: bool,
        verify: bool,
        dry_run: bool,
        path: &'a str,
    },
    Vendor {
        json: bool,
        reir: bool,
        dry_run: bool,
        path: &'a str,
    },
    Add {
        dependency: &'a str,
    },
}

fn parse_package_args(args: &[String]) -> Result<PackageCommand<'_>, String> {
    let mut json = false;
    let mut reir = false;
    let mut verify = false;
    let mut dry_run = false;
    let mut registry_path = None;
    let mut words = Vec::new();
    let mut paths = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--reir" {
            reir = true;
        } else if arg == "--verify" {
            verify = true;
        } else if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--registry" {
            index += 1;
            registry_path = Some(required_flag_value(args, index, "--registry")?);
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if matches!(
            arg.as_str(),
            "review" | "diff" | "ci" | "publish" | "lock" | "tree" | "metadata" | "vendor" | "add"
        ) {
            words.push(arg.as_str());
        } else {
            paths.push(arg.as_str());
        }
        index += 1;
    }

    if json && reir {
        return Err("`--json` and `--reir` cannot be combined.".to_owned());
    }
    if json && matches!(words.as_slice(), ["add"]) {
        return Err("`--json` is not supported for `rss pkg add`.".to_owned());
    }
    if dry_run && !matches!(words.as_slice(), ["publish"] | ["vendor"] | ["metadata"]) {
        return Err(
            "`--dry-run` is only supported for `rss pkg publish`, `vendor`, or `metadata`."
                .to_owned(),
        );
    }
    if registry_path.is_some() && !matches!(words.as_slice(), ["publish"]) {
        return Err("`--registry` is only supported for `rss pkg publish`.".to_owned());
    }
    if verify && !matches!(words.as_slice(), ["metadata"]) {
        return Err("`--verify` is only supported for `rss pkg metadata`.".to_owned());
    }
    if reir
        && !matches!(
            words.as_slice(),
            ["lock"] | ["tree"] | ["metadata"] | ["vendor"]
        )
    {
        return Err(
            "`--reir` is only supported for `rss pkg lock`, `tree`, `metadata`, or `vendor`."
                .to_owned(),
        );
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
        (["lock"], []) => Ok(PackageCommand::Lock {
            json,
            reir,
            path: ".",
        }),
        (["lock"], [path]) => Ok(PackageCommand::Lock { json, reir, path }),
        (["tree"], []) => Ok(PackageCommand::Tree {
            json,
            reir,
            path: ".",
        }),
        (["tree"], [path]) => Ok(PackageCommand::Tree { json, reir, path }),
        (["metadata"], []) => Ok(PackageCommand::Metadata {
            json,
            reir,
            verify,
            dry_run,
            path: ".",
        }),
        (["metadata"], [path]) => Ok(PackageCommand::Metadata {
            json,
            reir,
            verify,
            dry_run,
            path,
        }),
        (["vendor"], []) => Ok(PackageCommand::Vendor {
            json,
            reir,
            dry_run,
            path: ".",
        }),
        (["vendor"], [path]) => Ok(PackageCommand::Vendor {
            json,
            reir,
            dry_run,
            path,
        }),
        (["add"], [dependency]) => Ok(PackageCommand::Add { dependency }),
        _ => Err("invalid package arguments.".to_string()),
    }
}

pub(crate) fn run_new_package(args: &[String]) -> ExitCode {
    let Some(name) = args.first() else {
        eprintln!("missing package name.");
        return ExitCode::from(2);
    };
    if args.len() > 1 {
        eprintln!("unexpected extra argument `{}`.", args[1]);
        return ExitCode::from(2);
    }
    match create_new_package(Path::new(name), name) {
        Ok(()) => {
            println!("created RSScript package `{name}`");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn create_new_package(path: &Path, name: &str) -> Result<(), String> {
    validate_package_name(name)?;
    if path.exists() {
        return Err(format!("package path already exists: {}", path.display()));
    }
    fs::create_dir_all(path.join("src"))
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    fs::write(path.join("rsspkg.toml"), package_manifest_template(name))
        .map_err(|error| format!("failed to write {}/rsspkg.toml: {error}", path.display()))?;
    fs::write(path.join("src/main.rss"), package_main_template(name))
        .map_err(|error| format!("failed to write {}/src/main.rss: {error}", path.display()))?;
    let lock = lock_package_dir(path)?;
    fs::write(path.join("rsspkg.lock"), format_package_lock_toml(&lock))
        .map_err(|error| format!("failed to write {}/rsspkg.lock: {error}", path.display()))?;
    Ok(())
}

fn package_manifest_template(name: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2026"

[sources]
paths = ["src"]
"#
    )
}

fn package_main_template(name: &str) -> String {
    format!(
        r#"fn main() -> Unit {{
    Log.write(message: read "hello {name}")
    return Unit
}}
"#
    )
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

fn run_package_lock(json: bool, reir: bool, path: &str) -> ExitCode {
    let lock = match lock_package_dir(Path::new(path)) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    let toml = format_package_lock_toml(&lock);
    let lock_path = Path::new(path).join("rsspkg.lock");
    if let Err(error) = fs::write(&lock_path, &toml) {
        eprintln!("failed to write {}: {error}", lock_path.display());
        return ExitCode::from(2);
    }

    if reir {
        println!("{}", format_package_lock_reir_json(&lock));
    } else if json {
        println!("{}", format_package_lock_json(&lock));
    } else {
        print!("{toml}");
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

fn run_package_metadata(
    json: bool,
    reir: bool,
    verify: bool,
    dry_run: bool,
    path: &str,
) -> ExitCode {
    let report = if verify {
        package_metadata_verify(Path::new(path))
    } else {
        package_metadata(Path::new(path), dry_run)
    };
    let report = match report {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if reir {
        println!("{}", format_package_metadata_reir_json(&report));
    } else if json {
        println!("{}", format_package_metadata_json(&report));
    } else {
        print!("{}", format_package_metadata_human(&report));
    }

    if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_package_vendor(json: bool, reir: bool, dry_run: bool, path: &str) -> ExitCode {
    let report = match vendor_package_dir(Path::new(path), dry_run) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if reir {
        println!("{}", format_package_vendor_reir_json(&report));
    } else if json {
        println!("{}", format_package_vendor_json(&report));
    } else {
        print!("{}", format_package_vendor_human(&report));
    }

    if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_package_add(dependency: &str) -> ExitCode {
    match add_dependency_to_package(Path::new("."), dependency) {
        Ok(name) => {
            println!("added dependency `{name}`");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn add_dependency_to_package(package_dir: &Path, dependency: &str) -> Result<String, String> {
    let manifest_path = package_dir.join("rsspkg.toml");
    let source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let mut manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))?;
    let (name, value) = dependency_value(package_dir, dependency)?;
    let table = manifest
        .as_table_mut()
        .ok_or_else(|| "rsspkg.toml must be a TOML table.".to_string())?;
    let dependencies = table
        .entry("dependencies")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| "`[dependencies]` must be a TOML table.".to_string())?;
    dependencies.insert(name.clone(), value);
    let rendered = toml::to_string_pretty(&manifest)
        .map_err(|error| format!("failed to render {}: {error}", manifest_path.display()))?;
    fs::write(&manifest_path, rendered)
        .map_err(|error| format!("failed to write {}: {error}", manifest_path.display()))?;
    Ok(name)
}

fn dependency_value(package_dir: &Path, dependency: &str) -> Result<(String, toml::Value), String> {
    let candidate_path = dependency_path(package_dir, dependency);
    if candidate_path.join("rsspkg.toml").is_file() {
        let name = package_name_from_manifest(&candidate_path.join("rsspkg.toml"))?;
        validate_package_name(&name)?;
        let mut table = toml::map::Map::new();
        table.insert(
            "path".to_string(),
            toml::Value::String(normalized_path_text(dependency)),
        );
        return Ok((name, toml::Value::Table(table)));
    }

    if let Some((name, version)) = dependency.split_once('@') {
        validate_package_name(name)?;
        if version.is_empty() {
            return Err("dependency version cannot be empty.".to_string());
        }
        return Ok((name.to_string(), toml::Value::String(version.to_string())));
    }

    validate_package_name(dependency)?;
    Ok((dependency.to_string(), toml::Value::String("*".to_string())))
}

fn dependency_path(package_dir: &Path, dependency: &str) -> PathBuf {
    let path = PathBuf::from(dependency);
    if path.is_absolute() {
        path
    } else {
        package_dir.join(path)
    }
}

fn package_name_from_manifest(manifest_path: &Path) -> Result<String, String> {
    let source = fs::read_to_string(manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))?;
    manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{} is missing `[package] name`.", manifest_path.display()))
}

fn normalized_path_text(path: &str) -> String {
    path.replace('\\', "/")
}

fn validate_package_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("package name cannot be empty.".to_string());
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!(
            "package name `{name}` may only contain ASCII letters, digits, `_`, or `-`."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("temp dir should be created");
        path
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
        let values = args(&["examples/packages/code-agent"]);
        let command = super::parse_package_args(&values).expect("path check should parse");

        match command {
            super::PackageCommand::Check { json, path } => {
                assert!(!json);
                assert_eq!(path, "examples/packages/code-agent");
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
    fn parse_package_args_accepts_lock_tree_metadata_and_vendor() {
        let values = args(&["lock", "pkgdir"]);
        match super::parse_package_args(&values).expect("lock should parse") {
            super::PackageCommand::Lock { json, reir, path } => {
                assert!(!json && !reir);
                assert_eq!(path, "pkgdir");
            }
            other => panic!("unexpected package command: {other:?}"),
        }

        let values = args(&["tree", "--reir", "pkgdir"]);
        match super::parse_package_args(&values).expect("tree --reir should parse") {
            super::PackageCommand::Tree { reir, path, .. } => {
                assert!(reir);
                assert_eq!(path, "pkgdir");
            }
            other => panic!("unexpected package command: {other:?}"),
        }

        let values = args(&["metadata", "--verify", "pkgdir"]);
        match super::parse_package_args(&values).expect("metadata --verify should parse") {
            super::PackageCommand::Metadata {
                verify,
                dry_run,
                path,
                ..
            } => {
                assert!(verify && !dry_run);
                assert_eq!(path, "pkgdir");
            }
            other => panic!("unexpected package command: {other:?}"),
        }

        let values = args(&["vendor", "--dry-run"]);
        match super::parse_package_args(&values).expect("vendor --dry-run should parse") {
            super::PackageCommand::Vendor { dry_run, path, .. } => {
                assert!(dry_run);
                assert_eq!(path, ".");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_accepts_add() {
        let values = args(&["add", "rss-async"]);
        match super::parse_package_args(&values).expect("add should parse") {
            super::PackageCommand::Add { dependency } => {
                assert_eq!(dependency, "rss-async");
            }
            other => panic!("unexpected package command: {other:?}"),
        }
    }

    #[test]
    fn parse_package_args_restricts_reir_and_verify_to_their_commands() {
        assert_eq!(
            super::parse_package_args(&args(&["review", "--reir", "p"]))
                .expect_err("reir on review should fail"),
            "`--reir` is only supported for `rss pkg lock`, `tree`, `metadata`, or `vendor`."
        );
        assert_eq!(
            super::parse_package_args(&args(&["lock", "--verify", "p"]))
                .expect_err("verify on lock should fail"),
            "`--verify` is only supported for `rss pkg metadata`."
        );
        assert_eq!(
            super::parse_package_args(&args(&["tree", "--json", "--reir", "p"]))
                .expect_err("json+reir should fail"),
            "`--json` and `--reir` cannot be combined."
        );
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
            "`--dry-run` is only supported for `rss pkg publish`, `vendor`, or `metadata`."
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

        let values = args(&["add", "--json", "dep"]);
        let error = super::parse_package_args(&values).expect_err("add json should fail");
        assert_eq!(error, "`--json` is not supported for `rss pkg add`.");
    }

    #[test]
    fn new_package_writes_minimal_package_skeleton() {
        let root = temp_dir("rsscript-new-package");
        let package_dir = root.join("hello-rss");

        super::create_new_package(&package_dir, "hello-rss")
            .expect("new package should be created");

        let manifest =
            fs::read_to_string(package_dir.join("rsspkg.toml")).expect("manifest should exist");
        let main = fs::read_to_string(package_dir.join("src/main.rss")).expect("main should exist");
        let lock = fs::read_to_string(package_dir.join("rsspkg.lock")).expect("lock should exist");
        assert!(manifest.contains("name = \"hello-rss\""));
        assert!(manifest.contains("[sources]"));
        assert!(main.contains("hello hello-rss"));
        assert!(lock.contains("version = 1"));
        assert!(lock.contains("name = \"hello-rss\""));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn package_add_writes_version_and_path_dependencies() {
        let root = temp_dir("rsscript-pkg-add");
        let app = root.join("app");
        let dep = root.join("dep");
        super::create_new_package(&app, "app").expect("app package should be created");
        super::create_new_package(&dep, "dep-pkg").expect("dep package should be created");

        super::add_dependency_to_package(&app, "rss-async@0.1.0")
            .expect("version dependency should add");
        let relative_dep = "../dep";
        super::add_dependency_to_package(&app, relative_dep).expect("path dependency should add");

        let manifest =
            fs::read_to_string(app.join("rsspkg.toml")).expect("manifest should be readable");
        assert!(manifest.contains("rss-async = \"0.1.0\""));
        assert!(manifest.contains("[dependencies.dep-pkg]"));
        assert!(manifest.contains("path = \"../dep\""));

        let _ = fs::remove_dir_all(root);
    }
}
