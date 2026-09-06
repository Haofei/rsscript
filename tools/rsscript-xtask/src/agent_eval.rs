//! Deterministic, offline scorer for the RSScript agent-evaluation corpus.
//!
//! This scorer deliberately does not invoke the CLI or any model service. It
//! reads task/candidate inputs, calls parser and semantic analyzer APIs in
//! process, and emits a versioned report suitable for a caller-owned runner.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rsscript_diagnostics::Severity;
use rsscript_semantics::{
    analyze_source_with_interfaces, semantic_completion, standard_package_interfaces,
};
use rsscript_syntax::{
    ExpectedTerminal, PrefixParseState, TerminalCompleteness, parse_source_prefix, parse_source_raw,
};
use serde::{Deserialize, Serialize};

const AGENT_EVAL_SCHEMA: &str = "rsscript.agent_eval.v1";
const TASK_SCHEMA: &str = "rsscript.eval.task.v1";
const CANDIDATE_SCHEMA: &str = "rsscript.eval.candidate.v1";
const REPORT_SCHEMA: &str = "rsscript.eval.report.v1";

pub(crate) fn run(
    workspace_root: &Path,
    arguments: impl Iterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    let options = parse_options(arguments)?;
    let tasks = resolve_path(workspace_root, &options.tasks);
    let candidates = resolve_path(workspace_root, &options.candidates);
    let output = resolve_path(workspace_root, &options.output);
    let report = score(&tasks, &candidates)?;
    fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!(
        "wrote {} task report(s) to {}",
        report.tasks.len(),
        output.display()
    );
    if report.aggregate.oracle_soundness.violation_count != 0 {
        return Err(format!(
            "agent-eval found {} oracle soundness violation(s)",
            report.aggregate.oracle_soundness.violation_count
        )
        .into());
    }
    Ok(())
}

#[derive(Debug)]
struct Options {
    tasks: PathBuf,
    candidates: PathBuf,
    output: PathBuf,
}

fn parse_options(arguments: impl Iterator<Item = String>) -> Result<Options, Box<dyn Error>> {
    let mut tasks = None;
    let mut candidates = None;
    let mut output = None;
    let values = arguments.collect::<Vec<_>>();
    let mut index = 0;
    while let Some(argument) = values.get(index) {
        let target = match argument.as_str() {
            "--tasks" => &mut tasks,
            "--candidates" => &mut candidates,
            "--output" => &mut output,
            _ => return Err(format!("unknown agent-eval argument `{argument}`").into()),
        };
        index += 1;
        let value = values
            .get(index)
            .ok_or_else(|| format!("missing value for `{argument}`"))?;
        if value.starts_with("--") {
            return Err(format!("missing value for `{argument}`").into());
        }
        if target.replace(PathBuf::from(value)).is_some() {
            return Err(format!("duplicate `{argument}`").into());
        }
        index += 1;
    }
    Ok(Options {
        tasks: tasks.ok_or("agent-eval requires --tasks")?,
        candidates: candidates.ok_or("agent-eval requires --candidates")?,
        output: output.ok_or("agent-eval requires --output")?,
    })
}

fn resolve_path(workspace_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    schema: String,
    id: String,
    version: u32,
    mode: TaskMode,
    #[serde(rename = "title")]
    _title: Option<String>,
    #[serde(rename = "prompt")]
    _prompt: String,
    candidate: PathBuf,
    interfaces: Vec<PathBuf>,
    expected: PathBuf,
    #[serde(default)]
    completion_probes: Vec<CompletionProbe>,
    #[serde(rename = "tags")]
    _tags: Option<Vec<String>>,
}

/// A small, explicit completion oracle fixture.  The scorer never treats an
/// intentionally-unrepaired task candidate as an oracle failure; only these
/// parser/semantic facts are checked for soundness.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionProbe {
    source: String,
    candidate: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TaskMode {
    Repair,
    Review,
}

/// How a candidate was generated. This describes runner provenance, not the
/// task's repair/review contract.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GenerationMode {
    PromptOnly,
    LanguageCard,
    RepairLoop,
    Constrained,
    OfflineFixture,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    task_id: String,
    candidate_check: ExpectedCheck,
    target_check: ExpectedCheck,
    invariants: Vec<Invariant>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedCheck {
    outcome: CheckOutcome,
    diagnostic_codes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum CheckOutcome {
    Pass,
    Fail,
    NotRun,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invariant {
    kind: String,
    value: String,
}

/// Optional sidecar for model-runner metadata and an alternate source file.
/// Source metadata is normalized before it reaches the report schema.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateManifest {
    schema: String,
    task_id: String,
    source: Option<PathBuf>,
    mode: Option<GenerationMode>,
    model: Option<String>,
    model_version: Option<String>,
    temperature: Option<f64>,
    attempt: Option<u64>,
    generation_tokens: Option<u64>,
    generation_duration_ms: Option<u64>,
    repair_turns: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct CandidateMetadata {
    task_id: String,
    task_mode: TaskMode,
    mode: GenerationMode,
    model: String,
    model_version: String,
    temperature: Option<f64>,
    attempt: u64,
    generation_tokens: u64,
    generation_duration_ms: u64,
    repair_turns: u64,
}

#[derive(Debug, Serialize)]
struct AgentEvalReport {
    schema: &'static str,
    provenance: EvalProvenance,
    tasks: Vec<TaskReport>,
    aggregate: Aggregate,
}

#[derive(Debug, Serialize)]
struct EvalProvenance {
    static_check_environment: &'static str,
}

#[derive(Debug, Serialize)]
struct TaskReport {
    schema: &'static str,
    candidate: CandidateMetadata,
    status: ScoreStatus,
    /// Static check of the task's fixed seed candidate.
    candidate_static_check: StaticCheck,
    parse: ParseCheck,
    static_check: StaticCheck,
    diagnostic_match: DiagnosticMatch,
    safety: SafetyFacts,
    invariants: Vec<InvariantResult>,
    /// Machine-readable account of every target-contract violation.  Unlike
    /// latency, this is a score fact and a nonzero aggregate exits the command.
    oracle_soundness: OracleSoundness,
    /// Nondeterministic measurement intentionally isolated from score facts.
    compiler_query_latency_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScoreStatus {
    Pass,
    Fail,
}

#[derive(Debug, Serialize)]
struct ParseCheck {
    outcome: CheckOutcome,
    unknown_top_level_count: usize,
    malformed_declaration_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct StaticCheck {
    outcome: CheckOutcome,
    diagnostic_codes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DiagnosticMatch {
    /// Match against the task's pre-repair candidate contract.
    candidate_check_matches: bool,
    /// Match against the repaired/review target contract.
    target_check_matches: bool,
    expected_diagnostics: Vec<String>,
    forbidden_diagnostics: Vec<String>,
    expected_diagnostics_match: bool,
    forbidden_diagnostics_match: bool,
}

#[derive(Debug, Serialize)]
struct SafetyFacts {
    unknown_tool_or_argument: bool,
    unknown_tool_or_argument_diagnostics: Vec<String>,
    invalid_mut_or_take: bool,
    invalid_mut_or_take_diagnostics: Vec<String>,
}

#[derive(Debug, Serialize)]
struct InvariantResult {
    kind: String,
    value: String,
    scope: InvariantScope,
    passed: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum InvariantScope {
    Candidate,
    Target,
}

#[derive(Debug, Serialize)]
struct OracleSoundness {
    prefixes_checked: usize,
    terminals_recommended: usize,
    terminals_accepted: usize,
    dead_after_insert: usize,
    semantic_candidates_checked: usize,
    violations: Vec<OracleViolation>,
}

#[derive(Debug, Serialize)]
struct OracleViolation {
    kind: &'static str,
    value: String,
}

#[derive(Debug, Serialize)]
struct Aggregate {
    task_count: usize,
    passed: usize,
    failed: usize,
    parse_passed: usize,
    static_check_passed: usize,
    candidate_check_matches: usize,
    target_check_matches: usize,
    expected_diagnostics_match: usize,
    forbidden_diagnostics_match: usize,
    unknown_tool_or_argument: usize,
    invalid_mut_or_take: usize,
    compiler_query_latency_ms: u64,
    oracle_soundness: OracleSoundnessAggregate,
}

#[derive(Debug, Serialize)]
struct OracleSoundnessAggregate {
    prefixes_checked: usize,
    terminals_recommended: usize,
    terminals_accepted: usize,
    dead_after_insert: usize,
    semantic_candidates_checked: usize,
    violation_count: usize,
    sound_tasks: usize,
}

fn score(tasks_path: &Path, candidates_path: &Path) -> Result<AgentEvalReport, Box<dyn Error>> {
    let task_paths = collect_task_paths(tasks_path)?;
    let evaluation_root = task_root(tasks_path)?;
    let mut tasks = task_paths
        .iter()
        .map(|path| score_task(path, &evaluation_root, candidates_path))
        .collect::<Result<Vec<_>, _>>()?;
    tasks.sort_by(|left, right| left.candidate.task_id.cmp(&right.candidate.task_id));
    let aggregate = aggregate(&tasks);
    Ok(AgentEvalReport {
        schema: AGENT_EVAL_SCHEMA,
        provenance: EvalProvenance {
            static_check_environment: "standard_package_interfaces_plus_task_interfaces",
        },
        tasks,
        aggregate,
    })
}

fn collect_task_paths(path: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut paths = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no task TOML files found in {}", path.display()).into());
    }
    Ok(paths)
}

fn task_root(tasks_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if tasks_path.is_dir() {
        return tasks_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "task directory must have an evaluation root".into());
    }
    tasks_path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "task file must be below an evaluation root".into())
}

fn score_task(
    task_path: &Path,
    evaluation_root: &Path,
    candidates_path: &Path,
) -> Result<TaskReport, Box<dyn Error>> {
    let task: Task = toml::from_str(&fs::read_to_string(task_path)?)?;
    if task.schema != TASK_SCHEMA || task.version != 1 {
        return Err(format!("{} has an unsupported task schema", task_path.display()).into());
    }
    let mut expected: Expected =
        serde_json::from_slice(&fs::read(evaluation_root.join(&task.expected))?)?;
    if expected.task_id != task.id {
        return Err(format!("{} expected task_id does not match", task_path.display()).into());
    }
    normalize_codes(&mut expected.candidate_check.diagnostic_codes);
    normalize_codes(&mut expected.target_check.diagnostic_codes);
    let (candidate_path, metadata) = candidate_input(&task, evaluation_root, candidates_path)?;
    let source = fs::read_to_string(&candidate_path)?;
    let interfaces = task
        .interfaces
        .iter()
        .map(|path| {
            let path = evaluation_root.join(path);
            fs::read_to_string(&path).map(|source| (path, source))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let interface_refs = interfaces
        .iter()
        .map(|(path, source)| (path.to_string_lossy(), source.as_str()))
        .collect::<Vec<_>>();
    // Match the default CLI checking universe: standard package contracts are
    // visible alongside each task's explicit interfaces.
    let mut analyzer_interfaces = standard_package_interfaces().to_vec();
    analyzer_interfaces.extend(
        interface_refs
            .iter()
            .map(|(path, source)| (path.as_ref(), *source)),
    );

    let baseline_path = evaluation_root.join(&task.candidate);
    let baseline_source = fs::read_to_string(&baseline_path)?;
    let parsed = parse_source_raw(&candidate_path.to_string_lossy(), &source);
    let parse = ParseCheck {
        outcome: if parsed.unknown_top_level_spans.is_empty()
            && parsed.malformed_declaration_spans.is_empty()
        {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail
        },
        unknown_top_level_count: parsed.unknown_top_level_spans.len(),
        malformed_declaration_count: parsed.malformed_declaration_spans.len(),
    };

    let start = Instant::now();
    let diagnostics = analyze_source_with_interfaces(
        &candidate_path.to_string_lossy(),
        &source,
        &analyzer_interfaces,
    );
    let compiler_query_latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let static_codes = diagnostic_codes(&diagnostics);
    let static_check = static_check_from_codes(static_codes.clone());
    let candidate_static_check = if baseline_path == candidate_path {
        static_check.clone()
    } else {
        let diagnostics = analyze_source_with_interfaces(
            &baseline_path.to_string_lossy(),
            &baseline_source,
            &analyzer_interfaces,
        );
        static_check_from_codes(diagnostic_codes(&diagnostics))
    };
    let invariant_results = expected
        .invariants
        .iter()
        .map(|invariant| evaluate_invariant(invariant, &baseline_source, &source, &static_codes))
        .collect::<Vec<_>>();
    let candidate_check_matches = check_matches(&expected.candidate_check, &candidate_static_check);
    let target_check_matches = check_matches(&expected.target_check, &static_check);
    let expected_diagnostics = expected.target_check.diagnostic_codes.clone();
    let mut forbidden_diagnostics = expected
        .invariants
        .iter()
        .filter(|invariant| invariant.kind == "diagnostic_absent")
        .map(|invariant| invariant.value.clone())
        .collect::<Vec<_>>();
    normalize_codes(&mut forbidden_diagnostics);
    let expected_diagnostics_match = expected_diagnostics
        .iter()
        .all(|code| static_codes.binary_search(code).is_ok());
    let forbidden_diagnostics_match = forbidden_diagnostics
        .iter()
        .all(|code| static_codes.binary_search(code).is_err());
    let unknown_tool_or_argument_diagnostics = static_codes
        .iter()
        .filter(|code| {
            matches!(
                code.as_str(),
                "RS0201" | "RS0203" | "RS0204" | "RS0205" | "RS0206"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let invalid_mut_or_take_diagnostics = static_codes
        .iter()
        .filter(|code| matches!(code.as_str(), "RS0202" | "RS0308" | "RS0310"))
        .cloned()
        .collect::<Vec<_>>();
    let safety = SafetyFacts {
        unknown_tool_or_argument: !unknown_tool_or_argument_diagnostics.is_empty(),
        unknown_tool_or_argument_diagnostics,
        invalid_mut_or_take: !invalid_mut_or_take_diagnostics.is_empty(),
        invalid_mut_or_take_diagnostics,
    };
    let status = if parse.outcome == CheckOutcome::Pass
        && target_check_matches
        && expected_diagnostics_match
        && forbidden_diagnostics_match
        && invariant_results
            .iter()
            .filter(|invariant| matches!(invariant.scope, InvariantScope::Target))
            .all(|invariant| invariant.passed)
    {
        ScoreStatus::Pass
    } else {
        ScoreStatus::Fail
    };
    let oracle_soundness = oracle_soundness(&task, &candidate_path.to_string_lossy());

    Ok(TaskReport {
        schema: REPORT_SCHEMA,
        candidate: metadata,
        status,
        candidate_static_check,
        parse,
        static_check,
        diagnostic_match: DiagnosticMatch {
            candidate_check_matches,
            target_check_matches,
            expected_diagnostics,
            forbidden_diagnostics,
            expected_diagnostics_match,
            forbidden_diagnostics_match,
        },
        safety,
        invariants: invariant_results,
        oracle_soundness,
        compiler_query_latency_ms,
    })
}

fn oracle_soundness(task: &Task, file: &str) -> OracleSoundness {
    let mut result = OracleSoundness {
        prefixes_checked: 0,
        terminals_recommended: 0,
        terminals_accepted: 0,
        dead_after_insert: 0,
        semantic_candidates_checked: 0,
        violations: Vec::new(),
    };
    for probe in &task.completion_probes {
        let prefix = parse_source_prefix(file, &probe.source);
        result.prefixes_checked = result.prefixes_checked.saturating_add(1);
        if !prefix.matches_source(&probe.source) {
            result.violations.push(OracleViolation {
                kind: "stale_prefix_identity",
                value: probe.candidate.clone(),
            });
            continue;
        }
        for terminal in &prefix.expected_terminals {
            let ExpectedTerminal::Fixed { text, completeness } = terminal else {
                continue;
            };
            if *completeness != TerminalCompleteness::Complete {
                continue;
            }
            result.terminals_recommended = result.terminals_recommended.saturating_add(1);
            let candidate = replace_prefix_terminal(&probe.source, &prefix, text);
            if parse_source_prefix(file, &candidate).state == PrefixParseState::Dead {
                result.dead_after_insert = result.dead_after_insert.saturating_add(1);
                result.violations.push(OracleViolation {
                    kind: "terminal_dead_after_replace",
                    value: (*text).to_string(),
                });
            } else {
                result.terminals_accepted = result.terminals_accepted.saturating_add(1);
            }
        }
        let semantic = semantic_completion(file, &probe.source, &prefix);
        let Some(candidate) = semantic
            .candidates
            .iter()
            .find(|candidate| candidate.name == probe.candidate)
        else {
            result.violations.push(OracleViolation {
                kind: "semantic_candidate_missing",
                value: probe.candidate.clone(),
            });
            continue;
        };
        result.semantic_candidates_checked = result.semantic_candidates_checked.saturating_add(1);
        let candidate_source =
            replace_prefix_terminal(&probe.source, &prefix, &candidate.insert_text);
        if parse_source_prefix(file, &candidate_source).state == PrefixParseState::Dead {
            result.dead_after_insert = result.dead_after_insert.saturating_add(1);
            result.violations.push(OracleViolation {
                kind: "semantic_candidate_dead_after_replace",
                value: candidate.name.clone(),
            });
        }
    }
    result
}

fn replace_prefix_terminal(
    source: &str,
    prefix: &rsscript_syntax::PrefixParseResult,
    text: &str,
) -> String {
    format!(
        "{}{}{}",
        &source[..prefix.replace_range.start],
        text,
        &source[prefix.replace_range.end..]
    )
}

fn candidate_input(
    task: &Task,
    evaluation_root: &Path,
    candidates_path: &Path,
) -> Result<(PathBuf, CandidateMetadata), Box<dyn Error>> {
    let manifest_path = candidates_path.join(format!("{}.json", task.id));
    let manifest = manifest_path
        .is_file()
        .then(|| -> Result<CandidateManifest, Box<dyn Error>> {
            Ok(serde_json::from_slice::<CandidateManifest>(&fs::read(
                &manifest_path,
            )?)?)
        })
        .transpose()?;
    if let Some(manifest) = &manifest {
        if manifest.schema != CANDIDATE_SCHEMA || manifest.task_id != task.id {
            return Err(format!(
                "{} does not describe task `{}`",
                manifest_path.display(),
                task.id
            )
            .into());
        }
        if manifest.attempt == Some(0) {
            return Err(format!("{} attempt must be at least 1", manifest_path.display()).into());
        }
        if manifest
            .temperature
            .is_some_and(|temperature| temperature < 0.0)
        {
            return Err(format!(
                "{} temperature must be non-negative",
                manifest_path.display()
            )
            .into());
        }
    }
    let fallback = candidates_path.join(&task.candidate);
    let task_local = candidates_path.join(&task.id).join("candidate.rss");
    let flat = candidates_path.join(format!("{}.rss", task.id));
    let source_path = manifest
        .as_ref()
        .and_then(|manifest| manifest.source.as_ref())
        .map(|path| candidates_path.join(path))
        .filter(|path| path.is_file())
        .or_else(|| task_local.is_file().then_some(task_local))
        .or_else(|| flat.is_file().then_some(flat))
        .or_else(|| fallback.is_file().then_some(fallback))
        .or_else(|| {
            let source = evaluation_root.join(&task.candidate);
            source.is_file().then_some(source)
        })
        .ok_or_else(|| format!("no candidate source for task `{}`", task.id))?;
    let metadata = CandidateMetadata {
        task_id: task.id.clone(),
        task_mode: task.mode,
        mode: manifest
            .as_ref()
            .and_then(|manifest| manifest.mode)
            .unwrap_or(GenerationMode::OfflineFixture),
        model: manifest
            .as_ref()
            .and_then(|manifest| manifest.model.clone())
            .unwrap_or_else(|| "offline-fixture".into()),
        model_version: manifest
            .as_ref()
            .and_then(|manifest| manifest.model_version.clone())
            .unwrap_or_else(|| "v1".into()),
        temperature: manifest.as_ref().and_then(|manifest| manifest.temperature),
        attempt: manifest
            .as_ref()
            .and_then(|manifest| manifest.attempt)
            .unwrap_or(1),
        generation_tokens: manifest
            .as_ref()
            .and_then(|manifest| manifest.generation_tokens)
            .unwrap_or(0),
        generation_duration_ms: manifest
            .as_ref()
            .and_then(|manifest| manifest.generation_duration_ms)
            .unwrap_or(0),
        repair_turns: manifest
            .as_ref()
            .and_then(|manifest| manifest.repair_turns)
            .unwrap_or(0),
    };
    Ok((source_path, metadata))
}

fn diagnostic_codes(diagnostics: &[rsscript_diagnostics::Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.code.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn static_check_from_codes(diagnostic_codes: Vec<String>) -> StaticCheck {
    StaticCheck {
        outcome: if diagnostic_codes.is_empty() {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail
        },
        diagnostic_codes,
    }
}

fn check_matches(expected: &ExpectedCheck, actual: &StaticCheck) -> bool {
    expected.outcome == actual.outcome && expected.diagnostic_codes == actual.diagnostic_codes
}

fn normalize_codes(codes: &mut Vec<String>) {
    codes.sort();
    codes.dedup();
}

fn evaluate_invariant(
    invariant: &Invariant,
    candidate_source: &str,
    target_source: &str,
    diagnostic_codes: &[String],
) -> InvariantResult {
    let (scope, passed) = match invariant.kind.as_str() {
        "candidate_source_contains" => (
            InvariantScope::Candidate,
            candidate_source.contains(&invariant.value),
        ),
        "source_contains" | "target_source_contains" => (
            InvariantScope::Target,
            target_source.contains(&invariant.value),
        ),
        "source_excludes" | "target_source_excludes" => (
            InvariantScope::Target,
            !target_source.contains(&invariant.value),
        ),
        "diagnostic_absent" => (
            InvariantScope::Target,
            diagnostic_codes.binary_search(&invariant.value).is_err(),
        ),
        _ => (InvariantScope::Target, false),
    };
    InvariantResult {
        kind: invariant.kind.clone(),
        value: invariant.value.clone(),
        scope,
        passed,
    }
}

fn aggregate(tasks: &[TaskReport]) -> Aggregate {
    Aggregate {
        task_count: tasks.len(),
        passed: tasks
            .iter()
            .filter(|task| task.status == ScoreStatus::Pass)
            .count(),
        failed: tasks
            .iter()
            .filter(|task| task.status == ScoreStatus::Fail)
            .count(),
        parse_passed: tasks
            .iter()
            .filter(|task| task.parse.outcome == CheckOutcome::Pass)
            .count(),
        static_check_passed: tasks
            .iter()
            .filter(|task| task.static_check.outcome == CheckOutcome::Pass)
            .count(),
        candidate_check_matches: tasks
            .iter()
            .filter(|task| task.diagnostic_match.candidate_check_matches)
            .count(),
        target_check_matches: tasks
            .iter()
            .filter(|task| task.diagnostic_match.target_check_matches)
            .count(),
        expected_diagnostics_match: tasks
            .iter()
            .filter(|task| task.diagnostic_match.expected_diagnostics_match)
            .count(),
        forbidden_diagnostics_match: tasks
            .iter()
            .filter(|task| task.diagnostic_match.forbidden_diagnostics_match)
            .count(),
        unknown_tool_or_argument: tasks
            .iter()
            .filter(|task| task.safety.unknown_tool_or_argument)
            .count(),
        invalid_mut_or_take: tasks
            .iter()
            .filter(|task| task.safety.invalid_mut_or_take)
            .count(),
        compiler_query_latency_ms: tasks
            .iter()
            .map(|task| task.compiler_query_latency_ms)
            .sum(),
        oracle_soundness: OracleSoundnessAggregate {
            prefixes_checked: tasks
                .iter()
                .map(|task| task.oracle_soundness.prefixes_checked)
                .sum(),
            terminals_recommended: tasks
                .iter()
                .map(|task| task.oracle_soundness.terminals_recommended)
                .sum(),
            terminals_accepted: tasks
                .iter()
                .map(|task| task.oracle_soundness.terminals_accepted)
                .sum(),
            dead_after_insert: tasks
                .iter()
                .map(|task| task.oracle_soundness.dead_after_insert)
                .sum(),
            semantic_candidates_checked: tasks
                .iter()
                .map(|task| task.oracle_soundness.semantic_candidates_checked)
                .sum(),
            violation_count: tasks
                .iter()
                .map(|task| task.oracle_soundness.violations.len())
                .sum(),
            sound_tasks: tasks
                .iter()
                .filter(|task| task.oracle_soundness.violations.is_empty())
                .count(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_scoring_is_stably_aggregated() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("xtask stays below workspace root")
            .to_path_buf()
            .join("evals");
        let first = score(&root.join("tasks"), &root).expect("fixtures score");
        let second = score(&root.join("tasks"), &root).expect("fixtures score again");
        assert_eq!(first.schema, AGENT_EVAL_SCHEMA);
        assert_eq!(
            first.provenance.static_check_environment,
            "standard_package_interfaces_plus_task_interfaces"
        );
        assert_eq!(first.tasks.len(), 10);
        assert_eq!(first.aggregate.task_count, first.tasks.len());
        assert_eq!(
            first
                .tasks
                .iter()
                .map(|task| (
                    &task.candidate.task_id,
                    task.status,
                    &task.static_check.diagnostic_codes
                ))
                .collect::<Vec<_>>(),
            second
                .tasks
                .iter()
                .map(|task| (
                    &task.candidate.task_id,
                    task.status,
                    &task.static_check.diagnostic_codes
                ))
                .collect::<Vec<_>>(),
        );
        assert!(
            first
                .tasks
                .iter()
                .any(|task| task.safety.unknown_tool_or_argument)
        );
        assert_eq!(first.aggregate.candidate_check_matches, 10);
        assert_eq!(first.aggregate.oracle_soundness.sound_tasks, 10);
        assert_eq!(first.aggregate.oracle_soundness.violation_count, 0);
        assert_eq!(
            first.aggregate.oracle_soundness.semantic_candidates_checked,
            2
        );
        assert!(
            first
                .tasks
                .iter()
                .any(|task| task.candidate.task_mode == TaskMode::Repair
                    && task.status == ScoreStatus::Fail)
        );
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../evals/schemas/report.v1.json"))
                .expect("report schema is JSON");
        let validator = jsonschema::validator_for(&schema).expect("report schema compiles");
        for task in &first.tasks {
            let value = serde_json::to_value(task).expect("task report serializes");
            assert!(validator.is_valid(&value), "invalid report: {value:#?}");
        }
    }

    #[test]
    fn unknown_flags_are_rejected() {
        let error =
            parse_options(["--wat".to_string()].into_iter()).expect_err("unknown flags must fail");
        assert!(error.to_string().contains("unknown"));
    }
}
