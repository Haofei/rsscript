// ---------------------------------------------------------------------------
// Package-contract parity — RS1301 is not a single-source checker diagnostic.
// The self-hosted checker covers functions plus the package data-model and
// protocol declarations that production treats as one RS1301 contract surface.
// ---------------------------------------------------------------------------

fn compile_package_contract_checker() -> Result<RegVmExecutable, String> {
    compile_selfhost_tool("package_contract.rss", "package contract checker")
}

fn parse_package_contract_output(stdout: &str) -> Result<Vec<String>, String> {
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    match lines.as_slice() {
        ["CLEAN"] => Ok(Vec::new()),
        [code::PACKAGE_INTERFACE_MISMATCH] => {
            Ok(vec![code::PACKAGE_INTERFACE_MISMATCH.to_string()])
        }
        [] => Err("rss package contract checker emitted no verdict".to_string()),
        _ => Err(format!(
            "rss package contract checker emitted malformed output: {lines:?}"
        )),
    }
}

fn run_package_contract_checker(
    exe: &RegVmExecutable,
    interface_source: &str,
    source: &str,
) -> Result<Vec<String>, String> {
    run_package_contract_checker_with_native(exe, interface_source, source, "")
}

fn run_package_contract_checker_with_native(
    exe: &RegVmExecutable,
    interface_source: &str,
    source: &str,
    native_bindings: &str,
) -> Result<Vec<String>, String> {
    let output = exe
        .eval_main_with_args([
            interface_source.to_string(),
            source.to_string(),
            native_bindings.to_string(),
        ])
        .map_err(|e| format!("rss package contract checker failed to run: {e:?}"))?;
    parse_package_contract_output(&output.stdout)
}

fn selfhost_unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

fn write_package_contract_fixture(
    dir: &Path,
    interface_source: &str,
    source: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(dir.join("interface"))
        .map_err(|e| format!("cannot create interface dir under {}: {e}", dir.display()))?;
    std::fs::create_dir_all(dir.join("src"))
        .map_err(|e| format!("cannot create src dir under {}: {e}", dir.display()))?;
    std::fs::write(
        dir.join("rsspkg.toml"),
        "[package]\nname = \"selfhost-contract-parity\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[interfaces]\npaths = [\"interface\"]\n",
    )
    .map_err(|e| format!("cannot write package manifest under {}: {e}", dir.display()))?;
    std::fs::write(dir.join("interface/lib.rssi"), interface_source).map_err(|e| {
        format!(
            "cannot write package interface under {}: {e}",
            dir.display()
        )
    })?;
    std::fs::write(dir.join("src/lib.rss"), source)
        .map_err(|e| format!("cannot write package source under {}: {e}", dir.display()))?;
    Ok(())
}

fn package_contract_oracle_codes(interface_source: &str, source: &str) -> Vec<String> {
    package_contract_oracle_codes_with_native(interface_source, source, &[])
}

fn package_contract_oracle_codes_with_native(
    interface_source: &str,
    source: &str,
    native_bindings: &[(&str, &str)],
) -> Vec<String> {
    let dir = selfhost_unique_temp_dir("rss-selfhost-package-contract");
    write_package_contract_fixture(&dir, interface_source, source)
        .expect("package contract fixture should be writable");
    if !native_bindings.is_empty() {
        std::fs::create_dir_all(dir.join("native"))
            .expect("native binding directory should be writable");
        let mut manifest = String::from("[bindings]\n");
        for (symbol, target) in native_bindings {
            manifest.push_str(&format!("\"{symbol}\" = \"{target}\"\n"));
        }
        std::fs::write(dir.join("native/bindings.rssbind.toml"), manifest)
            .expect("native binding manifest should be writable");
    }
    let review = review_package_dir(&dir).expect("package review should succeed");
    let _ = std::fs::remove_dir_all(&dir);
    let mut codes = review
        .diagnostics
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.severity == Severity::Error
                && diagnostic.code == code::PACKAGE_INTERFACE_MISMATCH
        })
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    codes
}

fn package_contract_oracle_bundle_codes(
    interface_sources: &[(&str, &str)],
    source_files: &[(&str, &str)],
) -> Vec<String> {
    let dir = selfhost_unique_temp_dir("rss-selfhost-package-contract-bundle");
    std::fs::create_dir_all(dir.join("interface"))
        .expect("package interface directory should be writable");
    std::fs::create_dir_all(dir.join("src")).expect("package source directory should be writable");
    std::fs::write(
        dir.join("rsspkg.toml"),
        "[package]\nname = \"selfhost-contract-bundle\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[interfaces]\npaths = [\"interface\"]\n",
    )
    .expect("package manifest should be writable");
    for (path, contents) in interface_sources {
        std::fs::write(dir.join("interface").join(path), contents)
            .expect("package interface file should be writable");
    }
    for (path, contents) in source_files {
        std::fs::write(dir.join("src").join(path), contents)
            .expect("package source file should be writable");
    }
    let review = review_package_dir(&dir).expect("package bundle review should succeed");
    let _ = std::fs::remove_dir_all(&dir);
    let mut codes = review
        .diagnostics
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.severity == Severity::Error
                && diagnostic.code == code::PACKAGE_INTERFACE_MISMATCH
        })
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    codes
}

fn join_package_contract_sources(files: &[(&str, &str)]) -> String {
    let mut joined = String::new();
    for (_, contents) in files {
        joined.push_str(contents);
        if !contents.ends_with('\n') {
            joined.push('\n');
        }
    }
    joined
}

#[test]
fn package_contract_function_rs1301_parity_smoke() {
    let cases = [
        (
            "matching function",
            "pub fn render(body: read String) -> String\n",
            "pub fn render(body: read String) -> String {\n    return body\n}\n",
        ),
        (
            "missing implementation",
            "pub fn render(body: read String) -> String\n",
            "fn helper(body: read String) -> String {\n    return body\n}\n",
        ),
        (
            "signature mismatch",
            "pub fn render(body: read String) -> fresh String\n    effects(no_panic)\n",
            "pub fn render(body: read String) -> String {\n    return body\n}\n",
        ),
    ];
    let exe = compile_package_contract_checker().expect("rss package checker should compile");
    for (name, interface_source, source) in cases {
        let oracle = package_contract_oracle_codes(interface_source, source);
        let actual = run_package_contract_checker(&exe, interface_source, source)
            .expect("rss package contract checker should run");
        assert_eq!(
            oracle, actual,
            "package contract parity diverged for {name}"
        );
    }
}

#[test]
fn package_contract_declaration_rs1301_parity() {
    let cases = [
        (
            "matching struct",
            "struct Config {\n    retries: Int\n}\n",
            "pub struct Config {\n    retries: Int\n}\n",
        ),
        (
            "struct field mismatch",
            "struct Config {\n    retries: Int\n}\n",
            "pub struct Config {\n    retries: String\n}\n",
        ),
        (
            "opaque struct hides fields",
            "opaque struct Config\n",
            "pub struct Config {\n    retries: Int\n}\n",
        ),
        (
            "opaque type still checks kind",
            "opaque struct Config\n",
            "pub resource Config {\n    retries: Int\n}\n",
        ),
        (
            "matching sum",
            "sum PackageError {\n    Io(code: Int),\n    Invalid\n}\n",
            "pub sum PackageError {\n    Io(code: Int),\n    Invalid\n}\n",
        ),
        (
            "sum variant mismatch",
            "sum PackageError {\n    Io(code: Int),\n    Invalid\n}\n",
            "pub sum PackageError {\n    Io(code: String),\n    Invalid\n}\n",
        ),
        (
            "matching alias and const",
            "type PackageName = String\nconst MAX_RETRIES: Int = 3\n",
            "pub type PackageName = String\npub const MAX_RETRIES: Int = 3\n",
        ),
        (
            "const mismatch",
            "const MAX_RETRIES: Int = 3\n",
            "pub const MAX_RETRIES: Int = 4\n",
        ),
        (
            "matching protocol",
            "protocol Writer {\n    fn write(self: mut Self, message: read String) -> Unit\n        effects(retains(message))\n}\n",
            "protocol Writer {\n    fn write(self: mut Self, message: read String) -> Unit\n        effects(retains(message))\n}\n",
        ),
        (
            "protocol mismatch",
            "protocol Writer {\n    fn write(self: mut Self, message: read String) -> Unit\n        effects(retains(message))\n}\n",
            "protocol Writer {\n    fn write(self: mut Self, message: read String) -> Unit\n}\n",
        ),
        (
            "protocol impl mismatch",
            "protocol Writer {\n    fn write(self: mut Self) -> Unit\n}\nstruct Buffer\nimpl Writer for Buffer {\n    write = Buffer.write\n}\n",
            "protocol Writer {\n    fn write(self: mut Self) -> Unit\n}\npub struct Buffer\nimpl Writer for Buffer {\n    write = Buffer.audit\n}\n",
        ),
    ];
    let exe = compile_package_contract_checker().expect("rss package checker should compile");
    for (name, interface_source, source) in cases {
        let oracle = package_contract_oracle_codes(interface_source, source);
        let actual = run_package_contract_checker(&exe, interface_source, source)
            .expect("rss package contract checker should run");
        assert_eq!(
            oracle, actual,
            "package declaration contract parity diverged for {name}"
        );
    }
}

#[test]
fn package_contract_native_function_exemption_parity() {
    let interface_source = "features: native\n\nnative fn Native.echo(message: read String) -> String\n    effects(native)\n";
    let source = "fn helper() -> Unit {\n    return Unit\n}\n";
    let native_bindings = [("Native.echo", "rss_native::echo")];
    let oracle =
        package_contract_oracle_codes_with_native(interface_source, source, &native_bindings);
    let exe = compile_package_contract_checker().expect("rss package checker should compile");
    let actual =
        run_package_contract_checker_with_native(&exe, interface_source, source, "Native.echo")
            .expect("rss package contract checker should run");
    assert_eq!(oracle, actual, "native interface exemption diverged");
}

#[test]
fn package_contract_resolved_multifile_bundle_parity() {
    let interface_sources = [
        ("api.rssi", "fn render(body: read String) -> String\n"),
        (
            "model.rssi",
            "struct Config {\n    retries: Int\n}\ntype PackageName = String\n",
        ),
    ];
    let matching_sources = [
        (
            "api.rss",
            "pub fn render(body: read String) -> String {\n    return body\n}\n",
        ),
        (
            "model.rss",
            "pub struct Config {\n    retries: Int\n}\npub type PackageName = String\n",
        ),
    ];
    let missing_sources = [(
        "api.rss",
        "pub fn render(body: read String) -> String {\n    return body\n}\n",
    )];
    let exe = compile_package_contract_checker().expect("rss package checker should compile");
    let interface_bundle = join_package_contract_sources(&interface_sources);

    for (name, sources) in [
        ("matching bundle", matching_sources.as_slice()),
        ("missing model file", missing_sources.as_slice()),
    ] {
        let oracle = package_contract_oracle_bundle_codes(&interface_sources, sources);
        let source_bundle = join_package_contract_sources(sources);
        let actual = run_package_contract_checker(&exe, &interface_bundle, &source_bundle)
            .expect("rss package bundle checker should run");
        assert_eq!(
            oracle, actual,
            "resolved package bundle diverged for {name}"
        );
    }
}

