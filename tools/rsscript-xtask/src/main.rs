#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use rsscript_provider_api::CancellationToken;
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
    NativeInterpreterFn, ParameterSignature, ProviderCallMode, ProviderDescriptor, ProviderError,
    ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor, RUNTIME_ABI_VERSION,
    ResourceCleanupContract,
};
use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    provider_api::ProviderRegistry,
    runtime::{ExecutionRequest, RunLimits, Runtime},
};
use serde::{Deserialize, Serialize};

const METRICS_SCHEMA: &str = "rsscript.core_metrics.v1";
const SLO_SCHEMA: &str = "rsscript.core_slo.v1";
const MIGRATION_STATUS_SCHEMA: &str = "rsscript.migration_status.v1";
const MIGRATION_QUEUE_SCHEMA: &str = "rsscript.migration_queue.v1";
const WORKLOAD: &str = r#"
fn main() -> Int {
    let mut index = 0
    let mut total = 0
    while index < 1000 {
        total = (total + index) % 1000000
        index = index + 1
    }
    return total
}
"#;
const CANCELLATION_WORKLOAD: &str = r#"
fn main() -> Int {
    let mut value = 0
    while true {
        value = value + 1
    }
    return value
}
"#;
const PROVIDER_WORKLOAD: &str = r#"
module metrics
use host.metrics.*

fn main() -> Int {
    let mut index = 0
    let mut total = 0
    while index < 1000 {
        total = echo(value: index)
        index = index + 1
    }
    return total
}
"#;
const PROVIDER_INTERFACE: &str = "module host.metrics\npub fn echo(value: read Int) -> Int\n";

#[derive(Debug, Serialize)]
struct MetricDistribution {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Serialize)]
struct CoreMetrics {
    schema: &'static str,
    iterations: usize,
    environment: Environment,
    check: MetricDistribution,
    compile: MetricDistribution,
    artifact_verify: MetricDistribution,
    vm_execute: MetricDistribution,
    provider_execute: MetricDistribution,
    pre_cancel_rejection: MetricDistribution,
    artifact_bytes: usize,
    execution_steps: u64,
    execution_allocated_bytes: usize,
    provider_calls: u64,
    provider_request_bytes: usize,
    provider_response_bytes: usize,
    provider_total_duration_ns: u64,
    provider_max_duration_ns: u64,
}

#[derive(Debug, Serialize)]
struct Environment {
    os: &'static str,
    arch: &'static str,
    profile: &'static str,
    git_revision: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreSlo {
    schema: String,
    max_p95_ms: LatencySlo,
    max_artifact_bytes: usize,
    max_execution_steps: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LatencySlo {
    check: f64,
    compile: f64,
    artifact_verify: f64,
    vm_execute: f64,
    provider_execute: f64,
    pre_cancel_rejection: f64,
}

#[derive(Debug)]
struct Arguments {
    iterations: usize,
    output: Option<PathBuf>,
    check: Option<PathBuf>,
}

#[derive(Debug)]
struct MigrationStatusArguments {
    json: bool,
    open_only: bool,
    required_items: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct MigrationVerifyArguments {
    item_id: String,
    dry_run: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct MigrationWorkArguments {
    item_id: String,
    json: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct MigrationAuditArguments {
    json: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationStatus {
    schema: &'static str,
    source: &'static str,
    completed: usize,
    open: usize,
    items: Vec<MigrationStatusItem>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationStatusItem {
    id: String,
    title: String,
    completed: bool,
    line: usize,
}

/// A deliberately small, curated frontier over the complete migration
/// checklist. The Markdown checklist remains the authoritative status source;
/// this manifest supplies only prerequisite edges and reproducible acceptance
/// commands for the next independently mergeable slices.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationQueueManifest {
    schema: String,
    tasks: Vec<MigrationQueueTask>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationQueueTask {
    id: String,
    priority: u16,
    depends_on: Vec<String>,
    #[serde(default)]
    scope: Vec<String>,
    #[serde(default)]
    acceptance: Vec<String>,
    verification: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationReadyQueue {
    schema: &'static str,
    source: &'static str,
    ready: Vec<MigrationReadyItem>,
    blocked: Vec<MigrationBlockedItem>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationReadyItem {
    id: String,
    title: String,
    priority: u16,
    verification: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationBlockedItem {
    id: String,
    title: String,
    priority: u16,
    blocked_by: Vec<String>,
}

/// A self-contained work packet for one focused migration slice. It is
/// derived from the checklist and curated queue rather than becoming a second
/// mutable TODO source.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationWorkPacket {
    schema: &'static str,
    checklist_source: &'static str,
    queue_source: &'static str,
    id: String,
    title: String,
    checklist_line: usize,
    priority: u16,
    state: &'static str,
    depends_on: Vec<String>,
    blocked_by: Vec<String>,
    scope: Vec<String>,
    acceptance: Vec<String>,
    verification: Vec<String>,
}

/// Read-only consistency report over the authoritative checklist and the
/// deliberately smaller execution frontier. It never infers completion: a
/// parent whose children are checked is only a candidate for a human review
/// of its stated acceptance condition.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationAudit {
    schema: &'static str,
    checklist_source: &'static str,
    queue_source: &'static str,
    open_leaf_items_not_queued: Vec<MigrationAuditItem>,
    open_parents_with_completed_children: Vec<MigrationAuditItem>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MigrationAuditItem {
    id: String,
    title: String,
    line: usize,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("core-metrics") => run_core_metrics(parse_arguments(arguments)?),
        Some("migration-status") => run_migration_status(parse_migration_status_arguments(arguments)?),
        Some("migration-next") => run_migration_next(),
        Some("migration-next-json") => run_migration_next_json(),
        Some("migration-audit") => run_migration_audit(parse_migration_audit_arguments(arguments)?),
        Some("migration-work") => run_migration_work(parse_migration_work_arguments(arguments)?),
        Some("migration-verify") => run_migration_verify(parse_migration_verify_arguments(arguments)?),
        _ => Err(
            "usage:\n  cargo run -p rsscript-xtask --release -- core-metrics [--iterations N] [--output FILE] [--check SLO]\n  cargo run -p rsscript-xtask -- migration-status [--json] [--open] [--require ITEM]\n  cargo run -p rsscript-xtask -- migration-next\n  cargo run -p rsscript-xtask -- migration-next-json\n  cargo run -p rsscript-xtask -- migration-audit [--json]\n  cargo run -p rsscript-xtask -- migration-work ITEM [--json]\n  cargo run -p rsscript-xtask -- migration-verify ITEM [--dry-run]"
                .into(),
        ),
    }
}

fn run_migration_audit(arguments: MigrationAuditArguments) -> Result<(), Box<dyn Error>> {
    let audit = migration_audit()?;
    if arguments.json {
        println!("{}", serde_json::to_string_pretty(&audit)?);
        return Ok(());
    }
    println!(
        "Migration audit: {} open leaf item(s) outside the curated queue; {} parent completion candidate(s)",
        audit.open_leaf_items_not_queued.len(),
        audit.open_parents_with_completed_children.len()
    );
    print_audit_items(
        "Open leaf items not queued",
        &audit.open_leaf_items_not_queued,
    );
    print_audit_items(
        "Open parents whose declared children are all checked (review parent acceptance before checking)",
        &audit.open_parents_with_completed_children,
    );
    Ok(())
}

fn print_audit_items(label: &str, items: &[MigrationAuditItem]) {
    if items.is_empty() {
        println!("{label}: none");
        return;
    }
    println!("{label}:");
    for item in items {
        println!("  - {} — {} (line {})", item.id, item.title, item.line);
    }
}

fn run_migration_work(arguments: MigrationWorkArguments) -> Result<(), Box<dyn Error>> {
    let packet = migration_work_packet(&arguments.item_id)?;
    if arguments.json {
        println!("{}", serde_json::to_string_pretty(&packet)?);
        return Ok(());
    }
    println!(
        "Migration work packet: {} — {} [priority {}, {}]",
        packet.id, packet.title, packet.priority, packet.state
    );
    println!(
        "Checklist: {}:{}",
        packet.checklist_source, packet.checklist_line
    );
    print_packet_list("Prerequisites", &packet.depends_on);
    if !packet.blocked_by.is_empty() {
        print_packet_list("Blocked by", &packet.blocked_by);
    }
    print_packet_list("Expected change scope", &packet.scope);
    print_packet_list("Mechanical acceptance", &packet.acceptance);
    print_packet_list("Verification", &packet.verification);
    if packet.state == "ready" {
        println!(
            "Handoff: cargo run -p rsscript-xtask -- migration-verify {}",
            packet.id
        );
    } else {
        println!("Handoff: complete the blocking checklist items before this slice.");
    }
    Ok(())
}

fn print_packet_list(label: &str, values: &[String]) {
    if values.is_empty() {
        println!("{label}: none");
        return;
    }
    println!("{label}:");
    for value in values {
        println!("  - {value}");
    }
}

/// Executes the curated acceptance commands for one ready migration slice.
///
/// The queue remains deliberately declarative: this command cannot mark an
/// item complete or edit the checklist. It only removes the repeated manual
/// work of finding the exact focused test set, and fails before running when a
/// prerequisite is still open.
fn run_migration_verify(arguments: MigrationVerifyArguments) -> Result<(), Box<dyn Error>> {
    let queue = migration_ready_queue()?;
    let item = queue
        .ready
        .iter()
        .find(|item| item.id == arguments.item_id)
        .ok_or_else(|| {
            if let Some(blocked) = queue
                .blocked
                .iter()
                .find(|item| item.id == arguments.item_id)
            {
                format!(
                    "migration item `{}` is blocked by {}; run `migration-next` for the ready frontier",
                    arguments.item_id,
                    blocked.blocked_by.join(", ")
                )
            } else {
                format!(
                    "migration item `{}` is not a ready queued item; run `migration-next` for the ready frontier",
                    arguments.item_id
                )
            }
        })?;

    println!(
        "Migration verification: {} — {} ({} command{})",
        item.id,
        item.title,
        item.verification.len(),
        if item.verification.len() == 1 {
            ""
        } else {
            "s"
        }
    );
    for command in &item.verification {
        let cargo_arguments = parse_verification_command(command)?;
        if arguments.dry_run {
            println!("[dry-run] {command}");
            continue;
        }
        println!("[verify] {command}");
        let status = Command::new("cargo")
            .args(&cargo_arguments)
            .current_dir(workspace_root())
            .status()?;
        if !status.success() {
            return Err(format!(
                "migration verification for `{}` failed: `{command}` exited with {status}",
                item.id
            )
            .into());
        }
    }
    if arguments.dry_run {
        println!("Dry run complete; no verification commands were executed.");
    } else {
        println!(
            "Migration verification passed for `{}`. Update the checklist only when the item’s mechanical acceptance condition is also satisfied.",
            item.id
        );
    }
    Ok(())
}

fn run_migration_next() -> Result<(), Box<dyn Error>> {
    let queue = migration_ready_queue()?;
    println!(
        "Migration ready queue: {} ready, {} blocked ({})",
        queue.ready.len(),
        queue.blocked.len(),
        workspace_root()
            .join("docs/architecture/migration-work-queue.json")
            .display()
    );
    for item in queue.ready {
        println!(
            "- {} — {} [priority {}]",
            item.id, item.title, item.priority
        );
        for command in item.verification {
            println!("  verify: {command}");
        }
    }
    Ok(())
}

fn run_migration_next_json() -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&migration_ready_queue()?)?
    );
    Ok(())
}

fn migration_ready_queue() -> Result<MigrationReadyQueue, Box<dyn Error>> {
    let root = workspace_root();
    let status = migration_status(&fs::read_to_string(
        root.join("docs/architecture/migration-baseline.md"),
    )?)?;
    let manifest: MigrationQueueManifest = serde_json::from_slice(&fs::read(
        root.join("docs/architecture/migration-work-queue.json"),
    )?)?;
    if manifest.schema != MIGRATION_QUEUE_SCHEMA {
        return Err(format!(
            "unsupported migration work queue schema `{}`",
            manifest.schema
        )
        .into());
    }

    let by_id = status
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<BTreeMap<_, _>>();
    let mut queued = std::collections::BTreeSet::new();
    let mut ready = Vec::new();
    let mut blocked = Vec::new();
    for task in manifest.tasks {
        if !queued.insert(task.id.clone()) {
            return Err(format!("duplicate migration queue task `{}`", task.id).into());
        }
        let item = by_id
            .get(task.id.as_str())
            .ok_or_else(|| format!("migration queue task `{}` is not declared", task.id))?;
        if migration_item_has_children(&status, &item.id) {
            return Err(format!(
                "migration queue task `{}` is a parent milestone; queue only independently closeable leaf items",
                task.id
            )
            .into());
        }
        if item.completed {
            return Err(format!(
                "migration queue task `{}` is already complete; remove it from the frontier",
                task.id
            )
            .into());
        }
        if task.verification.is_empty() {
            return Err(format!(
                "migration queue task `{}` must declare at least one verification command",
                task.id
            )
            .into());
        }
        if task.scope.is_empty() || task.acceptance.is_empty() {
            return Err(format!(
                "migration queue task `{}` must declare non-empty scope and mechanical acceptance",
                task.id
            )
            .into());
        }
        for scope in &task.scope {
            validate_migration_scope(&root, scope)?;
        }
        for command in &task.verification {
            parse_verification_command(command)?;
        }
        let mut blocked_by = Vec::new();
        for dependency in task.depends_on {
            let dependency_item = by_id.get(dependency.as_str()).ok_or_else(|| {
                format!(
                    "migration queue task `{}` depends on undeclared item `{dependency}`",
                    task.id
                )
            })?;
            if !dependency_item.completed {
                blocked_by.push(dependency);
            }
        }
        if blocked_by.is_empty() {
            ready.push(MigrationReadyItem {
                id: task.id,
                title: item.title.clone(),
                priority: task.priority,
                verification: task.verification,
            });
        } else {
            blocked.push(MigrationBlockedItem {
                id: task.id,
                title: item.title.clone(),
                priority: task.priority,
                blocked_by,
            });
        }
    }
    ready.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then(left.id.cmp(&right.id))
    });
    blocked.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then(left.id.cmp(&right.id))
    });
    Ok(MigrationReadyQueue {
        schema: MIGRATION_QUEUE_SCHEMA,
        source: "docs/architecture/migration-work-queue.json",
        ready,
        blocked,
    })
}

/// Reject stale work-packet paths before they become an implementation plan.
///
/// Scope remains intentionally declarative and accepts a trailing glob such as
/// `crates/rsscript-vm/**`, but its non-glob anchor must resolve inside the
/// repository. This catches package renames and proposed crates that were
/// never created without turning the queue into a general glob engine.
fn validate_migration_scope(root: &Path, scope: &str) -> Result<(), Box<dyn Error>> {
    let anchor = scope
        .split_once('*')
        .map_or(scope, |(prefix, _)| prefix)
        .trim_end_matches('/');
    if anchor.is_empty() || Path::new(anchor).is_absolute() {
        return Err(format!("migration scope `{scope}` must have a relative path anchor").into());
    }
    let path = root.join(anchor);
    if !path.exists() {
        return Err(format!(
            "migration scope `{scope}` has no repository path anchor `{}`",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn migration_work_packet(item_id: &str) -> Result<MigrationWorkPacket, Box<dyn Error>> {
    let root = workspace_root();
    let status = migration_status(&fs::read_to_string(
        root.join("docs/architecture/migration-baseline.md"),
    )?)?;
    let manifest: MigrationQueueManifest = serde_json::from_slice(&fs::read(
        root.join("docs/architecture/migration-work-queue.json"),
    )?)?;
    if manifest.schema != MIGRATION_QUEUE_SCHEMA {
        return Err(format!(
            "unsupported migration work queue schema `{}`",
            manifest.schema
        )
        .into());
    }
    let task = manifest
        .tasks
        .iter()
        .find(|task| task.id == item_id)
        .ok_or_else(|| format!("migration work item `{item_id}` is not in the curated queue"))?;
    if task.scope.is_empty() || task.acceptance.is_empty() {
        return Err(format!(
            "migration work item `{item_id}` must declare non-empty scope and mechanical acceptance"
        )
        .into());
    }
    let checklist = status
        .items
        .iter()
        .find(|item| item.id == item_id)
        .ok_or_else(|| {
            format!("migration work item `{item_id}` is not declared in the checklist")
        })?;
    if checklist.completed {
        return Err(format!(
            "migration work item `{item_id}` is already complete; remove it from the curated queue"
        )
        .into());
    }

    let ready = migration_ready_queue()?;
    let blocked_by = ready
        .blocked
        .iter()
        .find(|item| item.id == item_id)
        .map(|item| item.blocked_by.clone())
        .unwrap_or_default();
    let state = if ready.ready.iter().any(|item| item.id == item_id) {
        "ready"
    } else if !blocked_by.is_empty() {
        "blocked"
    } else {
        return Err(format!(
            "migration work item `{item_id}` is neither ready nor blocked; validate its queue dependencies"
        )
        .into());
    };
    Ok(MigrationWorkPacket {
        schema: "rsscript.migration_work_packet.v1",
        checklist_source: "docs/architecture/migration-baseline.md",
        queue_source: "docs/architecture/migration-work-queue.json",
        id: task.id.clone(),
        title: checklist.title.clone(),
        checklist_line: checklist.line,
        priority: task.priority,
        state,
        depends_on: task.depends_on.clone(),
        blocked_by,
        scope: task.scope.clone(),
        acceptance: task.acceptance.clone(),
        verification: task.verification.clone(),
    })
}

/// The frontier intentionally stores only focused Cargo test/run commands.
/// Keeping this parser narrow prevents a JSON task entry from silently
/// becoming an arbitrary shell program and makes the reported command exactly
/// the command that is executed by `migration-verify`.
fn parse_verification_command(command: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let arguments = command
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let Some((program, rest)) = arguments.split_first() else {
        return Err("migration verification command cannot be empty".into());
    };
    if program != "cargo" {
        return Err(format!(
            "migration verification command must start with `cargo`, found `{program}`"
        )
        .into());
    }
    let Some(subcommand) = rest.first() else {
        return Err("migration verification command must include a Cargo subcommand".into());
    };
    if !matches!(subcommand.as_str(), "test" | "run") {
        return Err(format!(
            "migration verification command must use `cargo test` or `cargo run`, found `cargo {subcommand}`"
        )
        .into());
    }
    if arguments.iter().any(|argument| {
        argument
            .contains(|character: char| matches!(character, ';' | '|' | '&' | '`' | '\n' | '\r'))
    }) {
        return Err(format!(
            "migration verification command contains unsupported shell syntax: `{command}`"
        )
        .into());
    }
    Ok(rest.to_vec())
}

fn parse_migration_status_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<MigrationStatusArguments, Box<dyn Error>> {
    let mut parsed = MigrationStatusArguments {
        json: false,
        open_only: false,
        required_items: Vec::new(),
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json" => parsed.json = true,
            "--open" => parsed.open_only = true,
            "--require" => parsed.required_items.push(
                arguments
                    .next()
                    .ok_or("--require requires a migration item ID")?,
            ),
            _ => return Err(format!("unknown migration-status argument: {argument}").into()),
        }
    }
    Ok(parsed)
}

fn parse_migration_verify_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<MigrationVerifyArguments, Box<dyn Error>> {
    let item_id = arguments
        .next()
        .ok_or("migration-verify requires a migration item ID")?;
    let mut dry_run = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dry-run" => dry_run = true,
            _ => return Err(format!("unknown migration-verify argument: {argument}").into()),
        }
    }
    Ok(MigrationVerifyArguments { item_id, dry_run })
}

fn parse_migration_work_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<MigrationWorkArguments, Box<dyn Error>> {
    let item_id = arguments
        .next()
        .ok_or("migration-work requires a migration item ID")?;
    let mut json = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json" => json = true,
            _ => return Err(format!("unknown migration-work argument: {argument}").into()),
        }
    }
    Ok(MigrationWorkArguments { item_id, json })
}

fn parse_migration_audit_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<MigrationAuditArguments, Box<dyn Error>> {
    let mut parsed = MigrationAuditArguments { json: false };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json" => parsed.json = true,
            _ => return Err(format!("unknown migration-audit argument: {argument}").into()),
        }
    }
    Ok(parsed)
}

fn run_migration_status(arguments: MigrationStatusArguments) -> Result<(), Box<dyn Error>> {
    let path = workspace_root().join("docs/architecture/migration-baseline.md");
    let status = migration_status(&fs::read_to_string(&path)?)?;
    for required in &arguments.required_items {
        let item = status
            .items
            .iter()
            .find(|item| item.id == *required)
            .ok_or_else(|| format!("migration item `{required}` is not declared"))?;
        if !item.completed {
            return Err(format!(
                "migration item `{required}` remains open (line {}: {})",
                item.line, item.title
            )
            .into());
        }
    }

    if arguments.json {
        let mut output = status;
        if arguments.open_only {
            output.items.retain(|item| !item.completed);
        }
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!(
        "Migration checklist: {} complete, {} open ({})",
        status.completed,
        status.open,
        path.display()
    );
    for item in status
        .items
        .iter()
        .filter(|item| {
            arguments.required_items.is_empty()
                || arguments
                    .required_items
                    .iter()
                    .any(|required| required == &item.id)
        })
        .filter(|item| !arguments.open_only || !item.completed)
    {
        let marker = if item.completed { 'x' } else { ' ' };
        println!(
            "- [{marker}] {} — {} (line {})",
            item.id, item.title, item.line
        );
    }
    Ok(())
}

fn migration_status(document: &str) -> Result<MigrationStatus, Box<dyn Error>> {
    let mut items = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let lines = document.lines().collect::<Vec<_>>();
    let mut offset = 0;
    while offset < lines.len() {
        let line = lines[offset];
        let trimmed = line.trim_start();
        let (completed, label) = match trimmed
            .strip_prefix("- [x] **")
            .map(|label| (true, label))
            .or_else(|| trimmed.strip_prefix("- [ ] **").map(|label| (false, label)))
        {
            Some(item) => item,
            None => {
                offset += 1;
                continue;
            }
        };
        let start_line = offset + 1;
        let mut label = label.to_string();
        while !label.contains("**") {
            offset += 1;
            let continuation = lines
                .get(offset)
                .ok_or_else(|| format!("unterminated migration item on line {start_line}"))?;
            label.push(' ');
            label.push_str(continuation.trim());
        }
        let (label, _) = label
            .split_once("**")
            .expect("migration item loop must stop at closing bold marker");
        let (id, title) = label
            .split_once(" — ")
            .ok_or_else(|| format!("migration item on line {start_line} is missing ` — `"))?;
        if id.is_empty() || title.is_empty() {
            return Err(format!("invalid migration item on line {start_line}").into());
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("duplicate migration item `{id}`").into());
        }
        items.push(MigrationStatusItem {
            id: id.to_string(),
            title: title.to_string(),
            completed,
            line: start_line,
        });
        offset += 1;
    }
    if items.is_empty() {
        return Err("migration checklist contains no parseable items".into());
    }
    let completed = items.iter().filter(|item| item.completed).count();
    Ok(MigrationStatus {
        schema: MIGRATION_STATUS_SCHEMA,
        source: "docs/architecture/migration-baseline.md",
        completed,
        open: items.len() - completed,
        items,
    })
}

fn migration_audit() -> Result<MigrationAudit, Box<dyn Error>> {
    let root = workspace_root();
    let status = migration_status(&fs::read_to_string(
        root.join("docs/architecture/migration-baseline.md"),
    )?)?;
    // Validate the frontier before reporting against it. An audit that accepted
    // an unknown or completed queue entry would hide exactly the drift it is
    // intended to surface.
    let _ready = migration_ready_queue()?;
    let manifest: MigrationQueueManifest = serde_json::from_slice(&fs::read(
        root.join("docs/architecture/migration-work-queue.json"),
    )?)?;
    let queued = manifest
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    Ok(audit_migration_status(&status, &queued))
}

fn audit_migration_status(
    status: &MigrationStatus,
    queued: &std::collections::BTreeSet<&str>,
) -> MigrationAudit {
    let mut open_leaf_items_not_queued = Vec::new();
    let mut open_parents_with_completed_children = Vec::new();
    for item in &status.items {
        let prefix = format!("{}.", item.id);
        let descendants = status
            .items
            .iter()
            .filter(|candidate| is_migration_descendant(&item.id, &candidate.id, &prefix))
            .collect::<Vec<_>>();
        if descendants.is_empty() {
            if !item.completed && !queued.contains(item.id.as_str()) {
                open_leaf_items_not_queued.push(MigrationAuditItem {
                    id: item.id.clone(),
                    title: item.title.clone(),
                    line: item.line,
                });
            }
        } else if !item.completed && descendants.iter().all(|child| child.completed) {
            open_parents_with_completed_children.push(MigrationAuditItem {
                id: item.id.clone(),
                title: item.title.clone(),
                line: item.line,
            });
        }
    }
    MigrationAudit {
        schema: "rsscript.migration_audit.v1",
        checklist_source: "docs/architecture/migration-baseline.md",
        queue_source: "docs/architecture/migration-work-queue.json",
        open_leaf_items_not_queued,
        open_parents_with_completed_children,
    }
}

/// The checklist predates the execution helper and uses both `M03.2.a`-style
/// and compact `M03.2a` child IDs. Treat a dotted suffix as a descendant, or
/// a non-empty all-letter suffix as a compact child; a numeric suffix such as
/// `M01.10` must remain a sibling of `M01.1`, not become its child.
fn is_migration_descendant(parent: &str, candidate: &str, dotted_prefix: &str) -> bool {
    candidate.starts_with(dotted_prefix)
        || candidate.strip_prefix(parent).is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphabetic())
        })
}

/// A queue entry is executable only when it maps to one independently
/// closeable checklist item. Parent milestones deliberately stay out of the
/// frontier: their children carry the bounded implementation contracts and a
/// parent may require an additional acceptance review after they close.
fn migration_item_has_children(status: &MigrationStatus, id: &str) -> bool {
    let dotted_prefix = format!("{id}.");
    status
        .items
        .iter()
        .any(|candidate| is_migration_descendant(id, &candidate.id, &dotted_prefix))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask must remain under the workspace tools directory")
        .to_path_buf()
}

fn parse_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Arguments, Box<dyn Error>> {
    let mut parsed = Arguments {
        iterations: 20,
        output: None,
        check: None,
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--iterations" => {
                parsed.iterations = arguments
                    .next()
                    .ok_or("--iterations requires a value")?
                    .parse()?;
                if parsed.iterations < 2 {
                    return Err("--iterations must be at least 2".into());
                }
            }
            "--output" => {
                parsed.output = Some(PathBuf::from(
                    arguments.next().ok_or("--output requires a path")?,
                ));
            }
            "--check" => {
                parsed.check = Some(PathBuf::from(
                    arguments.next().ok_or("--check requires a path")?,
                ));
            }
            _ => return Err(format!("unknown core-metrics argument: {argument}").into()),
        }
    }
    Ok(parsed)
}

fn run_core_metrics(arguments: Arguments) -> Result<(), Box<dyn Error>> {
    let compiler = Compiler;
    let provider_package = compiler.compile_with_interfaces(
        &[("provider-metrics.rss", PROVIDER_WORKLOAD)],
        &[("provider-metrics.rssi", PROVIDER_INTERFACE)],
    )?;
    let provider_package = ArtifactVerifier
        .verify(provider_package)?
        .admit_trusted_input();
    let provider_runtime = metrics_provider_runtime()?;

    // Warm each path before collecting distributions so the report measures
    // steady Core behavior rather than one-time process and allocator setup.
    for _ in 0..3 {
        let _ = compiler.check("core-metrics.rss", WORKLOAD);
        let package = compiler.compile("core-metrics.rss", WORKLOAD)?;
        let bytes = package.bundle_bytes()?;
        let loaded = ArtifactVerifier.verify_bytes(&bytes)?.admit_trusted_input();
        Runtime::default()
            .link(&loaded)?
            .execute(ExecutionRequest::default());
        provider_runtime
            .link(&provider_package)?
            .execute(ExecutionRequest::default());
    }

    let mut check = Vec::with_capacity(arguments.iterations);
    let mut compile = Vec::with_capacity(arguments.iterations);
    let mut verify = Vec::with_capacity(arguments.iterations);
    let mut execute = Vec::with_capacity(arguments.iterations);
    let mut provider_execute = Vec::with_capacity(arguments.iterations);
    let mut cancel = Vec::with_capacity(arguments.iterations);
    let mut artifact_bytes = 0;
    let mut execution_steps = 0;
    let mut execution_allocated_bytes = 0;
    let mut provider_calls = 0;
    let mut provider_request_bytes = 0;
    let mut provider_response_bytes = 0;
    let mut provider_total_duration_ns = 0;
    let mut provider_max_duration_ns = 0;

    let cancellation_package = ArtifactVerifier
        .verify(compiler.compile("cancel.rss", CANCELLATION_WORKLOAD)?)?
        .admit_trusted_input();
    for _ in 0..arguments.iterations {
        check.push(measure(|| {
            let diagnostics = compiler.check("core-metrics.rss", WORKLOAD);
            assert!(
                diagnostics.is_empty(),
                "Core metric workload must remain valid"
            );
        }));

        let started = Instant::now();
        let package = compiler.compile("core-metrics.rss", WORKLOAD)?;
        compile.push(elapsed_ms(started));

        let bytes = package.bundle_bytes()?;
        artifact_bytes = bytes.len();
        let started = Instant::now();
        let loaded = ArtifactVerifier.verify_bytes(&bytes)?.admit_trusted_input();
        verify.push(elapsed_ms(started));

        let runtime = Runtime::default();
        let linked = runtime.link(&loaded)?;
        let started = Instant::now();
        let report = linked.execute(ExecutionRequest::default());
        if let Some(error) = report.failure() {
            return Err(error.to_string().into());
        }
        execute.push(elapsed_ms(started));
        execution_steps = report.usage.steps_consumed;
        execution_allocated_bytes = report.usage.allocation_bytes_consumed;

        let linked = provider_runtime.link(&provider_package)?;
        let started = Instant::now();
        let report = linked.execute(ExecutionRequest::default());
        if let Some(error) = report.failure() {
            return Err(error.to_string().into());
        }
        provider_execute.push(elapsed_ms(started));
        let summary = report
            .telemetry
            .provider_functions
            .first()
            .ok_or("Provider metric workload did not record its external call")?;
        provider_calls = summary.calls;
        provider_request_bytes = summary.request_bytes;
        provider_response_bytes = summary.response_bytes;
        provider_total_duration_ns = summary.total_duration_ns;
        provider_max_duration_ns = summary.max_duration_ns;

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let runtime = Runtime::new(ProviderRegistry::default());
        let linked = runtime.link(&cancellation_package)?;
        let started = Instant::now();
        let report = linked.execute(
            ExecutionRequest::default()
                .limits(RunLimits::unbounded_for_trusted_host().with_cancellation(cancellation)),
        );
        cancel.push(elapsed_ms(started));
        if report.termination_reason().as_str() != "cancelled" {
            return Err(format!(
                "pre-cancel workload terminated as {}",
                report.termination_reason().as_str()
            )
            .into());
        }
    }

    let metrics = CoreMetrics {
        schema: METRICS_SCHEMA,
        iterations: arguments.iterations,
        environment: Environment {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            git_revision: std::env::var("GITHUB_SHA").ok(),
        },
        check: distribution(check),
        compile: distribution(compile),
        artifact_verify: distribution(verify),
        vm_execute: distribution(execute),
        provider_execute: distribution(provider_execute),
        pre_cancel_rejection: distribution(cancel),
        artifact_bytes,
        execution_steps,
        execution_allocated_bytes,
        provider_calls,
        provider_request_bytes,
        provider_response_bytes,
        provider_total_duration_ns,
        provider_max_duration_ns,
    };
    let json = serde_json::to_string_pretty(&metrics)?;
    println!("{json}");
    if let Some(output) = arguments.output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output, format!("{json}\n"))?;
    }
    if let Some(slo_path) = arguments.check {
        check_slo(&metrics, &slo_path)?;
    }
    Ok(())
}

fn metrics_provider_runtime() -> Result<Runtime, Box<dyn Error>> {
    let symbol = ExternalSymbol::new("host.metrics.echo")
        .map_err(|_| "invalid built-in metrics provider symbol")?;
    let signature = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "value".into(),
            effect: DataEffect::Read,
            ty: "Int".into(),
            retained: false,
        }],
        result: "Int".into(),
        asynchronous: false,
    };
    let descriptor = ProviderDescriptor {
        provider_id: "rsscript.metrics".into(),
        provider_version: "1.0.0".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "echo".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: ResourceCleanupContract::None,
            error_mapping: ProviderErrorMapping::StructuredV1,
        }],
    };
    let mut providers = ProviderRegistry::default();
    providers.register(
        &descriptor,
        BTreeMap::from([(
            symbol,
            ProviderFunction {
                signature,
                callable: NativeInterpreterFn::new(|mut values| {
                    values.pop().filter(|_| values.is_empty()).ok_or_else(|| {
                        ProviderError::invalid_argument("metrics echo expects one argument")
                    })
                }),
            },
        )]),
    )?;
    Ok(Runtime::new(providers))
}

fn measure(action: impl FnOnce()) -> f64 {
    let started = Instant::now();
    action();
    elapsed_ms(started)
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn distribution(mut samples: Vec<f64>) -> MetricDistribution {
    samples.sort_by(f64::total_cmp);
    MetricDistribution {
        p50_ms: percentile(&samples, 0.50),
        p95_ms: percentile(&samples, 0.95),
        max_ms: *samples.last().expect("at least two metric samples"),
    }
}

fn percentile(samples: &[f64], quantile: f64) -> f64 {
    let index = ((samples.len() - 1) as f64 * quantile).ceil() as usize;
    samples[index]
}

fn check_slo(metrics: &CoreMetrics, path: &PathBuf) -> Result<(), Box<dyn Error>> {
    let slo: CoreSlo = serde_json::from_slice(&fs::read(path)?)?;
    if slo.schema != SLO_SCHEMA {
        return Err(format!("unsupported Core SLO schema: {}", slo.schema).into());
    }
    let latency = [
        ("check", metrics.check.p95_ms, slo.max_p95_ms.check),
        ("compile", metrics.compile.p95_ms, slo.max_p95_ms.compile),
        (
            "artifact_verify",
            metrics.artifact_verify.p95_ms,
            slo.max_p95_ms.artifact_verify,
        ),
        (
            "vm_execute",
            metrics.vm_execute.p95_ms,
            slo.max_p95_ms.vm_execute,
        ),
        (
            "provider_execute",
            metrics.provider_execute.p95_ms,
            slo.max_p95_ms.provider_execute,
        ),
        (
            "pre_cancel_rejection",
            metrics.pre_cancel_rejection.p95_ms,
            slo.max_p95_ms.pre_cancel_rejection,
        ),
    ];
    let mut failures = latency
        .into_iter()
        .filter(|(_, observed, maximum)| observed > maximum)
        .map(|(name, observed, maximum)| {
            format!("{name} p95 {observed:.3} ms exceeds {maximum:.3} ms")
        })
        .collect::<Vec<_>>();
    if metrics.artifact_bytes > slo.max_artifact_bytes {
        failures.push(format!(
            "artifact size {} exceeds {} bytes",
            metrics.artifact_bytes, slo.max_artifact_bytes
        ));
    }
    if metrics.execution_steps > slo.max_execution_steps {
        failures.push(format!(
            "execution steps {} exceed {}",
            metrics.execution_steps, slo.max_execution_steps
        ));
    }
    if failures.is_empty() {
        println!("Core SLO check passed: {}", path.display());
        Ok(())
    } else {
        Err(format!("Core SLO check failed:\n- {}", failures.join("\n- ")).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_a_conservative_nearest_rank() {
        let samples = (1..=20).map(f64::from).collect::<Vec<_>>();
        assert_eq!(percentile(&samples, 0.50), 11.0);
        assert_eq!(percentile(&samples, 0.95), 20.0);
    }

    #[test]
    fn argument_parser_rejects_single_sample_runs() {
        let error = parse_arguments(["--iterations".into(), "1".into()].into_iter())
            .expect_err("one iteration cannot produce a useful distribution");
        assert!(error.to_string().contains("at least 2"));
    }

    #[test]
    fn migration_status_parses_nested_and_wrapped_checklist_items() {
        let status = migration_status(
            "- [x] **G01 — Completed parent.**\n  - [ ] **G01.1 — Open child\n    item.**\n",
        )
        .expect("well-formed checklist");
        assert_eq!(status.completed, 1);
        assert_eq!(status.open, 1);
        assert_eq!(
            status.items,
            vec![
                MigrationStatusItem {
                    id: "G01".into(),
                    title: "Completed parent.".into(),
                    completed: true,
                    line: 1,
                },
                MigrationStatusItem {
                    id: "G01.1".into(),
                    title: "Open child item.".into(),
                    completed: false,
                    line: 2,
                },
            ]
        );
    }

    #[test]
    fn migration_status_rejects_duplicate_item_ids() {
        let error = migration_status("- [x] **G01 — First.**\n- [ ] **G01 — Second.**\n")
            .expect_err("duplicate IDs make a checklist gate ambiguous");
        assert!(error.to_string().contains("duplicate migration item `G01`"));
    }

    #[test]
    fn migration_audit_distinguishes_unqueued_leaves_from_parent_candidates() {
        let status = migration_status(
            "- [ ] **G01 — Parent.**\n  - [x] **G01.1 — Completed child.**\n- [ ] **S05.1 — Queued leaf.**\n- [ ] **E02.2 — Unqueued leaf.**\n",
        )
        .expect("well-formed checklist");
        let queued = std::collections::BTreeSet::from(["S05.1"]);
        assert_eq!(
            audit_migration_status(&status, &queued),
            MigrationAudit {
                schema: "rsscript.migration_audit.v1",
                checklist_source: "docs/architecture/migration-baseline.md",
                queue_source: "docs/architecture/migration-work-queue.json",
                open_leaf_items_not_queued: vec![MigrationAuditItem {
                    id: "E02.2".into(),
                    title: "Unqueued leaf.".into(),
                    line: 4,
                }],
                open_parents_with_completed_children: vec![MigrationAuditItem {
                    id: "G01".into(),
                    title: "Parent.".into(),
                    line: 1,
                }],
            }
        );
    }

    #[test]
    fn migration_audit_recognizes_compact_lettered_children_without_merging_numeric_siblings() {
        assert!(is_migration_descendant("M03.2", "M03.2a", "M03.2."));
        assert!(is_migration_descendant("M03.2", "M03.2.a", "M03.2."));
        assert!(!is_migration_descendant("M01.1", "M01.10", "M01.1."));
    }

    #[test]
    fn migration_frontier_rejects_parent_milestones() {
        let status = migration_status(
            "- [ ] **S05.1 — Parent.**\n  - [x] **S05.1a — Completed child.**\n  - [ ] **S05.1b — Open child.**\n- [ ] **E02.2 — Leaf.**\n",
        )
        .expect("well-formed checklist");
        assert!(migration_item_has_children(&status, "S05.1"));
        assert!(!migration_item_has_children(&status, "S05.1a"));
        assert!(!migration_item_has_children(&status, "E02.2"));
    }

    #[test]
    fn published_migration_checklist_is_machine_readable() {
        let status = migration_status(include_str!(
            "../../../docs/architecture/migration-baseline.md"
        ))
        .expect("published migration checklist must remain parseable");
        assert!(status.items.len() > 100);
        assert!(status.items.iter().any(|item| item.id == "S02"));
        assert!(status.items.iter().any(|item| item.id == "A09"));
    }

    #[test]
    fn published_migration_frontier_is_fail_closed_and_prioritized() {
        let queue = migration_ready_queue().expect("published frontier must be valid");
        assert_eq!(queue.schema, MIGRATION_QUEUE_SCHEMA);
        assert!(queue.ready.iter().any(|item| item.id == "A09.3a"));
        assert!(!queue.ready.iter().any(|item| item.id == "S05.2c2"));
        assert!(!queue.ready.iter().any(|item| item.id == "S05.2b"));
        assert!(!queue.ready.iter().any(|item| item.id == "S05.1e"));
        assert!(
            queue
                .ready
                .windows(2)
                .all(|items| items[0].priority <= items[1].priority)
        );
        assert!(queue.ready.iter().all(|item| !item.verification.is_empty()));
    }

    #[test]
    fn migration_verify_arguments_require_one_item_id() {
        assert_eq!(
            parse_migration_verify_arguments(["S05.1".into(), "--dry-run".into()].into_iter())
                .expect("valid migration verify invocation"),
            MigrationVerifyArguments {
                item_id: "S05.1".into(),
                dry_run: true,
            }
        );
        let error = parse_migration_verify_arguments(std::iter::empty())
            .expect_err("an item ID is required");
        assert!(error.to_string().contains("requires a migration item ID"));
    }

    #[test]
    fn migration_work_arguments_require_one_item_id_and_support_json() {
        assert_eq!(
            parse_migration_work_arguments(["S05.1".into(), "--json".into()].into_iter())
                .expect("valid migration work invocation"),
            MigrationWorkArguments {
                item_id: "S05.1".into(),
                json: true,
            }
        );
        let error = parse_migration_work_arguments(std::iter::empty())
            .expect_err("a work item ID is required");
        assert!(error.to_string().contains("requires a migration item ID"));
    }

    #[test]
    fn migration_audit_arguments_only_accept_json() {
        assert_eq!(
            parse_migration_audit_arguments(["--json".into()].into_iter())
                .expect("JSON audit invocation"),
            MigrationAuditArguments { json: true }
        );
        let error = parse_migration_audit_arguments(["--unknown".into()].into_iter())
            .expect_err("unknown audit flags must fail closed");
        assert!(
            error
                .to_string()
                .contains("unknown migration-audit argument")
        );
    }

    #[test]
    fn published_migration_work_packet_is_bounded_and_actionable() {
        let packet = migration_work_packet("A09.3a").expect("published work packet");
        assert_eq!(packet.schema, "rsscript.migration_work_packet.v1");
        assert_eq!(packet.state, "ready");
        assert!(!packet.scope.is_empty());
        assert!(!packet.acceptance.is_empty());
        assert!(!packet.verification.is_empty());
        assert!(
            packet
                .scope
                .iter()
                .any(|path| path.contains("process-guard"))
        );

        let runner = migration_work_packet("A09.3a").expect("published runner work packet");
        assert!(matches!(runner.state, "ready" | "blocked"));
        if runner.state == "blocked" {
            assert!(!runner.blocked_by.is_empty());
        } else {
            assert!(runner.blocked_by.is_empty());
        }
    }

    #[test]
    fn migration_verification_commands_are_narrow_cargo_invocations() {
        assert_eq!(
            parse_verification_command("cargo test -p rsscript-sdk --locked")
                .expect("Cargo test command"),
            vec!["test", "-p", "rsscript-sdk", "--locked"]
        );
        for command in ["", "echo test", "cargo clean", "cargo test; rm -rf test"] {
            assert!(parse_verification_command(command).is_err(), "{command}");
        }
    }

    #[test]
    fn migration_scopes_must_name_existing_relative_anchors() {
        let root = workspace_root();
        validate_migration_scope(&root, "crates/process-guard/**").expect("existing glob anchor");
        validate_migration_scope(&root, "docs/threat-model.md").expect("existing file anchor");
        let missing = validate_migration_scope(&root, "crates/not-a-real-package/**")
            .expect_err("stale scope must fail before work begins");
        assert!(missing.to_string().contains("no repository path anchor"));
        assert!(validate_migration_scope(&root, "/tmp/**").is_err());
    }

    #[test]
    fn metrics_shape_matches_the_published_schema() {
        let distribution = || MetricDistribution {
            p50_ms: 1.0,
            p95_ms: 2.0,
            max_ms: 3.0,
        };
        let metrics = CoreMetrics {
            schema: METRICS_SCHEMA,
            iterations: 20,
            environment: Environment {
                os: "test",
                arch: "test",
                profile: "release",
                git_revision: None,
            },
            check: distribution(),
            compile: distribution(),
            artifact_verify: distribution(),
            vm_execute: distribution(),
            provider_execute: distribution(),
            pre_cancel_rejection: distribution(),
            artifact_bytes: 1,
            execution_steps: 1,
            execution_allocated_bytes: 0,
            provider_calls: 1000,
            provider_request_bytes: 8000,
            provider_response_bytes: 8000,
            provider_total_duration_ns: 1000,
            provider_max_duration_ns: 1,
        };
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/rsscript.core_metrics.v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let instance = serde_json::to_value(metrics).unwrap();
        assert!(validator.is_valid(&instance));
    }
}
