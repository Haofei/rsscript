use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use rsscript::{
    Diagnostic, analyze_source, analyze_source_with_interfaces, check_generated_rust_package,
    check_package_dir, core_interfaces, diff_package_dirs, diff_package_locks,
    explain_diagnostic_code, format_diagnostic_explanation, format_diagnostics_human,
    format_diagnostics_json, format_package_check_human, format_package_check_json,
    format_package_diff_human, format_package_diff_json, format_package_lock_diff_human,
    format_package_lock_diff_json, format_package_lock_json, format_package_lock_toml,
    format_package_metadata_human, format_package_metadata_json, format_package_publish_human,
    format_package_publish_json, format_package_review_human, format_package_review_json,
    format_package_tree_human, format_package_tree_json, format_package_vendor_human,
    format_package_vendor_json, format_review_human, format_review_json, format_review_map_human,
    format_review_map_json, format_source, lint_source, lock_package_dir, lower_source_to_rust,
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
    match parse_package_args(args) {
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
        PackageCommand::Invalid => {
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn run_lint(args: &[String]) -> ExitCode {
    let options = parse_check_args(args);
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
        analyze_source(path, &source)
    } else {
        analyze_source_with_interfaces(path, &source, &interface_refs)
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

    let options = parse_check_args(args);
    let Some(path) = options.path else {
        print_usage();
        return ExitCode::from(2);
    };

    if is_package_directory(path) {
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
        analyze_source(path, &source)
    } else {
        analyze_source_with_interfaces(path, &source, &interface_refs)
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
    let (_, path) = parse_path_args(args);
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
    match parse_review_args(args) {
        ReviewCommand::Diff {
            json,
            old_path,
            new_path,
        } => run_review_diff(json, old_path, new_path),
        ReviewCommand::Map { json, path } => run_review_map(json, path),
        ReviewCommand::Invalid => {
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn run_lower(args: &[String]) -> ExitCode {
    let options = parse_lower_args(args);
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
    let options = parse_verify_args(args);
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
    let options = parse_run_args(args);
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
    cargo
        .arg("run")
        .arg("--quiet")
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
    env::var_os("RSSCRIPT_GENERATED_TARGET_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn run_remap_rustc(args: &[String]) -> ExitCode {
    let (json, paths) = parse_multi_path_args(args);
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

fn parse_path_args(args: &[String]) -> (bool, Option<&str>) {
    let mut json = false;
    let mut path = None;

    for arg in args {
        if arg == "--json" {
            json = true;
        } else if path.is_none() {
            path = Some(arg.as_str());
        }
    }

    (json, path)
}

fn parse_multi_path_args(args: &[String]) -> (bool, Vec<&str>) {
    let mut json = false;
    let mut paths = Vec::new();

    for arg in args {
        if arg == "--json" {
            json = true;
        } else {
            paths.push(arg.as_str());
        }
    }

    (json, paths)
}

struct LowerOptions<'a> {
    emit_rust: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
}

struct RunOptions<'a> {
    json: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
    program_args: Vec<&'a str>,
}

struct VerifyOptions<'a> {
    json: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
}

struct CheckOptions<'a> {
    json: bool,
    use_core: bool,
    path: Option<&'a str>,
    interfaces: Vec<&'a str>,
}

fn parse_check_args(args: &[String]) -> CheckOptions<'_> {
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
            if let Some(interface) = args.get(index) {
                interfaces.push(interface.as_str());
            }
        } else if path.is_none() {
            path = Some(arg.as_str());
        }
        index += 1;
    }

    CheckOptions {
        json,
        use_core,
        path,
        interfaces,
    }
}

fn parse_lower_args(args: &[String]) -> LowerOptions<'_> {
    let mut emit_rust = false;
    let mut path = None;
    let mut out_dir = None;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--rust" {
            emit_rust = true;
        } else if arg == "--out-dir" {
            index += 1;
            out_dir = args.get(index).map(String::as_str);
        } else if path.is_none() {
            path = Some(arg.as_str());
        }
        index += 1;
    }

    LowerOptions {
        emit_rust,
        path,
        out_dir,
    }
}

fn parse_run_args(args: &[String]) -> RunOptions<'_> {
    let mut json = false;
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
        } else if arg == "--out-dir" {
            index += 1;
            out_dir = args.get(index).map(String::as_str);
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            program_args.push(arg.as_str());
        }
        index += 1;
    }

    RunOptions {
        json,
        path,
        out_dir,
        program_args,
    }
}

fn parse_verify_args(args: &[String]) -> VerifyOptions<'_> {
    let mut json = false;
    let mut path = None;
    let mut out_dir = None;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--out-dir" {
            index += 1;
            out_dir = args.get(index).map(String::as_str);
        } else if path.is_none() {
            path = Some(arg.as_str());
        }
        index += 1;
    }

    VerifyOptions {
        json,
        path,
        out_dir,
    }
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
    let path = env::current_dir()
        .map_err(|error| format!("failed to read current directory: {error}"))?
        .join("runtime");
    if !path.join("Cargo.toml").is_file() {
        return Err(format!(
            "failed to locate rsscript-runtime crate at {}",
            path.display()
        ));
    }
    path.canonicalize()
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))
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
    env::temp_dir().join(format!(
        "{prefix}-{package_name}-{}-{now}",
        std::process::id()
    ))
}

fn cleanup_temp_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

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
    Invalid,
}

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
    Invalid,
}

fn parse_package_args(args: &[String]) -> PackageCommand<'_> {
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
            let Some(path) = args.get(index + 1) else {
                return PackageCommand::Invalid;
            };
            from_path = Some(path.as_str());
            index += 1;
        } else if arg == "--registry" {
            let Some(path) = args.get(index + 1) else {
                return PackageCommand::Invalid;
            };
            registry_path = Some(path.as_str());
            index += 1;
        } else if arg == "--to" {
            let Some(path) = args.get(index + 1) else {
                return PackageCommand::Invalid;
            };
            to_path = Some(path.as_str());
            index += 1;
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
        (["check"], [], None, None) => PackageCommand::Check { json, path: "." },
        (["check"], [path], None, None) => PackageCommand::Check { json, path },
        (["review"], [path], None, None) => PackageCommand::Review { json, path },
        (["review", "update"], [], Some(old_lock_path), Some(new_lock_path)) => {
            PackageCommand::ReviewUpdate {
                json,
                old_lock_path,
                new_lock_path,
            }
        }
        (["lock"], [path], None, None) => PackageCommand::Lock { json, path },
        (["tree"], [], None, None) => PackageCommand::Tree { json, path: "." },
        (["tree"], [path], None, None) => PackageCommand::Tree { json, path },
        (["publish"], [], None, None) => PackageCommand::Publish {
            json,
            dry_run,
            path: ".",
            registry: registry_path,
        },
        (["publish"], [path], None, None) => PackageCommand::Publish {
            json,
            dry_run,
            path,
            registry: registry_path,
        },
        (["vendor"], [], None, None) => PackageCommand::Vendor {
            json,
            dry_run,
            path: ".",
        },
        (["vendor"], [path], None, None) => PackageCommand::Vendor {
            json,
            dry_run,
            path,
        },
        (["metadata"], [], None, None) => PackageCommand::Metadata {
            json,
            dry_run,
            path: ".",
        },
        (["metadata"], [path], None, None) => PackageCommand::Metadata {
            json,
            dry_run,
            path,
        },
        (["diff"], [old_path, new_path], None, None) => PackageCommand::Diff {
            json,
            old_path,
            new_path,
        },
        _ => PackageCommand::Invalid,
    }
}

fn parse_review_args(args: &[String]) -> ReviewCommand<'_> {
    let mut json = false;
    let mut command = None;
    let mut paths = Vec::new();

    for arg in args {
        if arg == "--json" {
            json = true;
        } else if arg == "--diff" || arg == "--map" {
            command = Some(arg.as_str());
        } else {
            paths.push(arg.as_str());
        }
    }

    match (command, paths.as_slice()) {
        (Some("--map"), [path]) => ReviewCommand::Map { json, path },
        (Some("--diff") | None, [old_path, new_path]) => ReviewCommand::Diff {
            json,
            old_path,
            new_path,
        },
        _ => ReviewCommand::Invalid,
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
    let sources = match read_review_map_sources(path) {
        Ok(sources) => sources,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let diagnostics = sources
        .iter()
        .flat_map(|source| analyze_source(&source.path, &source.contents))
        .collect::<Vec<_>>();
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        print_diagnostics(json, &diagnostics);
        return ExitCode::from(1);
    }

    let source_refs = sources
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect();
    let map = review_map_sources(source_refs);
    if json {
        println!("{}", format_review_map_json(&map));
    } else {
        print!("{}", format_review_map_human(&map));
    }
    ExitCode::SUCCESS
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
        "  rsscript check [--json] [--core|--no-core] [--interface <file.rssi> ...] <file-or-package-directory>"
    );
    eprintln!(
        "  rsscript lint [--json] [--core|--no-core] [--interface <file.rssi> ...] <file.rss>"
    );
    eprintln!("  rsscript check --explain <code>");
    eprintln!("  rsscript fmt <file.rss>");
    eprintln!("  rsscript lower --rust <file.rss>");
    eprintln!("  rsscript lower --rust <file.rss> --out-dir <directory>");
    eprintln!("  rsscript run [--json] <file-or-package-directory> [-- <args>...]");
    eprintln!(
        "  rsscript run [--json] <file-or-package-directory> --out-dir <directory> [-- <args>...]"
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
