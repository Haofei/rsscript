//! `rss fix` applies machine-applicable structured edits to source files.

use std::fs;
use std::process::Command;

#[cfg(feature = "execution")]
fn run_isolated_fixture(name: &str, source: &str) -> serde_json::Value {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let path = temp.path().join(name);
    fs::write(&path, source).expect("write isolated fixture");
    let output = Command::new(bin)
        .args(["run", "--json", path.to_str().expect("path is utf-8")])
        .output()
        .expect("isolated runner should execute");
    assert!(
        output.status.success(),
        "rss run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("runner emits an execution report")
}

#[cfg(feature = "execution")]
fn stable_runner_projection(report: &serde_json::Value) -> serde_json::Value {
    // Duration and Artifact identity are intentionally host/provenance-specific.
    // This checked-in projection freezes the report semantics the product
    // promises across the isolated parent/child protocol.
    serde_json::json!({
        "schema": report["schema"],
        "outcome": report["outcome"],
        "usage": report["usage"],
        "stdout": report["stdout"],
        "stderr": report["stderr"],
        "provider_call_count": report["provider_call_traces"].as_array().map(Vec::len),
        "diagnostic_count": report["diagnostics"].as_array().map(Vec::len),
    })
}

#[test]
fn top_level_help_succeeds_on_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("--help")
        .output()
        .expect("rss --help should run");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("usage:\n"));
    assert!(output.stderr.is_empty());
}

#[test]
fn unterminated_constants_cannot_hide_declarations_or_produce_artifacts() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("main.rss");
    for quote in ["\"", "\"\"\"", "$\""] {
        let source = format!(
            "fn main() -> Int {{ return 0 }}\nconst MESSAGE: String = {quote}oops\nfn broken() -> Int {{ return missing }}\n"
        );
        fs::write(&path, source).unwrap();
        let checked = Command::new(bin)
            .args(["check", "--json"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(checked.status.code(), Some(1), "{quote}: {checked:?}");
        let diagnostics: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
        assert!(!diagnostics.as_array().unwrap().is_empty());
        #[cfg(feature = "execution")]
        {
            let artifact = temp.path().join("main.rssbundle");
            let built = Command::new(bin)
                .arg("build")
                .arg(&path)
                .arg("--out")
                .arg(&artifact)
                .output()
                .unwrap();
            assert_eq!(built.status.code(), Some(1), "{quote}: {built:?}");
            assert!(!artifact.exists());
        }
    }
}

#[test]
fn generate_commands_emit_the_versioned_json_schemas() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let source = temp.path().join("prefix.rss");
    fs::write(&source, "fn main() -> Unit {\n").expect("fixture should write");
    let path = source.to_str().expect("path is utf-8");

    let status = Command::new(bin)
        .args(["generate", "prefix-status", "--json", path])
        .output()
        .expect("prefix status command should run");
    assert!(status.status.success(), "{:?}", status);
    let status: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("prefix status emits JSON");
    assert_eq!(status["schema"], "rsscript.generate.prefix_status.v1");
    assert_eq!(status["status"], "incomplete");
    assert_eq!(status["syntax_complete"], false);
    assert!(status["replace"]["start"].is_u64());
    assert!(status["terminals"].is_array());

    let continuations = Command::new(bin)
        .args([
            "generate",
            "continuations",
            "--json",
            "--no-core",
            "--max-names",
            "1",
            path,
        ])
        .output()
        .expect("continuations command should run");
    assert!(continuations.status.success(), "{:?}", continuations);
    let continuations: serde_json::Value =
        serde_json::from_slice(&continuations.stdout).expect("continuations emit JSON");
    assert_eq!(
        continuations["schema"],
        "rsscript.generate.continuations.v1"
    );
    assert!(continuations["current_terminal_completeness"].is_string());
    assert!(continuations["terminal_completeness"].is_string());
    assert!(continuations["name_completeness"].is_string());
    assert_eq!(continuations["status"], "incomplete");
    assert!(continuations["replace"]["start"].is_u64());
    assert!(continuations["replace"]["end"].is_u64());
    assert!(continuations["identity"]["session_id"].is_u64());
    assert!(continuations["identity"]["revision"].is_u64());
    assert!(continuations["identity"]["interface_revision"].is_u64());
    assert!(continuations["identity"]["source_bytes"].is_u64());
    assert!(
        continuations["names"]
            .as_array()
            .is_some_and(|names| names.len() <= 1)
    );
    assert!(continuations["total_discovered_names"].is_u64());
    assert!(continuations["truncated"].is_boolean());
}

#[test]
fn generate_no_core_changes_completion_and_semantic_validity() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let source = temp.path().join("core-prefix.rss");
    fs::write(&source, "fn main() -> Unit {\n    List.is_empty(").expect("fixture should write");
    let path = source.to_str().expect("path is utf-8");

    let run = |extra: &[&str]| {
        let mut args = vec!["generate", "continuations", "--json"];
        args.extend_from_slice(extra);
        args.push(path);
        let output = Command::new(bin)
            .args(args)
            .output()
            .expect("continuations command should run");
        assert!(output.status.success(), "{:?}", output);
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .expect("continuations emit JSON")
    };

    let with_core = run(&[]);
    assert!(
        with_core["names"]
            .as_array()
            .is_some_and(|names| { names.iter().any(|candidate| candidate["text"] == "list") })
    );

    let without_core = run(&["--no-core"]);
    assert_eq!(without_core["semantic_validity"], "invalid");
    assert_eq!(without_core["core_interfaces"], "without_core");
    assert!(
        !without_core["names"]
            .as_array()
            .is_some_and(|names| { names.iter().any(|candidate| candidate["text"] == "list") })
    );
}

#[test]
fn fix_write_resolves_missing_data_effects_to_a_clean_check() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let dir = temp.path();
    let file = dir.join("fixme.rss");
    // Four missing exclusive effects across three lines (one line has two), so
    // the test also exercises multi-edit-per-line application order. Default
    // `read` arguments intentionally do not produce fixes.
    fs::write(
        &file,
        concat!(
            "fn touch(left: mut List<Int>, right: mut List<Int>) -> Unit {\n",
            "    return Unit\n",
            "}\n",
            "fn main() -> Unit {\n",
            "    let mut left = List<Int>.new()\n",
            "    let mut right = List<Int>.new()\n",
            "    touch(left: left, right: right)\n",
            "    List.push(list: left, value: 1)\n",
            "    List.push(list: right, value: 2)\n",
            "    return Unit\n",
            "}\n",
        ),
    )
    .expect("fixture should write");
    let path = file.to_str().expect("path is utf-8");

    // Preview must not modify the file.
    let before = fs::read_to_string(&file).unwrap();
    let preview = Command::new(bin)
        .args(["fix", path])
        .output()
        .expect("rss fix preview runs");
    assert!(preview.status.success());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        before,
        "preview must not write"
    );

    // `--json` reports the applied edits structurally.
    let json_out = Command::new(bin)
        .args(["fix", "--json", path])
        .output()
        .expect("rss fix --json runs");
    let report: serde_json::Value =
        serde_json::from_slice(&json_out.stdout).expect("fix emits JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(
        report["applied"].as_array().map(Vec::len),
        Some(4),
        "four machine-applicable edits planned: {report}"
    );

    // `--write` applies the edits; a follow-up check must be clean.
    let write = Command::new(bin)
        .args(["fix", "--write", path])
        .output()
        .expect("rss fix --write runs");
    assert!(
        write.status.success(),
        "fix --write failed: {}",
        String::from_utf8_lossy(&write.stderr)
    );
    let fixed = fs::read_to_string(&file).unwrap();
    assert!(fixed.contains("left: mut left"), "fixed source:\n{fixed}");
    assert!(fixed.contains("right: mut right"), "fixed source:\n{fixed}");
    assert!(
        fixed.contains("List.push(list: mut left, value: 1)"),
        "fixed source:\n{fixed}"
    );
    assert!(
        fixed.contains("List.push(list: mut right, value: 2)"),
        "fixed source:\n{fixed}"
    );

    let check = Command::new(bin)
        .args(["check", path])
        .output()
        .expect("rss check runs");
    let check_out = String::from_utf8_lossy(&check.stdout);
    assert!(
        check_out.contains("ok"),
        "post-fix check not clean:\n{check_out}\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

/// A call site that already spells an effect, but the wrong one, must be
/// *rewritten* rather than prefixed.
///
/// The measured repair data shows models apply what a fix names, so a
/// machine-applicable edit that produces `take mut value` does not merely fail
/// to help — it hands back a file that no longer parses. Both wrong-effect
/// shapes are covered: a keyword that must become another keyword, and a
/// keyword in front of a `read` parameter, which is canonical by omission and
/// so must be deleted along with its trailing space.
#[test]
fn fix_write_rewrites_and_removes_wrong_call_site_effects() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let file = temp.path().join("wrong-effects.rss");
    fs::write(
        &file,
        concat!(
            "fn touch(target: mut List<Int>, note: read String) -> Unit {\n",
            "    return Unit\n",
            "}\n",
            "fn main() -> Unit {\n",
            "    let mut items = List<Int>.new()\n",
            "    touch(target: take items, note: mut \"hello\")\n",
            "    return Unit\n",
            "}\n",
        ),
    )
    .expect("fixture should write");
    let path = file.to_str().expect("path is utf-8");

    let json_out = Command::new(bin)
        .args(["fix", "--json", path])
        .output()
        .expect("rss fix --json runs");
    let report: serde_json::Value =
        serde_json::from_slice(&json_out.stdout).expect("fix emits JSON");
    assert_eq!(
        report["applied"].as_array().map(Vec::len),
        Some(2),
        "both wrong effects carry an edit: {report}"
    );

    let write = Command::new(bin)
        .args(["fix", "--write", path])
        .output()
        .expect("rss fix --write runs");
    assert!(
        write.status.success(),
        "fix --write failed: {}",
        String::from_utf8_lossy(&write.stderr)
    );
    let fixed = fs::read_to_string(&file).unwrap();
    assert!(
        fixed.contains("touch(target: mut items, note: \"hello\")"),
        "fixed source:\n{fixed}"
    );

    let check = Command::new(bin)
        .args(["check", path])
        .output()
        .expect("rss check runs");
    let check_out = String::from_utf8_lossy(&check.stdout);
    assert!(
        check_out.contains("ok"),
        "post-fix check not clean:\n{check_out}\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

/// A call written with another language's argument names is renamed to the
/// declared ones, and one rename clears both errors it caused.
///
/// A wrong label is charged twice — the label is unknown (`RS0203`) and the
/// parameter it should have filled is missing (`RS0204`) — so four errors here
/// are two edits.
#[test]
fn fix_write_renames_wrong_argument_labels_to_the_declared_ones() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let file = temp.path().join("wrong-labels.rss");
    fs::write(
        &file,
        concat!(
            "pub fn record(target: mut List<Int>, note: read String) -> Unit {\n",
            "    return Unit\n",
            "}\n",
            "fn main() -> Unit {\n",
            "    let mut items: List<Int> = List<Int>.new()\n",
            "    return record(tgt: mut items, text: \"hi\")\n",
            "}\n",
        ),
    )
    .expect("fixture should write");
    let path = file.to_str().expect("path is utf-8");

    let write = Command::new(bin)
        .args(["fix", "--write", path])
        .output()
        .expect("rss fix --write runs");
    assert!(
        write.status.success(),
        "fix --write failed: {}",
        String::from_utf8_lossy(&write.stderr)
    );
    let fixed = fs::read_to_string(&file).unwrap();
    assert!(
        fixed.contains("record(target: mut items, note: \"hi\")"),
        "fixed source:\n{fixed}"
    );

    let check = Command::new(bin)
        .args(["check", path])
        .output()
        .expect("rss check runs");
    let check_out = String::from_utf8_lossy(&check.stdout);
    assert!(
        check_out.contains("ok"),
        "post-fix check not clean:\n{check_out}\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[cfg(feature = "execution")]
#[test]
fn run_cli_defaults_to_the_isolated_verified_vm() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let file = temp.path().join("hello.rss");
    fs::write(
        &file,
        concat!(
            "fn main() -> String {\n",
            "    return \"hello VM\"\n",
            "}\n",
        ),
    )
    .expect("fixture should write");

    let output = Command::new(bin)
        .args(["run", file.to_str().expect("path is utf-8")])
        .output()
        .expect("rss run should execute through the isolated VM runner");

    assert!(
        output.status.success(),
        "rss run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello VM\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[cfg(feature = "execution")]
#[test]
fn structured_async_example_has_a_stable_isolated_runner_report() {
    let report = run_isolated_fixture(
        "structured-async-runner.rss",
        include_str!("../../../examples/structured-async-pipeline/script/isolated.rss"),
    );
    let projection = stable_runner_projection(&report);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/structured-async-pipeline/fixtures/isolated-runner.report.json"
    ))
    .expect("checked-in runner report projection is valid JSON");
    assert_eq!(projection, expected);
}

#[cfg(feature = "execution")]
#[test]
fn embedded_report_example_has_a_stable_isolated_runner_report() {
    let report = run_isolated_fixture(
        "embedded-report-runner.rss",
        include_str!("../../../examples/embedded-report-pipeline/script/isolated.rss"),
    );
    let projection = stable_runner_projection(&report);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/embedded-report-pipeline/fixtures/isolated-runner.report.json"
    ))
    .expect("checked-in runner report projection is valid JSON");
    assert_eq!(projection, expected);
}

#[cfg(feature = "execution")]
#[test]
fn artifact_bundle_verify_run_and_semantic_diff_form_one_cli_workflow() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir");
    let old = temp.path().join("old.rss");
    let new = temp.path().join("new.rss");
    let bundle = temp.path().join("old.rssbundle");
    fs::write(&old, "fn main() -> Int { return 1 }\n").unwrap();
    fs::write(&new, "fn main() -> Int { return 2 }\n").unwrap();

    let build = Command::new(bin)
        .args([
            "build",
            "--out",
            bundle.to_str().unwrap(),
            old.to_str().unwrap(),
        ])
        .output()
        .expect("build bundle");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        Command::new(bin)
            .args(["verify", bundle.to_str().unwrap()])
            .status()
            .expect("verify bundle")
            .success()
    );

    let run = Command::new(bin)
        .args(["run", bundle.to_str().unwrap()])
        .output()
        .expect("run bundle");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "1\n");

    let diff = Command::new(bin)
        .args([
            "diff",
            "--json",
            old.to_str().unwrap(),
            new.to_str().unwrap(),
        ])
        .output()
        .expect("semantic diff");
    assert!(
        diff.status.success(),
        "{}",
        String::from_utf8_lossy(&diff.stderr)
    );
    let diff: serde_json::Value = serde_json::from_slice(&diff.stdout).expect("diff JSON");
    assert_eq!(diff["schema"], "rsscript.semantic_diff.v2");
    assert_ne!(diff["old"]["module_digest"], diff["new"]["module_digest"]);
}

/// A build that fails at all writes nothing.
///
/// This is the end-to-end half of the promise: the ordering inside the
/// verify-then-write step is unit-tested in `cli::artifact` with a Bundle the
/// Artifact verifier refuses, which is the only honest way to reach that
/// refusal — `rss check` and `rss build` run the same compiler, so a source
/// program the checker accepts and the verifier rejects would be a compiler
/// bug rather than a fixture. What a source program can still prove is that a
/// refused build reports the checker's diagnostic and leaves no file behind.
///
/// The refused program is a `let ... else` whose else block does not diverge.
#[cfg(feature = "execution")]
#[test]
fn a_refused_build_reports_the_checker_and_writes_nothing() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir");
    let source = temp.path().join("main.rss");
    let bundle = temp.path().join("main.rssbundle");
    fs::write(
        &source,
        "fn unwrap(value: Option<String>) -> String {\n    let Some(inner) = value else {\n        let fallback = \"x\"\n    }\n    return inner\n}\n\nfn main() -> String {\n    return unwrap(value: Some(\"hi\"))\n}\n",
    )
    .expect("write fixture");

    let built = Command::new(bin)
        .args(["build", "--out", bundle.to_str().unwrap()])
        .arg(&source)
        .output()
        .expect("rss build should run");
    assert_eq!(built.status.code(), Some(1), "{built:?}");
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(
        stderr.contains("error[RS0020]"),
        "build must report the checker's own diagnostic: {stderr}"
    );
    assert!(
        !bundle.exists(),
        "a refused build must not leave a bundle behind"
    );
}

/// The other half of the same promise: what `rss build` does write is a bundle
/// `rss verify` accepts.
#[cfg(feature = "execution")]
#[test]
fn build_writes_a_bundle_that_verify_accepts() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir");
    let source = temp.path().join("main.rss");
    let bundle = temp.path().join("main.rssbundle");
    fs::write(
        &source,
        "fn unwrap(value: Option<String>) -> String {\n    let Some(inner) = value else {\n        return \"default\"\n    }\n    return inner\n}\n\nfn main() -> String {\n    return unwrap(value: None)\n}\n",
    )
    .expect("write fixture");

    let built = Command::new(bin)
        .args(["build", "--out", bundle.to_str().unwrap()])
        .arg(&source)
        .output()
        .expect("rss build should run");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let verified = Command::new(bin)
        .args(["verify", bundle.to_str().unwrap()])
        .output()
        .expect("rss verify should run");
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
}

/// A program that uses the standard package interfaces (`Channel`, `Sender`,
/// `Receiver`, `Output`) and names no host interface of its own.
#[cfg(feature = "execution")]
const CHANNEL_PROGRAM: &str = r#"async fn produce(sender: Sender<Int>) -> Result<Unit, ChannelError> {
    local first = 10
    await Sender.send(sender, value: take first)?
    return Ok(Unit)
}

async fn consume(receiver: Receiver<Int>) -> Result<Int, ChannelError> {
    let a = await Receiver.recv(receiver)?
    return Ok(2)
}

fn main() -> Result<Unit, ChannelError> {
    let mut channel = Channel.bounded<Int>(capacity: 4)?
    let sender = Channel.sender(channel)
    let receiver = Channel.receiver(channel: mut channel)?

    task_group {
        async let producer = produce(sender)
        async let consumer = consume(receiver)

        await producer?
        let count = await consumer?
    }

    Output.write(message: "channel pipeline ok")
    return Ok(Unit)
}
"#;

/// `rss check` attached the language's standard package interfaces while
/// `rss build`/`rss run` attached none, so a channel program checked clean and
/// then failed to build with sixteen diagnostics. Every command now assembles
/// its interfaces in one place, so this needs no `--interface` anywhere.
#[cfg(feature = "execution")]
#[test]
fn check_build_and_run_share_one_standard_package_interface_assembly() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir");
    let source = temp.path().join("main.rss");
    let bundle = temp.path().join("main.rssbundle");
    fs::write(&source, CHANNEL_PROGRAM).expect("write fixture");

    let checked = Command::new(bin)
        .arg("check")
        .arg(&source)
        .output()
        .expect("rss check should run");
    assert!(
        checked.status.success(),
        "check: {}",
        String::from_utf8_lossy(&checked.stdout)
    );

    let built = Command::new(bin)
        .args(["build", "--out", bundle.to_str().unwrap()])
        .arg(&source)
        .output()
        .expect("rss build should run");
    assert!(
        built.status.success(),
        "build: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(bundle.exists(), "a successful build must write its bundle");

    let ran = Command::new(bin)
        .args(["run", "--trusted-in-process", "--json"])
        .arg(&source)
        .output()
        .expect("rss run should run");
    assert!(
        ran.status.success(),
        "run: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    let inspected = Command::new(bin)
        .args(["inspect", "imports"])
        .arg(&source)
        .output()
        .expect("rss inspect should run");
    assert!(
        inspected.status.success(),
        "inspect: {}",
        String::from_utf8_lossy(&inspected.stderr)
    );
}

/// The other side of the shared assembly: a host interface is *not* implied by
/// any command. All four refuse the program without `--interface` and all four
/// get past the frontend with it.
#[cfg(feature = "execution")]
#[test]
fn a_host_interface_is_required_by_check_build_run_and_inspect() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir");
    let source = temp.path().join("main.rss");
    let interface = temp.path().join("host.rssi");
    let bundle = temp.path().join("main.rssbundle");
    fs::write(&source, "fn main() -> Int {\n    return Host.value()\n}\n").expect("write fixture");
    fs::write(&interface, "module Host\n\npub fn value() -> Int\n").expect("write interface");
    let source = source.to_str().unwrap();
    let interface = interface.to_str().unwrap();
    let bundle = bundle.to_str().unwrap();

    let commands: [&[&str]; 4] = [
        &["check"],
        &["build", "--out", bundle],
        &["run", "--trusted-in-process"],
        &["inspect", "imports"],
    ];
    for command in commands {
        let without = Command::new(bin)
            .args(command)
            .arg(source)
            .output()
            .expect("command should run");
        assert!(
            !without.status.success(),
            "{command:?} must not resolve `Host.value` without `--interface`"
        );
        let reported = String::from_utf8_lossy(&without.stdout).into_owned()
            + &String::from_utf8_lossy(&without.stderr);
        assert!(
            reported.contains("RS0206"),
            "{command:?} must report the unresolved call: {reported}"
        );

        let with = Command::new(bin)
            .args(command)
            .args(["--interface", interface])
            .arg(source)
            .output()
            .expect("command should run");
        let reported = String::from_utf8_lossy(&with.stdout).into_owned()
            + &String::from_utf8_lossy(&with.stderr);
        assert!(
            !reported.contains("RS0206"),
            "{command:?} must accept the declared interface: {reported}"
        );
    }
}

/// `rss build` used to print `"compilation failed with N diagnostic(s)"`. It
/// now renders the same diagnostics `rss check` does, so a failing build tells
/// the caller what to fix.
#[cfg(feature = "execution")]
#[test]
fn check_and_build_report_the_same_diagnostics() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir");
    let source = temp.path().join("main.rss");
    let bundle = temp.path().join("main.rssbundle");
    fs::write(
        &source,
        "fn main() -> Int {\n    return missing_helper()\n}\n",
    )
    .expect("write fixture");

    let checked = Command::new(bin)
        .arg("check")
        .arg(&source)
        .output()
        .expect("rss check should run");
    assert_eq!(checked.status.code(), Some(1));
    let built = Command::new(bin)
        .args(["build", "--out", bundle.to_str().unwrap()])
        .arg(&source)
        .output()
        .expect("rss build should run");
    assert_eq!(built.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&built.stderr),
        String::from_utf8_lossy(&checked.stdout),
        "build must render exactly the diagnostics check renders"
    );

    let json = Command::new(bin)
        .args(["check", "--json"])
        .arg(&source)
        .output()
        .expect("rss check --json should run");
    let diagnostics: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("check --json emits JSON");
    let codes = diagnostics
        .as_array()
        .expect("diagnostics are an array")
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(!codes.is_empty(), "the program must be rejected");
    let build_output = String::from_utf8_lossy(&built.stderr);
    for code in codes {
        assert!(
            build_output.contains(&code),
            "build output must carry {code}: {build_output}"
        );
    }
    assert!(!bundle.exists(), "a failed build must write nothing");
}

/// Check `source` end to end through the real `rss check --json` entry point and
/// return the diagnostic codes it reports, in order.
fn check_diagnostic_codes(source: &str) -> Vec<String> {
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let path = temp.path().join("main.rss");
    fs::write(&path, source).expect("write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .expect("rss check should run");
    let diagnostics: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("rss check --json emits JSON");
    diagnostics
        .as_array()
        .expect("diagnostics are an array")
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Brace struct literals are accepted surface sugar that the parser desugars to
/// the canonical constructor call, so the checker only ever sees one form: an
/// accepted program and a rejected one must produce the same diagnostics in
/// either spelling.
#[test]
fn brace_struct_literals_type_check_exactly_like_constructor_calls() {
    let program = |literal: &str| {
        format!(
            "struct Report {{\n    title: String\n    count: Int\n}}\n\nfn build(title: take String, count: Int) -> Report {{\n    return {literal}\n}}\n"
        )
    };

    assert_eq!(
        check_diagnostic_codes(&program("Report(title: take title, count: count)")),
        Vec::<String>::new(),
        "the canonical constructor call must check cleanly"
    );
    assert_eq!(
        check_diagnostic_codes(&program("Report { title: take title, count: count }")),
        Vec::<String>::new(),
        "the brace struct literal must check exactly like the constructor call"
    );

    // The sugar is not a checker bypass: the same mistake is reported the same
    // way in both spellings.
    let canonical_errors = check_diagnostic_codes(&program(
        "Report(title: take title, count: count, extra: 1)",
    ));
    let sugared_errors = check_diagnostic_codes(&program(
        "Report { title: take title, count: count, extra: 1 }",
    ));
    assert!(
        !canonical_errors.is_empty(),
        "an unknown constructor field must be rejected"
    );
    assert_eq!(canonical_errors, sugared_errors);
}

/// A comma-terminated expression match arm is accepted sugar for the canonical
/// block arm, and reaches the checker as the same AST.
#[test]
fn expression_match_arms_check_exactly_like_block_arms() {
    let program = |arms: &str| {
        format!("fn classify(value: Int) -> Int {{\n    return match value {{\n{arms}    }}\n}}\n")
    };

    assert_eq!(
        check_diagnostic_codes(&program(
            "        0 => {\n            10\n        }\n        _ => {\n            20\n        }\n"
        )),
        Vec::<String>::new(),
        "the canonical block arm must check cleanly"
    );
    assert_eq!(
        check_diagnostic_codes(&program("        0 => 10,\n        _ => 20,\n")),
        Vec::<String>::new(),
        "the expression arm must check exactly like the block arm"
    );
}

/// `RS0206` used to name only the failure, and the measured repair data shows
/// that is exactly the class a model cannot recover from: it persisted through
/// three repair turns nine times because nothing told it what to write instead.
/// The diagnostic now names in-scope replacements, and a pure rename carries a
/// machine-applicable edit that `rss fix` applies.
#[test]
fn unresolved_calls_suggest_in_scope_names_and_rename_fixes_apply() {
    let source = concat!(
        "fn report(count: Int) -> Unit {\n",
        "    print(String.from_int(value: count))\n",
        "    return Unit\n",
        "}\n",
        "\n",
        "fn parse(raw: String) -> Option<Int> {\n",
        "    return Int.parse(value: raw)\n",
        "}\n",
        "\n",
        "fn size(values: List<Int>) -> Int {\n",
        "    return List.lenn(list: values)\n",
        "}\n",
    );

    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let path = temp.path().join("main.rss");
    fs::write(&path, source).expect("write fixture");

    let checked = Command::new(env!("CARGO_BIN_EXE_rss"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .expect("rss check should run");
    let diagnostics: serde_json::Value =
        serde_json::from_slice(&checked.stdout).expect("rss check --json emits JSON");
    let diagnostics = diagnostics.as_array().expect("diagnostics are an array");
    assert_eq!(diagnostics.len(), 3, "{diagnostics:#?}");

    // The help names the whole call — labels, call-site effects, return type —
    // not just the function. The edit still replaces the callee with the bare
    // name, so the fix contract is unchanged.
    for (diagnostic, (expected, signature)) in diagnostics.iter().zip([
        ("Output.write", "Output.write(message: String) -> Unit"),
        (
            "String.parse_int",
            "String.parse_int(value: String) -> Option<Int>",
        ),
        ("List.len", "List.len<T>(list: List<T>) -> Int"),
    ]) {
        assert_eq!(diagnostic["code"], "RS0206");
        let fix = diagnostic["fixes"]
            .as_array()
            .and_then(|fixes| fixes.iter().find(|fix| fix["kind"] == "rename_callee"))
            .unwrap_or_else(|| panic!("RS0206 must carry a rename fix: {diagnostic:#?}"));
        assert!(
            fix["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("Did you mean") && title.contains(expected)),
            "help must name `{expected}`: {fix:#?}"
        );
        assert!(
            fix["title"]
                .as_str()
                .is_some_and(|title| title.contains(signature)),
            "help must carry the whole signature `{signature}`: {fix:#?}"
        );
        assert_eq!(fix["applicability"], "machine-applicable");
        assert_eq!(fix["edit"]["replacement"], expected);
    }

    let fixed = Command::new(env!("CARGO_BIN_EXE_rss"))
        .args(["fix", "--write"])
        .arg(&path)
        .output()
        .expect("rss fix should run");
    assert!(
        fixed.status.success(),
        "{}",
        String::from_utf8_lossy(&fixed.stderr)
    );

    let rewritten = fs::read_to_string(&path).expect("read fixed source");
    assert!(rewritten.contains("Output.write(String.from_int(value: count))"));
    assert!(rewritten.contains("return String.parse_int(value: raw)"));
    assert!(rewritten.contains("return List.len(list: values)"));
    assert!(
        check_diagnostic_codes(&rewritten)
            .iter()
            .all(|code| code != "RS0206"),
        "every unresolved call must be gone after the renames"
    );
}

/// A receiver spelling still gets the real name, but substituting it also moves
/// the receiver into a named argument, so the fix stays advice rather than
/// claiming a rewrite it cannot perform.
#[test]
fn receiver_spelling_suggestions_are_advisory_not_machine_applicable() {
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let path = temp.path().join("main.rss");
    fs::write(
        &path,
        "fn read_port(value: JsonValue) -> Int {\n    return value.get_int(name: \"port\")\n}\n",
    )
    .expect("write fixture");

    let checked = Command::new(env!("CARGO_BIN_EXE_rss"))
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .expect("rss check should run");
    let diagnostics: serde_json::Value =
        serde_json::from_slice(&checked.stdout).expect("rss check --json emits JSON");
    let fix = diagnostics[0]["fixes"]
        .as_array()
        .and_then(|fixes| fixes.iter().find(|fix| fix["kind"] == "rename_callee"))
        .expect("receiver spelling still gets a suggestion");

    assert_eq!(diagnostics[0]["code"], "RS0206");
    assert!(
        fix["title"]
            .as_str()
            .is_some_and(|title| title.contains("Json.field_int")),
        "{fix:#?}"
    );
    assert_eq!(fix["applicability"], "maybe-incorrect");
    assert!(fix["edit"].is_null(), "advice must carry no edit: {fix:#?}");
}

#[cfg(feature = "native-jit")]
fn run_trusted_native_fixture(name: &str, source: &str) -> (bool, serde_json::Value) {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let path = temp.path().join(name);
    fs::write(&path, source).expect("write trusted native fixture");
    let output = Command::new(bin)
        .args([
            "run",
            "--trusted-in-process",
            "--native",
            "--json",
            path.to_str().expect("path is utf-8"),
        ])
        .output()
        .expect("trusted native run should execute");
    let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "trusted native run must emit an execution report ({error}):\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), report)
}

/// The native path's `--json` report must be a report, not just report-shaped
/// JSON: `serde_json::from_str::<ExecutionReportV2>` has to accept exactly what
/// the CLI printed.
///
/// This used to be impossible. `ExecutionEngineTelemetryV2::Native` declared its
/// two nanosecond counters as `u128`, and serde's internally tagged enum buffers
/// the variant's content through `Content`, which has no 128-bit carrier — so a
/// native report could be written and never read back. The counters are `u64`
/// now, and the only engine variant the interpreter path can emit never covered
/// this, so the native path pins it here.
#[cfg(feature = "native-jit")]
#[test]
fn the_native_json_report_parses_as_the_typed_v2_contract() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let path = temp.path().join("native-report-parses.rss");
    fs::write(
        &path,
        "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 200000 { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total }\n",
    )
    .expect("write native fixture");
    let output = Command::new(bin)
        .args([
            "run",
            "--trusted-in-process",
            "--native",
            "--json",
            path.to_str().expect("path is utf-8"),
        ])
        .output()
        .expect("trusted native run should execute");
    assert!(
        output.status.success(),
        "rss run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = String::from_utf8(output.stdout).expect("the report is utf-8");
    let report = serde_json::from_str::<rsscript_runner_protocol::ExecutionReportV2>(&json)
        .unwrap_or_else(|error| panic!("the printed report must parse as v2 ({error}):\n{json}"));
    assert!(
        matches!(
            report.telemetry.engine,
            rsscript_runner_protocol::ExecutionEngineTelemetryV2::Native { .. }
        ),
        "the native path must report native engine telemetry: {:?}",
        report.telemetry.engine
    );
}

/// `--native` selects an accelerator, not a trust level: it keeps the same
/// default runner limit profile the interpreter path runs under instead of
/// replacing it with `RunLimits::unbounded_for_trusted_host()`. The profile's
/// 10,000,000-step budget is the one limit a pure scalar loop can reach without
/// allocating, and it is reachable only from inside generated code.
#[cfg(feature = "native-jit")]
#[test]
fn trusted_native_execution_keeps_the_default_runner_step_budget() {
    let source = "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 100000000 { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total }\n";
    let (success, report) = run_trusted_native_fixture("native-step-budget.rss", source);
    assert!(!success, "an over-budget run must not report success");
    assert_eq!(report["outcome"]["kind"], "failed");
    assert_eq!(
        report["outcome"]["reason"], "step_budget_exceeded",
        "`--native` must terminate on the runner profile's step budget: {}",
        report["outcome"]
    );
    assert_eq!(
        report["usage"]["steps_consumed"], 10_000_001_u64,
        "the reported step count must be the interpreter's, one past the budget"
    );
}

/// A normal program must still complete, produce the interpreter's result, and
/// actually reach the native tier under that same profile — the profile used to
/// refuse every whole-function and OSR region because it arms
/// `intrinsic_call_budget` and a non-default `max_depth`.
///
/// The second shape is the one the profile used to shut out completely: a `main`
/// whose whole hot loop lives in a called helper. `main` declines whole-function
/// native entry because its body contains a call while the profile arms the
/// memory controls, and the tier-0 executor used to run the helper inside
/// `main`'s own frame, so the helper was never offered to the native tier. The
/// CLI runs with telemetry collection off, so this pins the engine and identical
/// usage; `a_called_hot_helper_reaches_native_under_the_default_runner_limits`
/// in `crates/rsscript-sdk/tests/native_jit_differential.rs` pins
/// `native_calls + osr_entries > 0` for the same source under the same profile.
#[cfg(feature = "native-jit")]
#[test]
fn trusted_native_execution_still_engages_under_the_default_runner_limits() {
    for (name, source) in [
        (
            "native-engages",
            "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 200000 { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total }\n",
        ),
        (
            "native-engages-called-helper",
            "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total }\nfn main() -> Int { return hot(limit: 200000) }\n",
        ),
    ] {
        let (success, native) = run_trusted_native_fixture(&format!("{name}.rss"), source);
        assert!(
            success,
            "{name}: a normal program must complete: {}",
            native["outcome"]
        );
        assert_eq!(native["telemetry"]["engine"]["kind"], "native");

        let bin = env!("CARGO_BIN_EXE_rss");
        let temp = tempfile::tempdir().expect("temp dir should be creatable");
        let path = temp.path().join(format!("{name}-interpreted.rss"));
        fs::write(&path, source).expect("write interpreter fixture");
        let output = Command::new(bin)
            .args([
                "run",
                "--trusted-in-process",
                "--json",
                path.to_str().expect("path is utf-8"),
            ])
            .output()
            .expect("trusted interpreter run should execute");
        let interpreted: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("interpreter run emits a report");
        assert_eq!(
            native["outcome"], interpreted["outcome"],
            "{name}: `--native` must not change the outcome"
        );
        assert_eq!(
            native["usage"]["steps_consumed"], interpreted["usage"]["steps_consumed"],
            "{name}: `--native` must report the interpreter's step count"
        );
        assert_eq!(
            native["usage"]["intrinsic_calls"], interpreted["usage"]["intrinsic_calls"],
            "{name}: `--native` must report the interpreter's intrinsic-call count"
        );
        assert_eq!(
            native["usage"]["allocation_bytes_consumed"],
            interpreted["usage"]["allocation_bytes_consumed"],
            "{name}: `--native` must report the interpreter's allocation bytes"
        );
    }
}
