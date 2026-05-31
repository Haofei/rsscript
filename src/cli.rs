use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use rsscript::{
    Diagnostic, ReviewMap, analyze_source, analyze_source_with_interfaces,
    analyze_source_with_interfaces_without_core, analyze_source_without_core,
    analyze_sources_with_interfaces, check_generated_rust_package, check_package_dir,
    core_interfaces, diff_package_dirs, diff_package_locks, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
    format_package_check_human, format_package_check_json, format_package_diff_human,
    format_package_diff_json, format_package_lock_diff_human, format_package_lock_diff_json,
    format_package_lock_json, format_package_lock_toml, format_package_metadata_human,
    format_package_metadata_json, format_package_publish_human, format_package_publish_json,
    format_package_review_human, format_package_review_json, format_package_tree_human,
    format_package_tree_json, format_package_vendor_human, format_package_vendor_json,
    format_review_human, format_review_json, format_review_map_human, format_review_map_json,
    format_source, lint_source, lock_package_dir, lower_source_to_rust,
    lower_source_to_rust_package, lower_sources_to_rust_package_with_options,
    package_lowering_input, package_metadata, package_tree, parse_runtime_diagnostics,
    parse_source_map_json, publish_package_dry_run_with_registry,
    remap_rustc_diagnostic_json_lines, review_map_sources, review_package_dir, review_sources,
    vendor_package_dir, write_generated_rust_package,
};

pub fn run() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(command) = args.get(1).map(String::as_str) else {
        print_usage();
        return ExitCode::from(2);
    };

    match command {
        "check" => run_check(&args[2..]),
        "lint" => run_lint(&args[2..]),
        "fmt" => run_fmt(&args[2..]),
        "review" => run_review(&args[2..]),
        "pkg" => run_package(&args[2..]),
        "package" => {
            eprintln!("unknown command `package`; use `rsscript pkg ...`.");
            print_usage();
            ExitCode::from(2)
        }
        "lower" => run_lower(&args[2..]),
        "run" => run_generated_rust(&args[2..]),
        "remap-rustc" => run_remap_rustc(&args[2..]),
        "verify-rust" => run_verify_rust(&args[2..]),
        _ => {
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn run_package(args: &[String]) -> ExitCode {
    let command = match parse_package_args(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            return ExitCode::from(2);
        }
    };
    match command {
        PackageCommand::Check { json, path } => run_package_check(json, path),
        PackageCommand::Review { json, path } => run_package_review(json, path),
        PackageCommand::ReviewUpdate {
            json,
            old_lock_path,
            new_lock_path,
        } => run_package_review_update(json, old_lock_path, new_lock_path),
        PackageCommand::Lock { json, path } => run_package_lock(json, path),
        PackageCommand::Tree { json, path } => run_package_tree(json, path),
        PackageCommand::Publish {
            json,
            dry_run,
            path,
            registry,
        } => run_package_publish(json, dry_run, path, registry),
        PackageCommand::Vendor {
            json,
            dry_run,
            path,
        } => run_package_vendor(json, dry_run, path),
        PackageCommand::Metadata {
            json,
            dry_run,
            path,
        } => run_package_metadata(json, dry_run, path),
        PackageCommand::Diff {
            json,
            old_path,
            new_path,
        } => run_package_diff(json, old_path, new_path),
    }
}

fn run_lint(args: &[String]) -> ExitCode {
    let options = match parse_check_args(args) {
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
    let interface_refs = interfaces
        .iter()
        .map(|interface| (interface.path.as_str(), interface.contents.as_str()))
        .collect::<Vec<_>>();
    let mut diagnostics = if options.use_core {
        let mut combined = core_interfaces().to_vec();
        combined.extend(interface_refs);
        analyze_source_with_interfaces(path, &source, &combined)
    } else if interface_refs.is_empty() {
        analyze_source_without_core(path, &source)
    } else {
        analyze_source_with_interfaces_without_core(path, &source, &interface_refs)
    };
    diagnostics.extend(lint_source(path, &source));

    if options.json {
        println!("{}", format_diagnostics_json(&diagnostics));
    } else if diagnostics.is_empty() {
        println!("{path}: lint ok");
    } else {
        print!("{}", format_diagnostics_human(&diagnostics));
    }

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_check(args: &[String]) -> ExitCode {
    if let Some(code) = parse_explain_args(args) {
        let Some(explanation) = explain_diagnostic_code(code) else {
            eprintln!("unknown diagnostic code: {code}");
            return ExitCode::from(2);
        };
        print!("{}", format_diagnostic_explanation(explanation));
        return ExitCode::SUCCESS;
    }

    let options = match parse_check_args(args) {
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

    if is_package_directory(path) {
        if let Some(error) = package_check_option_error(&options) {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
        return run_package_check(options.json, path);
    }

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
    let interface_refs = interfaces
        .iter()
        .map(|interface| (interface.path.as_str(), interface.contents.as_str()))
        .collect::<Vec<_>>();
    let diagnostics = if options.use_core {
        let mut combined = core_interfaces().to_vec();
        combined.extend(interface_refs);
        analyze_source_with_interfaces(path, &source, &combined)
    } else if interface_refs.is_empty() {
        analyze_source_without_core(path, &source)
    } else {
        analyze_source_with_interfaces_without_core(path, &source, &interface_refs)
    };
    if options.json {
        println!("{}", format_diagnostics_json(&diagnostics));
    } else if diagnostics.is_empty() {
        println!("{path}: ok");
    } else {
        print!("{}", format_diagnostics_human(&diagnostics));
    }

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_fmt(args: &[String]) -> ExitCode {
    let (_, path) = match parse_path_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let Some(path) = path else {
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

    let diagnostics = analyze_source(path, &source);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        print!("{}", format_diagnostics_human(&diagnostics));
        return ExitCode::from(1);
    }

    print!("{}", format_source(path, &source));
    ExitCode::SUCCESS
}

fn run_review(args: &[String]) -> ExitCode {
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

fn run_lower(args: &[String]) -> ExitCode {
    let options = match parse_lower_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    if !options.emit_rust {
        print_usage();
        return ExitCode::from(2);
    }
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

    if let Some(out_dir) = options.out_dir {
        return run_lower_rust_package(path, &source, out_dir);
    }

    match lower_source_to_rust(path, &source) {
        Ok(rust_source) => {
            print!("{rust_source}");
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            print!("{}", format_diagnostics_human(&diagnostics));
            ExitCode::from(1)
        }
    }
}

fn run_lower_rust_package(path: &str, source: &str, out_dir: &str) -> ExitCode {
    let runtime_path = match default_runtime_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let package_name = generated_package_name(path);
    let package = match lower_source_to_rust_package(
        path,
        source,
        &package_name,
        &runtime_path.display().to_string(),
    ) {
        Ok(package) => package,
        Err(diagnostics) => {
            print!("{}", format_diagnostics_human(&diagnostics));
            return ExitCode::from(1);
        }
    };

    let out_dir = Path::new(out_dir);
    if let Err(error) = write_generated_rust_package(out_dir, &package) {
        eprintln!("{error}");
        return ExitCode::from(2);
    }

    println!(
        "wrote Rust package `{}` to {}",
        package.package_name,
        out_dir.display()
    );
    ExitCode::SUCCESS
}

fn lower_cli_input_to_rust_package(
    path: &str,
    runtime_path: &Path,
    json: bool,
) -> Result<rsscript::GeneratedRustPackage, ExitCode> {
    let runtime_path = runtime_path.display().to_string();
    if is_package_directory(path) {
        let input = package_lowering_input(Path::new(path)).map_err(|error| {
            eprintln!("{error}");
            ExitCode::from(2)
        })?;
        return lower_sources_to_rust_package_with_options(
            &input.sources,
            &input.package.name,
            &runtime_path,
            &input.interfaces,
            &input.native_dependencies,
        )
        .map_err(|diagnostics| {
            print_diagnostics(json, &diagnostics);
            ExitCode::from(1)
        });
    }

    let source = fs::read_to_string(path).map_err(|error| {
        eprintln!("failed to read {path}: {error}");
        ExitCode::from(2)
    })?;
    let package_name = generated_package_name(path);
    lower_source_to_rust_package(path, &source, &package_name, &runtime_path).map_err(
        |diagnostics| {
            print_diagnostics(json, &diagnostics);
            ExitCode::from(1)
        },
    )
}

fn run_verify_rust(args: &[String]) -> ExitCode {
    let options = match parse_verify_args(args) {
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
    let runtime_path = match default_runtime_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let package = match lower_cli_input_to_rust_package(path, &runtime_path, options.json) {
        Ok(package) => package,
        Err(exit_code) => return exit_code,
    };
    let package_dir = options
        .out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| verify_temp_dir(&package.package_name));
    let cleanup_package_dir = options.out_dir.is_none();
    if let Err(error) = write_generated_rust_package(&package_dir, &package) {
        eprintln!("{error}");
        if cleanup_package_dir {
            cleanup_temp_dir(&package_dir);
        }
        return ExitCode::from(2);
    }
    let result = match check_generated_rust_package(&package_dir) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{error}");
            if cleanup_package_dir {
                cleanup_temp_dir(&package_dir);
            }
            return ExitCode::from(2);
        }
    };
    if cleanup_package_dir {
        cleanup_temp_dir(&package_dir);
    }

    if result.diagnostics.is_empty() {
        if result.success {
            if !options.json {
                println!("{path}: rust backend ok");
                if options.out_dir.is_some() {
                    println!("generated Rust package kept at {}", package_dir.display());
                }
            } else {
                println!("[]");
            }
            return ExitCode::SUCCESS;
        }
        if !result.stderr.trim().is_empty() {
            eprintln!("{}", result.stderr.trim());
        }
        eprintln!("rust backend check failed without mappable diagnostics");
        return ExitCode::from(1);
    }

    print_diagnostics(options.json, &result.diagnostics);
    if result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_generated_rust(args: &[String]) -> ExitCode {
    let options = match parse_run_args(args) {
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
    let runtime_path = match default_runtime_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let package = match lower_cli_input_to_rust_package(path, &runtime_path, options.json) {
        Ok(package) => package,
        Err(exit_code) => return exit_code,
    };
    if package.main_rs.is_none() {
        eprintln!(
            "rss run requires a zero-argument `fn main() -> Unit` or `fn main() -> Result<Unit, E>`."
        );
        return ExitCode::from(1);
    }

    let package_dir = options
        .out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| run_temp_dir(&package.package_name));
    let cleanup_package_dir = options.out_dir.is_none();
    if let Err(error) = write_generated_rust_package(&package_dir, &package) {
        eprintln!("{error}");
        if cleanup_package_dir {
            cleanup_temp_dir(&package_dir);
        }
        return ExitCode::from(2);
    }
    let mut cargo = Command::new("cargo");
    cargo.arg("run").arg("--quiet");
    if options.release {
        cargo.arg("--release");
    }
    cargo
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .arg("--")
        .args(&options.program_args);
    if let Some(target_dir) = generated_target_dir_from_env() {
        cargo.env("CARGO_TARGET_DIR", target_dir);
    }
    let output = match cargo.output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("failed to run cargo: {error}");
            if cleanup_package_dir {
                cleanup_temp_dir(&package_dir);
            }
            return ExitCode::from(2);
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if output.status.success() {
        if cleanup_package_dir {
            cleanup_temp_dir(&package_dir);
        }
        return ExitCode::SUCCESS;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics = parse_runtime_diagnostics(&stderr);
    if !diagnostics.is_empty() {
        if cleanup_package_dir {
            cleanup_temp_dir(&package_dir);
        }
        print_diagnostics(options.json, &diagnostics);
        return ExitCode::from(1);
    }
    match check_generated_rust_package(&package_dir) {
        Ok(result) if !result.diagnostics.is_empty() => {
            if cleanup_package_dir {
                cleanup_temp_dir(&package_dir);
            }
            print_diagnostics(options.json, &result.diagnostics);
            return if result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
            {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            };
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("{error}");
        }
    }
    if cleanup_package_dir {
        cleanup_temp_dir(&package_dir);
    }
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }
    output
        .status
        .code()
        .map(|code| ExitCode::from(code as u8))
        .unwrap_or_else(|| ExitCode::from(1))
}

fn generated_target_dir_from_env() -> Option<PathBuf> {
    let path = env::var_os("RSSCRIPT_GENERATED_TARGET_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| ramdisk_root_dir().map(|root| root.join("rsscript-generated-target")))?;
    let _ = fs::create_dir_all(&path);

    Some(path)
}

fn run_remap_rustc(args: &[String]) -> ExitCode {
    let (json, paths) = match parse_multi_path_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let [source_map_path, rustc_json_path] = paths.as_slice() else {
        print_usage();
        return ExitCode::from(2);
    };

    let source_map_json = match fs::read_to_string(source_map_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {source_map_path}: {error}");
            return ExitCode::from(2);
        }
    };
    let rustc_json_lines = match fs::read_to_string(rustc_json_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {rustc_json_path}: {error}");
            return ExitCode::from(2);
        }
    };

    let source_map = match parse_source_map_json(&source_map_json) {
        Ok(source_map) => source_map,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let remapped = match remap_rustc_diagnostic_json_lines(&source_map, &rustc_json_lines) {
        Ok(remapped) => remapped,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let diagnostics = remapped
        .into_iter()
        .map(|remapped| remapped.diagnostic)
        .collect::<Vec<_>>();

    if json {
        println!("{}", format_diagnostics_json(&diagnostics));
    } else {
        print!("{}", format_diagnostics_human(&diagnostics));
    }

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_diagnostics(json: bool, diagnostics: &[Diagnostic]) {
    if json {
        println!("{}", format_diagnostics_json(diagnostics));
    } else {
        print!("{}", format_diagnostics_human(diagnostics));
    }
}

fn parse_explain_args(args: &[String]) -> Option<&str> {
    let [flag, code] = args else {
        return None;
    };
    (flag == "--explain").then_some(code.as_str())
}

fn parse_path_args(args: &[String]) -> Result<(bool, Option<&str>), String> {
    let mut json = false;
    let mut path = None;

    for arg in args {
        if arg == "--json" {
            json = true;
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            return Err(format!("unexpected extra path `{arg}`."));
        }
    }

    Ok((json, path))
}

fn parse_multi_path_args(args: &[String]) -> Result<(bool, Vec<&str>), String> {
    let mut json = false;
    let mut paths = Vec::new();

    for arg in args {
        if arg == "--json" {
            json = true;
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else {
            paths.push(arg.as_str());
        }
    }

    Ok((json, paths))
}

#[derive(Debug)]
struct LowerOptions<'a> {
    emit_rust: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
}

#[derive(Debug)]
struct RunOptions<'a> {
    json: bool,
    release: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
    program_args: Vec<&'a str>,
}

#[derive(Debug)]
struct VerifyOptions<'a> {
    json: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
}

#[derive(Debug)]
struct CheckOptions<'a> {
    json: bool,
    use_core: bool,
    path: Option<&'a str>,
    interfaces: Vec<&'a str>,
}

fn parse_check_args(args: &[String]) -> Result<CheckOptions<'_>, String> {
    let mut json = false;
    let mut use_core = true;
    let mut path = None;
    let mut interfaces = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--core" {
            use_core = true;
        } else if arg == "--no-core" {
            use_core = false;
        } else if arg == "--interface" {
            index += 1;
            let interface = required_flag_value(args, index, "--interface")?;
            interfaces.push(interface);
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            return Err(format!("unexpected extra path `{arg}`."));
        }
        index += 1;
    }

    Ok(CheckOptions {
        json,
        use_core,
        path,
        interfaces,
    })
}

fn package_check_option_error(options: &CheckOptions<'_>) -> Option<String> {
    if !options.use_core {
        return Some(
            "`rss check --no-core` is only valid for single-file checks; package checks use package interfaces and dependencies.".to_string(),
        );
    }
    if !options.interfaces.is_empty() {
        return Some(
            "`rss check --interface` is only valid for single-file checks; package checks read interfaces from rsspkg.toml.".to_string(),
        );
    }
    None
}

fn parse_lower_args(args: &[String]) -> Result<LowerOptions<'_>, String> {
    let mut emit_rust = false;
    let mut path = None;
    let mut out_dir = None;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--rust" {
            emit_rust = true;
        } else if arg == "--out-dir" {
            index += 1;
            out_dir = Some(required_flag_value(args, index, "--out-dir")?);
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            return Err(format!("unexpected extra path `{arg}`."));
        }
        index += 1;
    }

    Ok(LowerOptions {
        emit_rust,
        path,
        out_dir,
    })
}

fn parse_run_args(args: &[String]) -> Result<RunOptions<'_>, String> {
    let mut json = false;
    let mut release = false;
    let mut path = None;
    let mut out_dir = None;
    let mut program_args = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--" {
            program_args.extend(args[index + 1..].iter().map(String::as_str));
            break;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--release" {
            release = true;
        } else if arg == "--out-dir" {
            index += 1;
            out_dir = Some(required_flag_value(args, index, "--out-dir")?);
        } else if arg.starts_with("--") && path.is_none() {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            program_args.push(arg.as_str());
        }
        index += 1;
    }

    Ok(RunOptions {
        json,
        release,
        path,
        out_dir,
        program_args,
    })
}

fn parse_verify_args(args: &[String]) -> Result<VerifyOptions<'_>, String> {
    let mut json = false;
    let mut path = None;
    let mut out_dir = None;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--out-dir" {
            index += 1;
            out_dir = Some(required_flag_value(args, index, "--out-dir")?);
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            return Err(format!("unexpected extra path `{arg}`."));
        }
        index += 1;
    }

    Ok(VerifyOptions {
        json,
        path,
        out_dir,
    })
}

fn required_flag_value<'a>(
    args: &'a [String],
    index: usize,
    flag: &str,
) -> Result<&'a str, String> {
    let Some(value) = args.get(index) else {
        return Err(format!("missing value for `{flag}`."));
    };
    if value.starts_with("--") {
        return Err(format!("missing value for `{flag}`."));
    }
    Ok(value.as_str())
}

struct InterfaceSource {
    path: String,
    contents: String,
}

fn read_interface_sources(paths: &[&str]) -> Result<Vec<InterfaceSource>, String> {
    paths
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .map(|contents| InterfaceSource {
                    path: (*path).to_string(),
                    contents,
                })
                .map_err(|error| format!("failed to read interface {path}: {error}"))
        })
        .collect()
}

fn default_runtime_path() -> Result<PathBuf, String> {
    let current_dir =
        env::current_dir().map_err(|error| format!("failed to read current directory: {error}"))?;
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("RSSCRIPT_RUNTIME_PATH") {
        candidates.push(("RSSCRIPT_RUNTIME_PATH", PathBuf::from(path)));
    }
    candidates.push(("current directory", current_dir.join("runtime")));
    candidates.push(("compiled manifest directory", manifest_dir.join("runtime")));
    select_runtime_path(candidates)
}

fn select_runtime_path(candidates: Vec<(&'static str, PathBuf)>) -> Result<PathBuf, String> {
    let mut checked = Vec::new();
    for (source, path) in candidates {
        checked.push(format!("{source}: {}", path.display()));
        if path.join("Cargo.toml").is_file() {
            return path
                .canonicalize()
                .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()));
        }
    }
    Err(format!(
        "failed to locate rsscript-runtime crate; checked {}. Set RSSCRIPT_RUNTIME_PATH to the runtime crate directory.",
        checked.join(", ")
    ))
}

fn generated_package_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("rsscript-generated")
        .to_string()
}

fn verify_temp_dir(package_name: &str) -> PathBuf {
    temp_package_dir("rsscript-verify", package_name)
}

fn run_temp_dir(package_name: &str) -> PathBuf {
    temp_package_dir("rsscript-run", package_name)
}

fn temp_package_dir(prefix: &str, package_name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    temp_root_dir().join(format!(
        "{prefix}-{package_name}-{}-{now}",
        std::process::id()
    ))
}

fn temp_root_dir() -> PathBuf {
    let root = env::var_os("RSSCRIPT_TEMP_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| ramdisk_root_dir().map(|root| root.join("rsscript-temp")))
        .unwrap_or_else(env::temp_dir);
    let _ = fs::create_dir_all(&root);

    root
}

fn ramdisk_root_dir() -> Option<PathBuf> {
    env::var_os("RSSCRIPT_RAMDISK_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(default_ramdisk_root_dir)
}

#[cfg(target_os = "macos")]
fn default_ramdisk_root_dir() -> Option<PathBuf> {
    let path = PathBuf::from("/Volumes/RSScriptRAMDisk");
    if path.is_dir() {
        return Some(path);
    }

    let gib = env::var("RSSCRIPT_RAMDISK_GIB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8);
    let sectors = gib
        .saturating_mul(1024)
        .saturating_mul(1024)
        .saturating_mul(1024)
        / 512;
    let attach = Command::new("hdiutil")
        .arg("attach")
        .arg("-nomount")
        .arg(format!("ram://{sectors}"))
        .output()
        .ok()?;
    if !attach.status.success() {
        return None;
    }
    let device = String::from_utf8_lossy(&attach.stdout).trim().to_string();
    if device.is_empty() {
        return None;
    }

    let erase = Command::new("diskutil")
        .arg("erasevolume")
        .arg("HFS+")
        .arg("RSScriptRAMDisk")
        .arg(device)
        .output()
        .ok()?;
    if !erase.status.success() || !path.is_dir() {
        return None;
    }

    Some(path)
}

#[cfg(not(target_os = "macos"))]
fn default_ramdisk_root_dir() -> Option<PathBuf> {
    None
}

fn cleanup_temp_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

#[derive(Debug)]
enum ReviewCommand<'a> {
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

#[derive(Debug)]
enum PackageCommand<'a> {
    Check {
        json: bool,
        path: &'a str,
    },
    Review {
        json: bool,
        path: &'a str,
    },
    ReviewUpdate {
        json: bool,
        old_lock_path: &'a str,
        new_lock_path: &'a str,
    },
    Lock {
        json: bool,
        path: &'a str,
    },
    Tree {
        json: bool,
        path: &'a str,
    },
    Publish {
        json: bool,
        dry_run: bool,
        path: &'a str,
        registry: Option<&'a str>,
    },
    Vendor {
        json: bool,
        dry_run: bool,
        path: &'a str,
    },
    Metadata {
        json: bool,
        dry_run: bool,
        path: &'a str,
    },
    Diff {
        json: bool,
        old_path: &'a str,
        new_path: &'a str,
    },
}

fn parse_package_args(args: &[String]) -> Result<PackageCommand<'_>, String> {
    let mut json = false;
    let mut dry_run = false;
    let mut words = Vec::new();
    let mut from_path = None;
    let mut registry_path = None;
    let mut to_path = None;
    let mut paths = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--from" {
            index += 1;
            from_path = Some(required_flag_value(args, index, "--from")?);
        } else if arg == "--registry" {
            index += 1;
            registry_path = Some(required_flag_value(args, index, "--registry")?);
        } else if arg == "--to" {
            index += 1;
            to_path = Some(required_flag_value(args, index, "--to")?);
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
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
                | "diff"
        ) {
            words.push(arg.as_str());
        } else {
            paths.push(arg.as_str());
        }
        index += 1;
    }

    match (words.as_slice(), paths.as_slice(), from_path, to_path) {
        (["check"], [], None, None) => Ok(PackageCommand::Check { json, path: "." }),
        (["check"], [path], None, None) => Ok(PackageCommand::Check { json, path }),
        (["review"], [path], None, None) => Ok(PackageCommand::Review { json, path }),
        (["review", "update"], [], Some(old_lock_path), Some(new_lock_path)) => {
            Ok(PackageCommand::ReviewUpdate {
                json,
                old_lock_path,
                new_lock_path,
            })
        }
        (["lock"], [path], None, None) => Ok(PackageCommand::Lock { json, path }),
        (["tree"], [], None, None) => Ok(PackageCommand::Tree { json, path: "." }),
        (["tree"], [path], None, None) => Ok(PackageCommand::Tree { json, path }),
        (["publish"], [], None, None) => Ok(PackageCommand::Publish {
            json,
            dry_run,
            path: ".",
            registry: registry_path,
        }),
        (["publish"], [path], None, None) => Ok(PackageCommand::Publish {
            json,
            dry_run,
            path,
            registry: registry_path,
        }),
        (["vendor"], [], None, None) => Ok(PackageCommand::Vendor {
            json,
            dry_run,
            path: ".",
        }),
        (["vendor"], [path], None, None) => Ok(PackageCommand::Vendor {
            json,
            dry_run,
            path,
        }),
        (["metadata"], [], None, None) => Ok(PackageCommand::Metadata {
            json,
            dry_run,
            path: ".",
        }),
        (["metadata"], [path], None, None) => Ok(PackageCommand::Metadata {
            json,
            dry_run,
            path,
        }),
        (["diff"], [old_path, new_path], None, None) => Ok(PackageCommand::Diff {
            json,
            old_path,
            new_path,
        }),
        _ => Err("invalid package arguments.".to_string()),
    }
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

fn run_package_check(json: bool, path: &str) -> ExitCode {
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

fn run_package_lock(json: bool, path: &str) -> ExitCode {
    let lock = match lock_package_dir(Path::new(path)) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_package_lock_json(&lock));
    } else {
        print!("{}", format_package_lock_toml(&lock));
    }
    ExitCode::SUCCESS
}

fn run_package_review_update(json: bool, old_lock_path: &str, new_lock_path: &str) -> ExitCode {
    let diff = match diff_package_locks(Path::new(old_lock_path), Path::new(new_lock_path)) {
        Ok(diff) => diff,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_package_lock_diff_json(&diff));
    } else {
        print!("{}", format_package_lock_diff_human(&diff));
    }
    ExitCode::SUCCESS
}

fn run_package_tree(json: bool, path: &str) -> ExitCode {
    let tree = match package_tree(Path::new(path)) {
        Ok(tree) => tree,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_package_tree_json(&tree));
    } else {
        print!("{}", format_package_tree_human(&tree));
    }
    ExitCode::SUCCESS
}

fn run_package_publish(json: bool, dry_run: bool, path: &str, registry: Option<&str>) -> ExitCode {
    if !dry_run {
        eprintln!("rsscript pkg publish currently requires --dry-run");
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

fn run_package_vendor(json: bool, dry_run: bool, path: &str) -> ExitCode {
    let vendor = match vendor_package_dir(Path::new(path), dry_run) {
        Ok(vendor) => vendor,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
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

fn run_package_metadata(json: bool, dry_run: bool, path: &str) -> ExitCode {
    let metadata = match package_metadata(Path::new(path), dry_run) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    if json {
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

fn is_package_directory(path: &str) -> bool {
    let path = Path::new(path);
    path.is_dir() && path.join("rsspkg.toml").exists()
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!(
        "  rsscript check [--json] [--core|--no-core] [--interface <file.rssi> ...] <file.rss>"
    );
    eprintln!("  rsscript check [--json] <package-directory>");
    eprintln!(
        "  rsscript lint [--json] [--core|--no-core] [--interface <file.rssi> ...] <file.rss>"
    );
    eprintln!("  rsscript check --explain <code>");
    eprintln!("  rsscript fmt <file.rss>");
    eprintln!("  rsscript lower --rust <file.rss>");
    eprintln!("  rsscript lower --rust <file.rss> --out-dir <directory>");
    eprintln!("  rsscript run [--json] [--release] <file-or-package-directory> [-- <args>...]");
    eprintln!(
        "  rsscript run [--json] [--release] <file-or-package-directory> --out-dir <directory> [-- <args>...]"
    );
    eprintln!("  rsscript remap-rustc [--json] <rsscript-source-map.json> <rustc-json-lines>");
    eprintln!("  rsscript verify-rust [--json] <file-or-package-directory>");
    eprintln!("  rsscript verify-rust [--json] <file-or-package-directory> --out-dir <directory>");
    eprintln!("  rsscript review [--json] --diff <old.rss> <new.rss>");
    eprintln!("  rsscript review [--json] --map <file-or-directory>");
    eprintln!("  rsscript pkg check [--json] [package-directory]");
    eprintln!("  rsscript pkg review [--json] <package-directory>");
    eprintln!(
        "  rsscript pkg review update [--json] --from <old-rsspkg.lock> --to <new-rsspkg.lock>"
    );
    eprintln!("  rsscript pkg lock [--json] <package-directory>");
    eprintln!("  rsscript pkg tree [--json] [package-directory]");
    eprintln!(
        "  rsscript pkg publish --dry-run [--json] [--registry <directory>] [package-directory]"
    );
    eprintln!("  rsscript pkg vendor [--dry-run] [--json] [package-directory]");
    eprintln!("  rsscript pkg metadata [--dry-run] [--json] [package-directory]");
    eprintln!("  rsscript pkg diff [--json] <old-package-directory> <new-package-directory>");
}

#[cfg(test)]
mod tests {
    use super::parse_run_args;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_run_args_accepts_release_before_path() {
        let values = args(&[
            "--json",
            "--release",
            "rss/rayon/tests/sort-speed",
            "--",
            "input",
        ]);
        let options = parse_run_args(&values).expect("arguments should parse");

        assert!(options.json);
        assert!(options.release);
        assert_eq!(options.path, Some("rss/rayon/tests/sort-speed"));
        assert_eq!(options.program_args, vec!["input"]);
    }

    #[test]
    fn parse_run_args_treats_release_after_separator_as_program_arg() {
        let values = args(&["rss/rayon/tests/sort-speed", "--", "--release"]);
        let options = parse_run_args(&values).expect("arguments should parse");

        assert!(!options.release);
        assert_eq!(options.path, Some("rss/rayon/tests/sort-speed"));
        assert_eq!(options.program_args, vec!["--release"]);
    }

    #[test]
    fn parse_check_args_rejects_missing_interface_value() {
        let values = args(&["--interface", "--json", "demo.rss"]);
        let error = super::parse_check_args(&values).expect_err("missing interface should fail");

        assert_eq!(error, "missing value for `--interface`.");
    }

    #[test]
    fn package_check_options_reject_single_file_flags() {
        let values = args(&["--no-core", "package"]);
        let options = super::parse_check_args(&values).expect("arguments should parse");
        let error = super::package_check_option_error(&options)
            .expect("package check should reject no-core");

        assert!(error.contains("--no-core"));

        let values = args(&["--interface", "api.rssi", "package"]);
        let options = super::parse_check_args(&values).expect("arguments should parse");
        let error = super::package_check_option_error(&options)
            .expect("package check should reject explicit interface");

        assert!(error.contains("--interface"));
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
    fn parse_lower_args_rejects_unknown_flags() {
        let values = args(&["--rust", "--wat", "demo.rss"]);
        let error = super::parse_lower_args(&values).expect_err("unknown flag should fail");

        assert_eq!(error, "unknown argument `--wat`.");
    }

    #[test]
    fn parse_verify_args_rejects_extra_paths() {
        let values = args(&["one.rss", "two.rss"]);
        let error = super::parse_verify_args(&values).expect_err("extra path should fail");

        assert_eq!(error, "unexpected extra path `two.rss`.");
    }

    #[test]
    fn parse_run_args_rejects_missing_out_dir_value() {
        let values = args(&["demo.rss", "--out-dir"]);
        let error = parse_run_args(&values).expect_err("missing out-dir should fail");

        assert_eq!(error, "missing value for `--out-dir`.");
    }

    #[test]
    fn parse_path_args_rejects_extra_paths() {
        let values = args(&["one.rss", "two.rss"]);
        let error = super::parse_path_args(&values).expect_err("extra path should fail");

        assert_eq!(error, "unexpected extra path `two.rss`.");
    }

    #[test]
    fn parse_multi_path_args_rejects_unknown_flags() {
        let values = args(&["--wat", "source-map.json", "rustc.json"]);
        let error = super::parse_multi_path_args(&values).expect_err("unknown flag should fail");

        assert_eq!(error, "unknown argument `--wat`.");
    }

    #[test]
    fn parse_review_args_rejects_unknown_flags() {
        let values = args(&["--map", "--wat", "package"]);
        let error = super::parse_review_args(&values).expect_err("unknown flag should fail");

        assert_eq!(error, "unknown argument `--wat`.");
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
    fn runtime_path_selection_uses_first_valid_candidate() {
        let root = unique_temp_dir("runtime-path-selection");
        let invalid = root.join("missing");
        let valid = root.join("runtime");
        fs::create_dir_all(&valid).expect("runtime directory should create");
        fs::write(
            valid.join("Cargo.toml"),
            "[package]\nname = \"rsscript-runtime\"\n",
        )
        .expect("runtime manifest should write");

        let selected =
            super::select_runtime_path(vec![("env", invalid), ("manifest", valid.clone())])
                .expect("valid runtime path should be selected");

        assert_eq!(
            selected,
            valid
                .canonicalize()
                .expect("valid path should canonicalize")
        );
        fs::remove_dir_all(root).expect("temp runtime path should clean up");
    }

    #[test]
    fn runtime_path_selection_reports_checked_candidates() {
        let root = unique_temp_dir("runtime-path-missing");
        let missing = root.join("missing");
        let error = super::select_runtime_path(vec![("env", missing.clone())])
            .expect_err("missing runtime should fail");

        assert!(error.contains("RSSCRIPT_RUNTIME_PATH"));
        assert!(error.contains(&missing.display().to_string()));
        fs::remove_dir_all(root).expect("temp runtime path should clean up");
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
