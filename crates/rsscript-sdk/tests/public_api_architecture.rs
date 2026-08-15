use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

const REMOVED_ROOT_ALIASES: &[&str] = &[
    "VmExecutable",
    "vm_compile_source",
    "eval_package_main_with_args",
    "eval_package_main_with_args_and_external_bindings",
    "eval_package_main_with_args_and_external_bindings_and_limits",
    "eval_package_main_with_args_and_external_bindings_streaming_stdout",
    "eval_source_main",
    "eval_source_main_with_args",
    "vm_eval_source_main_with_args",
    "eval_source_main_with_args_and_external_bindings",
    "eval_source_main_with_args_streaming_stdout",
];

fn library_source() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("rsscript library source should be readable")
}

fn inventory() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/architecture/sdk-api-inventory.md"),
    )
    .expect("SDK public API inventory should be readable")
}

fn api_snapshot() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/architecture/sdk-api-snapshot.v1.toml"),
    )
    .expect("SDK public API snapshot should be readable")
}

fn module_body<'a>(source: &'a str, module: &str) -> &'a str {
    let marker = format!("pub mod {module}");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("facade module `{module}` is missing"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("facade module `{module}` has no body"));
    let mut depth = 0usize;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("facade module `{module}` has an unclosed body");
}

fn normalized_public_uses(source: &str, module: &str) -> String {
    let body = module_body(source, module);
    let mut statements = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("pub use ") {
        let statement = &rest[start..];
        let end = statement
            .find(';')
            .unwrap_or_else(|| panic!("public use in `{module}` is not terminated"));
        statements.push(normalize_public_use(&statement[..=end]));
        rest = &statement[end + 1..];
    }
    // `pub use` declaration order does not change the public API either.
    statements.sort();
    format!("{}\n", statements.join("\n"))
}

/// Canonicalize source spelling without treating rustfmt's import ordering as
/// an API change. A façade export is a set of paths: whitespace, declaration
/// order, and the order of one brace group's members have no semver meaning.
///
/// The reviewed façade intentionally restricts itself to simple `pub use`
/// declarations, so this small parser is both bounded and more transparent
/// than hashing the full module text. A future nested import should fail the
/// snapshot review rather than silently acquire a different interpretation.
fn normalize_public_use(statement: &str) -> String {
    let statement = statement.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(open) = statement.find('{') else {
        return statement;
    };
    let close = statement
        .rfind('}')
        .expect("brace-group public use must close");
    assert_eq!(
        statement.matches('{').count(),
        1,
        "reviewed façade import groups must stay flat"
    );
    assert_eq!(
        statement.matches('}').count(),
        1,
        "reviewed façade import groups must stay flat"
    );
    let mut members = statement[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|member| !member.is_empty())
        .collect::<Vec<_>>();
    members.sort_unstable();
    format!(
        "{}{{{}}}{}",
        &statement[..open],
        members.join(","),
        &statement[close + 1..]
    )
}

fn snapshot_digest(source: &str, module: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(normalized_public_uses(source, module).as_bytes())
    )
}

fn snapshot_value<'a>(snapshot: &'a str, module: &str) -> Option<&'a str> {
    snapshot.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == module).then(|| value.trim().trim_matches('"'))
    })
}

/// Return the balanced body of a simple top-level Rust item. The reviewed SDK
/// keeps phase-state structs deliberately small, so a lightweight structural
/// guard makes their public construction boundary visible in normal CI without
/// adding a separate compile-fail fixture crate.
fn item_body<'a>(source: &'a str, declaration: &str) -> &'a str {
    let start = source
        .find(declaration)
        .unwrap_or_else(|| panic!("reviewed item `{declaration}` is missing"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("reviewed item `{declaration}` has no body"));
    let mut depth = 0usize;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("reviewed item `{declaration}` has an unclosed body");
}

#[test]
fn versioned_facade_is_deleted() {
    let source = library_source();
    assert!(!source.contains("pub mod api"));
    assert!(!source.contains("pub mod v1"));
}

#[test]
fn removed_root_aliases_cannot_return() {
    let source = library_source();
    let root_exports = source.as_str();
    let identifiers = root_exports
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    let violations = REMOVED_ROOT_ALIASES
        .iter()
        .filter(|alias| identifiers.contains(alias))
        .copied()
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "removed compatibility aliases were reintroduced at the crate root: {}",
        violations.join(", ")
    );
}

#[test]
fn public_api_inventory_covers_the_current_migration_surface() {
    let inventory = inventory();
    for required in [
        "## Stable façade",
        "## Compatibility-only APIs",
        "## Feature-gated experimental APIs",
        "`reg_vm_*`",
        "`native-jit`",
    ] {
        assert!(
            inventory.contains(required),
            "SDK API inventory must classify `{required}`"
        );
    }

    let source = library_source();
    for module in [
        "pub mod compile",
        "pub mod operation",
        "pub mod artifact",
        "pub mod provider_api",
        "pub mod runtime",
        "pub mod report",
        "pub mod analysis",
    ] {
        assert!(
            source.contains(module),
            "stable façade module `{module}` is missing"
        );
    }
    assert!(
        source.contains("pub use rsscript_compiler::FrontendInputSnapshot"),
        "the reviewed compile façade must expose the immutable frontend input boundary"
    );

    for forbidden in [
        "pub use rsscript_vm::JitPlan",
        "pub use rsscript_vm::RegInstr",
    ] {
        assert!(
            !source.contains(forbidden),
            "experimental VM detail must not enter the default SDK surface: `{forbidden}`"
        );
    }
    assert!(
        source.contains("#[cfg(feature = \"native-jit\")]\npub use rsscript_vm::NativeStats"),
        "native JIT statistics must remain feature-gated"
    );
    assert!(
        source.contains("#[cfg(feature = \"native-jit\")]\npub use vm_adapter"),
        "native JIT execution helpers must remain feature-gated"
    );
    for legacy_export in [
        "pub use rsscript_compiler::compatibility::{",
        "pub use rsscript_bytecode::{",
        "pub use rsscript_vm::{",
        "pub use vm_adapter::{",
    ] {
        let gated = format!("#[cfg(feature = \"compatibility\")]\n{legacy_export}");
        assert!(
            source.contains(&gated),
            "legacy root export must require the compatibility feature: {legacy_export}"
        );
    }
}

#[test]
fn reviewed_facade_exports_match_the_checked_api_snapshot() {
    let source = library_source();
    let snapshot = api_snapshot();
    assert!(
        snapshot.contains("schema = \"rsscript.sdk_api_snapshot.v1\""),
        "SDK API snapshot must declare its versioned schema"
    );

    let modules = [
        "language",
        "compile",
        "operation",
        "artifact",
        "provider_api",
        "runtime",
        "report",
        "analysis",
    ];
    let mut mismatches = Vec::new();
    for module in modules {
        let expected = snapshot_value(&snapshot, module)
            .unwrap_or_else(|| panic!("SDK API snapshot is missing façade module `{module}`"));
        let actual = snapshot_digest(&source, module);
        if actual != expected {
            mismatches.push(format!("{module} = \"{actual}\" (expected \"{expected}\")"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "reviewed façade exports changed; update sdk-api-inventory.md and the checked snapshot deliberately:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn default_snapshot_checks_share_the_compilation_session_query_boundary() {
    let source = library_source();
    assert!(
        source.contains("use rsscript_semantics::CompilationSession;"),
        "CompilationSession must be a base SDK dependency, not execution-only"
    );
    let check_snapshot = source
        .split("pub fn check_snapshot(&self, snapshot: &FrontendInputSnapshot) -> Vec<Diagnostic>")
        .nth(1)
        .and_then(|rest| rest.split("pub fn check_with_operation").next())
        .expect("reviewed snapshot-check implementation must exist");
    assert!(
        check_snapshot.contains("analyze_snapshot_with_session(snapshot, None)"),
        "the default SDK check must use the session-owned analysis query"
    );
    assert!(
        !check_snapshot.contains("#[cfg(feature = \"execution\")]"),
        "default SDK checks must not select a separate non-session analysis path"
    );
}

#[test]
fn reviewed_execution_report_hides_legacy_native_value_behind_compatibility() {
    let source = library_source();
    let report_start = source
        .find("pub struct ExecutionReport")
        .expect("ExecutionReport must remain a reviewed report type");
    let report = &source[report_start..];
    let public_native_value = "pub native_value: Option<NativeValue>";
    let offset = report
        .find(public_native_value)
        .expect("compatibility report projection must remain explicitly gated");
    let prefix = &report[..offset];
    assert!(
        prefix.ends_with("#[cfg(feature = \"compatibility\")]\n    "),
        "the legacy NativeValue report field must require the compatibility feature"
    );
    assert!(
        report.contains("legacy_native_value: Option<serde_json::Value>"),
        "the v1 JSON compatibility projection must stay private to the reviewed SDK"
    );
    assert!(
        source.contains("wire_value: Option<provider::WireValue>"),
        "reviewed execution results must expose the canonical typed wire projection"
    );
}

#[test]
fn reviewed_execution_report_has_one_terminal_outcome() {
    let source = library_source();
    let report_start = source
        .find("pub struct ExecutionReport")
        .expect("ExecutionReport must remain a reviewed report type");
    let report = &source[report_start..];
    assert!(report.contains("outcome: ExecutionOutcome"));
    for legacy_phase_field in [
        "pub termination_reason:",
        "pub value:",
        "pub display_value:",
        "pub failure:",
    ] {
        assert!(
            !report.contains(legacy_phase_field),
            "reviewed report must derive terminal evidence from ExecutionOutcome, not expose `{legacy_phase_field}`"
        );
    }
}

#[test]
fn reviewed_artifact_phases_have_private_non_optional_state_and_one_way_transitions() {
    let source = library_source();
    for (declaration, required_field) in [
        ("pub struct BuiltArtifact", "bundle: ArtifactBundle"),
        ("pub struct VerifiedArtifact", "bundle: ArtifactBundle"),
        ("pub struct AdmittedArtifact", "artifact: VerifiedArtifact"),
        (
            "pub struct LinkedArtifact<'artifact>",
            "artifact: &'artifact AdmittedArtifact",
        ),
    ] {
        let body = item_body(&source, declaration);
        assert!(
            body.contains(required_field),
            "{declaration} must retain its required phase state `{required_field}`"
        );
        assert!(
            !body.contains("pub "),
            "{declaration} must not expose fields that let callers forge or mutate a phase"
        );
        assert!(
            !body.contains("Option<"),
            "{declaration} must not encode a different artifact phase as optional state"
        );
    }

    let built = item_body(&source, "pub struct BuiltArtifact");
    assert!(
        !built.contains("VerifiedArtifact") && !built.contains("AdmittedArtifact"),
        "the built phase must not carry later phase representations"
    );
    let verified = item_body(&source, "pub struct VerifiedArtifact");
    assert!(
        !verified.contains("AdmittedArtifact") && !verified.contains("LinkedArtifact"),
        "the verified phase must not carry admission or linking state"
    );

    for required_transition in [
        "pub fn verify(&self, built: BuiltArtifact) -> Result<VerifiedArtifact, VerifyError>",
        "pub fn admit<P: ArtifactAdmissionPolicy>(",
        "pub fn admit_trusted_input(self) -> AdmittedArtifact",
        "artifact: &'artifact AdmittedArtifact",
        "pub fn execute(&self, request: ExecutionRequest) -> ExecutionReport",
    ] {
        assert!(
            source.contains(required_transition),
            "reviewed SDK phase transition is missing: `{required_transition}`"
        );
    }
}

#[test]
fn reviewed_execution_conveniences_cannot_bypass_the_phase_or_report_boundary() {
    let source = library_source();
    let runtime = item_body(&source, "impl Runtime");
    let linked = item_body(&source, "impl LinkedArtifact<'_>");

    assert!(
        runtime.contains(
            "pub fn link<'artifact>(\n        &self,\n        artifact: &'artifact AdmittedArtifact,\n    ) -> Result<LinkedArtifact<'artifact>, LinkError>",
        ),
        "the only reviewed link convenience must require admission and retain a distinct host LinkError"
    );
    assert!(
        linked.contains("pub fn execute(&self, request: ExecutionRequest) -> ExecutionReport"),
        "the reviewed execution convenience must always retain an ExecutionReport"
    );
    assert!(
        !linked.contains("pub fn execute(&self, request: ExecutionRequest) -> Result<"),
        "script/provider/budget termination must not escape through a result-returning execution convenience"
    );
    assert!(
        !linked.contains("pub fn run(") && !runtime.contains("pub fn run("),
        "the reviewed SDK must not grow a second execution convenience that bypasses the report boundary"
    );
}
