use reir::adapters::rsscript::{
    rsscript_check_json_to_bundle, rsscript_json_to_bundle, rsscript_lock_diff_json_to_bundle,
    rsscript_lock_json_to_bundle, rsscript_metadata_json_to_bundle,
    rsscript_publish_json_to_bundle, rsscript_tree_json_to_bundle, rsscript_vendor_json_to_bundle,
};
use reir::adapters::terraform::{terraform_dir_to_bundle, terraform_plan_json_to_bundle};
use reir::{
    Bundle, Capability, Diff, DiffItem, DiffItemKind, Evidence, Fact, FactRole, Reconciliation,
    ReconciliationKind, ReconciliationStatus, Slice, SliceKind, compute_diff,
    format_pr_review_comment, reconcile_capabilities_for_target, slice_by_kind,
};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_CLI_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CLI_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_MERGE_INPUT_FILES: usize = 1024;
static OUTPUT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const USAGE: &str = "Usage:
  reir collect --producer rsscript [--review-map review-map.json] [--package-review package-review.json] [--package-check check.json] [--package-lock lock.json] [--lock-update lock-diff.json] [--package-tree tree.json] [--package-publish publish.json] [--package-metadata metadata.json] [--package-vendor vendor.json] [--package-name name] [--out bundle.json] [--json]
  reir collect --producer terraform --from infra/terraform [--out bundle.json] [--json]
  reir collect --producer terraform-plan --from plan.json [--out bundle.json] [--json]
  reir reconcile --required required.json --granted granted.json [--target name] [--json]
  reir reconcile [--bundle bundle.json] [--target name] [--out reconciled.json] [--json]
  reir report-pr --required required.json --granted granted.json --principal id [--target name] [--policy rss-policy.toml] [--ci-json | --sarif] [--ci-json-out path] [--sarif-out path] [--fail-on-missing | --allow-missing] [--fail-on-unknown | --allow-unknown] [--fail-on-excess | --allow-excess] [--require-verified-capabilities | --allow-unverified-capabilities]
  reir diff --baseline baseline.json --current current.json [--json] [--fail-on-change]
  reir slice --bundle bundle.json [--kind <slice-kind>] [--json]
  reir merge file1.json file2.json [...] --out merged.json
  reir show bundle.json [--json]";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(command) = args.get(1).map(String::as_str) else {
        print_usage();
        return ExitCode::from(2);
    };

    match command {
        "collect" => run_collect(&args[2..]),
        "reconcile" => run_reconcile(&args[2..]),
        "report-pr" => run_report_pr(&args[2..]),
        "diff" => run_diff(&args[2..]),
        "slice" => run_slice(&args[2..]),
        "merge" => run_merge(&args[2..]),
        "show" => run_show(&args[2..]),
        "--help" | "-h" | "help" => {
            print_usage();
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("unknown command: {command}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Runtime(String),
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime(message.into())
    }
}

fn run_reconcile(args: &[String]) -> ExitCode {
    match try_run_reconcile(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn run_collect(args: &[String]) -> ExitCode {
    match try_run_collect(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn run_report_pr(args: &[String]) -> ExitCode {
    match try_run_report_pr(args) {
        Ok((code, comment)) => {
            print!("{comment}");
            code
        }
        Err(error) => report_error(error),
    }
}

fn try_run_collect(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut producer = None;
    let mut review_map = None;
    let mut package_review = None;
    let mut package_check = None;
    let mut package_lock = None;
    let mut lock_update = None;
    let mut package_tree = None;
    let mut package_publish = None;
    let mut package_metadata = None;
    let mut package_vendor = None;
    let mut package_name = None;
    let mut from = None;
    let mut out = None;
    let mut json = false;
    let mut strict = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--strict" => strict = true,
            "--producer" => producer = Some(take_value(args, &mut index, "--producer")?),
            "--review-map" => review_map = Some(take_value(args, &mut index, "--review-map")?),
            "--package-review" => {
                package_review = Some(take_value(args, &mut index, "--package-review")?)
            }
            "--package-check" => {
                package_check = Some(take_value(args, &mut index, "--package-check")?)
            }
            "--package-lock" => {
                package_lock = Some(take_value(args, &mut index, "--package-lock")?)
            }
            "--lock-update" => lock_update = Some(take_value(args, &mut index, "--lock-update")?),
            "--package-tree" => {
                package_tree = Some(take_value(args, &mut index, "--package-tree")?)
            }
            "--package-publish" => {
                package_publish = Some(take_value(args, &mut index, "--package-publish")?)
            }
            "--package-metadata" => {
                package_metadata = Some(take_value(args, &mut index, "--package-metadata")?)
            }
            "--package-vendor" => {
                package_vendor = Some(take_value(args, &mut index, "--package-vendor")?)
            }
            "--package-name" => {
                package_name = Some(take_value(args, &mut index, "--package-name")?)
            }
            "--from" => from = Some(take_value(args, &mut index, "--from")?),
            "--out" => out = Some(take_value(args, &mut index, "--out")?),
            "--json" => json = true,
            other => {
                return Err(CliError::usage(format!(
                    "unknown collect argument: {other}"
                )));
            }
        }
        index += 1;
    }

    let producer = producer.ok_or_else(|| CliError::usage("missing --producer <name>"))?;
    if producer == "terraform" {
        let from_path =
            from.ok_or_else(|| CliError::usage("terraform collect requires --from <path>"))?;
        if review_map.is_some()
            || package_review.is_some()
            || package_check.is_some()
            || package_lock.is_some()
            || lock_update.is_some()
            || package_tree.is_some()
            || package_publish.is_some()
            || package_metadata.is_some()
            || package_vendor.is_some()
            || package_name.is_some()
        {
            return Err(CliError::usage(
                "terraform collect only accepts --from, --out, and --json",
            ));
        }
        let bundle =
            terraform_dir_to_bundle(std::path::Path::new(&from_path)).map_err(|error| {
                CliError::runtime(format!(
                    "failed to collect Terraform/OpenTofu evidence: {error}"
                ))
            })?;
        if let Some(out_path) = &out {
            write_json_file(out_path, &bundle)?;
            if !json {
                println!("collected terraform evidence into {out_path}");
                print_bundle_summary(&bundle);
            }
        }
        if json || out.is_none() {
            print_json(&bundle)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if producer == "terraform-plan" {
        let from_path = from
            .ok_or_else(|| CliError::usage("terraform-plan collect requires --from <plan.json>"))?;
        let plan_json = read_bounded_text(&from_path)?;
        let bundle = terraform_plan_json_to_bundle(&plan_json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect Terraform plan JSON evidence: {error}"
            ))
        })?;
        if let Some(out_path) = &out {
            write_json_file(out_path, &bundle)?;
            if !json {
                println!("collected terraform plan evidence into {out_path}");
                print_bundle_summary(&bundle);
            }
        }
        if json || out.is_none() {
            print_json(&bundle)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if producer != "rsscript" {
        return Err(CliError::usage(format!(
            "`reir collect --producer {producer}` is planned; this build supports `rsscript` and `terraform`."
        )));
    }
    if from.is_some() {
        return Err(CliError::usage(
            "`--from` is only supported by Terraform/OpenTofu collection; use RSScript JSON input flags with `--producer rsscript`.",
        ));
    }
    if review_map.is_none()
        && package_review.is_none()
        && package_check.is_none()
        && package_lock.is_none()
        && lock_update.is_none()
        && package_tree.is_none()
        && package_publish.is_none()
        && package_metadata.is_none()
        && package_vendor.is_none()
    {
        return Err(CliError::usage(
            "collect requires at least one RSScript JSON input",
        ));
    }

    let mut aggregate_input_bytes = 0;
    let review_map_json =
        read_optional_text_accounted(review_map.as_deref(), &mut aggregate_input_bytes)?;
    let package_review_json =
        read_optional_text_accounted(package_review.as_deref(), &mut aggregate_input_bytes)?;
    let package_check_json =
        read_optional_text_accounted(package_check.as_deref(), &mut aggregate_input_bytes)?;
    let package_lock_json =
        read_optional_text_accounted(package_lock.as_deref(), &mut aggregate_input_bytes)?;
    let lock_update_json =
        read_optional_text_accounted(lock_update.as_deref(), &mut aggregate_input_bytes)?;
    let package_tree_json =
        read_optional_text_accounted(package_tree.as_deref(), &mut aggregate_input_bytes)?;
    let package_publish_json =
        read_optional_text_accounted(package_publish.as_deref(), &mut aggregate_input_bytes)?;
    let package_metadata_json =
        read_optional_text_accounted(package_metadata.as_deref(), &mut aggregate_input_bytes)?;
    let package_vendor_json =
        read_optional_text_accounted(package_vendor.as_deref(), &mut aggregate_input_bytes)?;
    let bundle = collect_rsscript_bundle(RsscriptCollectInputs {
        review_map_json: review_map_json.as_deref(),
        package_review_json: package_review_json.as_deref(),
        package_check_json: package_check_json.as_deref(),
        package_lock_json: package_lock_json.as_deref(),
        package_lock_path: package_lock.as_deref(),
        lock_update_json: lock_update_json.as_deref(),
        package_tree_json: package_tree_json.as_deref(),
        package_publish_json: package_publish_json.as_deref(),
        package_metadata_json: package_metadata_json.as_deref(),
        package_vendor_json: package_vendor_json.as_deref(),
        package_name: package_name.as_deref(),
    })?;

    if strict {
        let error_diagnostics = bundle
            .facts
            .iter()
            .filter(|fact| fact.kind == reir::FactKind::Diagnostic && fact.unknown_reason.is_some())
            .count();
        if error_diagnostics > 0 {
            return Err(CliError::usage(format!(
                "--strict: refusing to emit REIR evidence built from {error_diagnostics} error \
                 diagnostic(s); fix the source and re-run"
            )));
        }
    }

    if let Some(out_path) = &out {
        write_json_file(out_path, &bundle)?;
        if !json {
            println!("collected rsscript evidence into {out_path}");
            print_bundle_summary(&bundle);
        }
    }
    if json || out.is_none() {
        print_json(&bundle)?;
    }

    Ok(ExitCode::SUCCESS)
}

fn try_run_report_pr(args: &[String]) -> Result<(ExitCode, String), CliError> {
    if wants_help(args) {
        print_usage();
        return Ok((ExitCode::SUCCESS, String::new()));
    }

    let mut required = None;
    let mut granted = None;
    let mut target = None;
    let mut principal = None;
    let mut ci_json = false;
    let mut sarif = false;
    let mut ci_json_out = None;
    let mut sarif_out = None;
    let mut policy_file = None;
    // CLI flag overrides, layered on top of any --policy file.
    let mut cli = reir::TargetGatePolicy::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--required" => required = Some(take_value(args, &mut index, "--required")?),
            "--granted" => granted = Some(take_value(args, &mut index, "--granted")?),
            "--target" => target = Some(take_value(args, &mut index, "--target")?),
            "--principal" => principal = Some(take_value(args, &mut index, "--principal")?),
            "--policy" => policy_file = Some(take_value(args, &mut index, "--policy")?),
            "--ci-json" => ci_json = true,
            "--sarif" => sarif = true,
            "--ci-json-out" => ci_json_out = Some(take_value(args, &mut index, "--ci-json-out")?),
            "--sarif-out" => sarif_out = Some(take_value(args, &mut index, "--sarif-out")?),
            "--fail-on-missing" => {
                set_policy_override(&mut cli.fail_on_missing, true, "missing capability policy")?
            }
            "--allow-missing" => {
                set_policy_override(&mut cli.fail_on_missing, false, "missing capability policy")?
            }
            "--fail-on-unknown" => {
                set_policy_override(&mut cli.fail_on_unknown, true, "unknown capability policy")?
            }
            "--allow-unknown" => {
                set_policy_override(&mut cli.fail_on_unknown, false, "unknown capability policy")?
            }
            "--fail-on-excess" => {
                set_policy_override(&mut cli.fail_on_excess, true, "excess capability policy")?
            }
            "--allow-excess" => {
                set_policy_override(&mut cli.fail_on_excess, false, "excess capability policy")?
            }
            "--require-verified-capabilities" => set_policy_override(
                &mut cli.require_verified_capabilities,
                true,
                "verified capability policy",
            )?,
            "--allow-unverified-capabilities" => set_policy_override(
                &mut cli.require_verified_capabilities,
                false,
                "verified capability policy",
            )?,
            unknown => {
                return Err(CliError::usage(format!(
                    "unknown report-pr flag `{unknown}`"
                )));
            }
        }
        index += 1;
    }

    // Resolve the gate policy: optional policy file for the target, then CLI overrides.
    let policy_config = match &policy_file {
        Some(path) => {
            let text = read_bounded_text(path).map_err(|error| match error {
                CliError::Usage(message) | CliError::Runtime(message) => CliError::usage(message),
            })?;
            Some(reir::GatePolicyFile::parse(&text).map_err(CliError::usage)?)
        }
        None => None,
    };
    let mut policy = match &policy_config {
        Some(config) => config
            .gate_policy_for(target.as_deref())
            .map_err(CliError::usage)?,
        None => reir::GatePolicy::production(),
    };
    cli.apply_to(&mut policy);

    if principal.is_none()
        && let (Some(config), Some(target)) = (&policy_config, target.as_deref())
    {
        principal = Some(
            config
                .principal_for(target)
                .map_err(CliError::usage)?
                .to_owned(),
        );
    }
    if principal.is_none() {
        return Err(CliError::usage(
            "report-pr requires an explicit --principal or `[target.<name>].principal` binding",
        ));
    }

    let required_path = required.ok_or_else(|| CliError::usage("missing --required <file>"))?;
    let granted_path = granted.ok_or_else(|| CliError::usage("missing --granted <file>"))?;
    let required_bundle = read_bundle(&required_path)?;
    let granted_bundle = read_bundle(&granted_path)?;
    required_bundle
        .validate_for_gate("required")
        .map_err(CliError::usage)?;
    granted_bundle
        .validate_for_gate("granted")
        .map_err(CliError::usage)?;
    let reconciliations = reir::reconcile_capabilities_for_gate(
        &required_bundle.facts,
        &granted_bundle.facts,
        target.as_deref(),
        principal.as_deref(),
    );

    let decision = reir::decide_validated_gate(
        &required_bundle.facts,
        &granted_bundle.facts,
        &reconciliations,
        policy,
    );
    let ci_output = reir::format_ci_gate_output_from_decision(
        &decision,
        &required_bundle.facts,
        &granted_bundle.facts,
        &reconciliations,
    );
    let ci_json_rendered = reir::format_ci_gate_json(&ci_output);
    let sarif_rendered = reir::format_sarif(&decision);
    if let Some(path) = ci_json_out {
        write_bounded_text_file(&path, &ci_json_rendered)?;
    }
    if let Some(path) = sarif_out {
        write_bounded_text_file(&path, &sarif_rendered)?;
    }
    let output = if sarif {
        sarif_rendered
    } else if ci_json {
        ci_json_rendered
    } else {
        format_pr_review_comment(
            &decision,
            &required_bundle.facts,
            &granted_bundle.facts,
            &reconciliations,
        )
    };
    // The exit code follows the (policy-aware) gate status so --fail-on-excess /
    // --fail-on-unknown actually block, not just annotate.
    let exit = if decision.status == reir::GateStatus::Fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };
    Ok((exit, output))
}

fn set_policy_override(slot: &mut Option<bool>, value: bool, name: &str) -> Result<(), CliError> {
    if slot.is_some_and(|existing| existing != value) {
        return Err(CliError::usage(format!(
            "conflicting command-line values for {name}"
        )));
    }
    *slot = Some(value);
    Ok(())
}

fn try_run_reconcile(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut required = None;
    let mut granted = None;
    let mut bundle = None;
    let mut out = None;
    let mut target = None;
    let mut json = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--required" => required = Some(take_value(args, &mut index, "--required")?),
            "--granted" => granted = Some(take_value(args, &mut index, "--granted")?),
            "--bundle" => bundle = Some(take_value(args, &mut index, "--bundle")?),
            "--target" => target = Some(take_value(args, &mut index, "--target")?),
            "--out" => out = Some(take_value(args, &mut index, "--out")?),
            "--json" => json = true,
            value if value.starts_with('-') => {
                return Err(CliError::usage(format!(
                    "unknown reconcile argument: {value}"
                )));
            }
            value => {
                if bundle.replace(value.to_owned()).is_some() {
                    return Err(CliError::usage("reconcile accepts a single bundle file"));
                }
            }
        }
        index += 1;
    }

    if let Some(bundle_path) = bundle {
        if required.is_some() || granted.is_some() {
            return Err(CliError::usage(
                "reconcile bundle mode cannot be combined with --required/--granted",
            ));
        }
        let mut bundle = read_bundle(&bundle_path)?;
        reconcile_bundle(&mut bundle, target.as_deref());

        if let Some(out_path) = &out {
            write_json_file(out_path, &bundle)?;
            if !json {
                println!("wrote reconciled bundle to {out_path}");
            }
        }
        if json {
            print_json(&bundle)?;
        } else {
            print_reconciliation_text(&bundle.reconciliations, &bundle, &bundle);
        }

        return Ok(exit_for_reconciliations(&bundle.reconciliations));
    }

    if out.is_some() {
        return Err(CliError::usage(
            "--out is only supported for reconcile bundle mode",
        ));
    }

    let required_path = required.ok_or_else(|| CliError::usage("missing --required <file>"))?;
    let granted_path = granted.ok_or_else(|| CliError::usage("missing --granted <file>"))?;

    let required_bundle = read_bundle(&required_path)?;
    let granted_bundle = read_bundle(&granted_path)?;
    let reconciliations = reconcile_capabilities_for_target(
        &required_bundle.facts,
        &granted_bundle.facts,
        target.as_deref(),
    );

    if json {
        print_json(&reconciliations)?;
    } else {
        print_reconciliation_text(&reconciliations, &required_bundle, &granted_bundle);
    }

    Ok(exit_for_reconciliations(&reconciliations))
}

fn run_diff(args: &[String]) -> ExitCode {
    match try_run_diff(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_diff(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut baseline = None;
    let mut current = None;
    let mut json = false;
    let mut fail_on_change = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--baseline" => baseline = Some(take_value(args, &mut index, "--baseline")?),
            "--current" => current = Some(take_value(args, &mut index, "--current")?),
            "--json" => json = true,
            "--fail-on-change" => fail_on_change = true,
            other => return Err(CliError::usage(format!("unknown diff argument: {other}"))),
        }
        index += 1;
    }

    let baseline_path = baseline.ok_or_else(|| CliError::usage("missing --baseline <file>"))?;
    let current_path = current.ok_or_else(|| CliError::usage("missing --current <file>"))?;

    let baseline_bundle = read_bundle(&baseline_path)?;
    let current_bundle = read_bundle(&current_path)?;
    let diff = compute_diff(&baseline_bundle, &current_bundle);

    if json {
        print_json(&diff)?;
    } else {
        print_diff_text(&diff);
    }

    Ok(exit_for_diff(&diff, fail_on_change))
}

fn run_slice(args: &[String]) -> ExitCode {
    match try_run_slice(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_slice(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut bundle = None;
    let mut filter_kind = None;
    let mut json = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bundle" => bundle = Some(take_value(args, &mut index, "--bundle")?),
            "--kind" => {
                filter_kind = Some(parse_slice_kind(&take_value(args, &mut index, "--kind")?)?)
            }
            "--json" => json = true,
            value if value.starts_with('-') => {
                return Err(CliError::usage(format!("unknown slice argument: {value}")));
            }
            value => {
                if bundle.replace(value.to_owned()).is_some() {
                    return Err(CliError::usage("slice accepts a single bundle file"));
                }
            }
        }
        index += 1;
    }

    let bundle_path = bundle.ok_or_else(|| CliError::usage("missing --bundle <file>"))?;
    let bundle = read_bundle(&bundle_path)?;
    let mut slices = slice_by_kind(&bundle);
    if let Some(kind) = filter_kind {
        slices.retain(|slice| slice.kind == kind);
    }

    if json {
        print_json(&slices)?;
    } else {
        print_slice_text(&slices);
    }

    Ok(ExitCode::SUCCESS)
}

fn run_merge(args: &[String]) -> ExitCode {
    match try_run_merge(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_merge(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut inputs = Vec::new();
    let mut out = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--out" => out = Some(take_value(args, &mut index, "--out")?),
            value if value.starts_with('-') => {
                return Err(CliError::usage(format!("unknown merge argument: {value}")));
            }
            value => inputs.push(value.to_owned()),
        }
        index += 1;
    }

    if inputs.is_empty() {
        return Err(CliError::usage("merge requires at least one input bundle"));
    }

    let out_path = out.ok_or_else(|| CliError::usage("missing --out <file>"))?;
    let merged = merge_bundles(&inputs)?;
    write_json_file(&out_path, &merged)?;

    println!("merged {} bundle(s) into {out_path}", inputs.len());
    print_bundle_summary(&merged);
    Ok(ExitCode::SUCCESS)
}

fn run_show(args: &[String]) -> ExitCode {
    match try_run_show(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_show(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut bundle = None;
    let mut json = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            value if value.starts_with('-') => {
                return Err(CliError::usage(format!("unknown show argument: {value}")));
            }
            value => {
                if bundle.replace(value.to_owned()).is_some() {
                    return Err(CliError::usage("show accepts a single bundle file"));
                }
            }
        }
        index += 1;
    }

    let bundle_path = bundle.ok_or_else(|| CliError::usage("missing bundle file"))?;
    let bundle = read_bundle(&bundle_path)?;

    if json {
        print_json(&bundle)?;
    } else {
        print_bundle_text(&bundle);
    }

    Ok(ExitCode::SUCCESS)
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, CliError> {
    *index += 1;
    let Some(value) = args.get(*index) else {
        return Err(CliError::usage(format!("missing value for {flag}")));
    };
    if value.starts_with("--") {
        return Err(CliError::usage(format!("missing value for {flag}")));
    }
    Ok(value.clone())
}

fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
}

fn read_bundle(path: &str) -> Result<Bundle, CliError> {
    let json = read_bounded_text(path)?;
    Bundle::from_json(&json)
        .map_err(|error| CliError::runtime(format!("failed to parse {path}: {error}")))
}

fn read_optional_text_accounted(
    path: Option<&str>,
    aggregate_bytes: &mut u64,
) -> Result<Option<String>, CliError> {
    path.map(|path| read_bounded_text_accounted(path, aggregate_bytes, MAX_CLI_INPUT_BYTES))
        .transpose()
}

fn read_bounded_text_accounted(
    path: &str,
    aggregate_bytes: &mut u64,
    aggregate_limit: u64,
) -> Result<String, CliError> {
    let remaining = aggregate_limit
        .checked_sub(*aggregate_bytes)
        .ok_or_else(|| CliError::runtime("aggregate input byte limit exceeded"))?;
    let text = read_bounded_text_with_limit(Path::new(path), remaining).map_err(|error| {
        if remaining < aggregate_limit {
            let detail = match error {
                CliError::Usage(message) | CliError::Runtime(message) => message,
            };
            CliError::runtime(format!(
                "aggregate input exceeds the {aggregate_limit} byte limit: {detail}"
            ))
        } else {
            error
        }
    })?;
    *aggregate_bytes = aggregate_bytes
        .checked_add(text.len() as u64)
        .ok_or_else(|| CliError::runtime("aggregate input byte count overflowed"))?;
    Ok(text)
}

fn read_bounded_text(path: &str) -> Result<String, CliError> {
    read_bounded_text_with_limit(Path::new(path), MAX_CLI_INPUT_BYTES)
}

fn read_bounded_text_with_limit(path: &Path, limit: u64) -> Result<String, CliError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CliError::runtime(format!("failed to inspect {}: {error}", path.display()))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(CliError::runtime(format!(
            "refusing to read symlink input {}",
            path.display()
        )));
    }
    if !metadata.is_file() {
        return Err(CliError::runtime(format!(
            "input is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > limit {
        return Err(CliError::runtime(format!(
            "input {} is {} bytes, exceeding the {limit} byte limit",
            path.display(),
            metadata.len()
        )));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options.open(path).map_err(|error| {
        CliError::runtime(format!("failed to read {}: {error}", path.display()))
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        CliError::runtime(format!("failed to inspect {}: {error}", path.display()))
    })?;
    if !opened_metadata.is_file() || !same_file(&metadata, &opened_metadata) {
        return Err(CliError::runtime(format!(
            "input changed while opening or is not a regular file: {}",
            path.display()
        )));
    }

    let mut bytes =
        Vec::with_capacity(usize::try_from(opened_metadata.len().min(limit)).unwrap_or(usize::MAX));
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CliError::runtime(format!("failed to read {}: {error}", path.display()))
        })?;
    if bytes.len() as u64 > limit {
        return Err(CliError::runtime(format!(
            "input {} exceeded the {limit} byte limit while reading",
            path.display()
        )));
    }
    String::from_utf8(bytes).map_err(|error| {
        CliError::runtime(format!(
            "failed to decode {} as UTF-8: {error}",
            path.display()
        ))
    })
}

struct RsscriptCollectInputs<'a> {
    review_map_json: Option<&'a str>,
    package_review_json: Option<&'a str>,
    package_check_json: Option<&'a str>,
    package_lock_json: Option<&'a str>,
    package_lock_path: Option<&'a str>,
    lock_update_json: Option<&'a str>,
    package_tree_json: Option<&'a str>,
    package_publish_json: Option<&'a str>,
    package_metadata_json: Option<&'a str>,
    package_vendor_json: Option<&'a str>,
    package_name: Option<&'a str>,
}

fn collect_rsscript_bundle(inputs: RsscriptCollectInputs<'_>) -> Result<Bundle, CliError> {
    let mut bundles = Vec::new();
    if inputs.review_map_json.is_some() || inputs.package_review_json.is_some() {
        bundles.push(
            rsscript_json_to_bundle(
                inputs.review_map_json,
                inputs.package_review_json,
                inputs.package_name,
            )
            .map_err(|error| {
                CliError::runtime(format!(
                    "failed to collect RSScript review evidence: {error}"
                ))
            })?,
        );
    }
    if let Some(json) = inputs.package_check_json {
        bundles.push(rsscript_check_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect RSScript check evidence: {error}"
            ))
        })?);
    }
    if let Some(json) = inputs.package_lock_json {
        let lock_json = package_lock_json_with_path(json, inputs.package_lock_path)?;
        bundles.push(rsscript_lock_json_to_bundle(&lock_json).map_err(|error| {
            CliError::runtime(format!("failed to collect RSScript lock evidence: {error}"))
        })?);
    }
    if let Some(json) = inputs.lock_update_json {
        bundles.push(rsscript_lock_diff_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect RSScript lock-update evidence: {error}"
            ))
        })?);
    }
    if let Some(json) = inputs.package_tree_json {
        bundles.push(rsscript_tree_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!("failed to collect RSScript tree evidence: {error}"))
        })?);
    }
    if let Some(json) = inputs.package_publish_json {
        bundles.push(rsscript_publish_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect RSScript publish evidence: {error}"
            ))
        })?);
    }
    if let Some(json) = inputs.package_metadata_json {
        bundles.push(rsscript_metadata_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect RSScript metadata evidence: {error}"
            ))
        })?);
    }
    if let Some(json) = inputs.package_vendor_json {
        bundles.push(rsscript_vendor_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect RSScript vendor evidence: {error}"
            ))
        })?);
    }
    if bundles.is_empty() {
        return Err(CliError::usage(
            "collect requires at least one RSScript JSON input",
        ));
    }
    merge_bundle_values(bundles)
}

fn package_lock_json_with_path(json: &str, path: Option<&str>) -> Result<String, CliError> {
    let Some(path) = path else {
        return Ok(json.to_owned());
    };
    let mut value: serde_json::Value = serde_json::from_str(json).map_err(|error| {
        CliError::runtime(format!("failed to parse RSScript lock evidence: {error}"))
    })?;
    if value
        .get("lockfile_path")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        value
            .as_object_mut()
            .ok_or_else(|| CliError::runtime("RSScript lock evidence must be a JSON object"))?
            .insert("lockfile_path".to_owned(), path.to_owned().into());
    }
    serde_json::to_string(&value).map_err(|error| {
        CliError::runtime(format!(
            "failed to prepare RSScript lock evidence with path: {error}"
        ))
    })
}

fn write_json_file(path: &str, bundle: &Bundle) -> Result<(), CliError> {
    let json = serde_json::to_string_pretty(bundle)
        .map_err(|error| CliError::runtime(format!("failed to serialize bundle: {error}")))?;
    if json.len() >= MAX_CLI_OUTPUT_BYTES {
        return Err(CliError::runtime(format!(
            "serialized bundle is too large to write within the {MAX_CLI_OUTPUT_BYTES} byte limit"
        )));
    }
    let mut output = json.into_bytes();
    output.push(b'\n');
    atomic_write_no_follow(Path::new(path), &output)
}

fn write_bounded_text_file(path: &str, output: &str) -> Result<(), CliError> {
    if output.len() > MAX_CLI_OUTPUT_BYTES {
        return Err(CliError::runtime(format!(
            "output is too large to write within the {MAX_CLI_OUTPUT_BYTES} byte limit"
        )));
    }
    atomic_write_no_follow(Path::new(path), output.as_bytes())
}

fn atomic_write_no_follow(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    reject_symlink_output(path)?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        CliError::runtime(format!("output path has no file name: {}", path.display()))
    })?;
    let mut temp_path = None;
    let mut temp_file = None;
    for _ in 0..128 {
        let sequence = OUTPUT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{}.reir-tmp-{}-{sequence}",
            file_name.to_string_lossy(),
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        set_no_follow(&mut options);
        match options.open(&candidate) {
            Ok(file) => {
                temp_path = Some(candidate);
                temp_file = Some(file);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(CliError::runtime(format!(
                    "failed to stage output {}: {error}",
                    path.display()
                )));
            }
        }
    }
    let temp_path = temp_path.ok_or_else(|| {
        CliError::runtime(format!(
            "failed to allocate a staging file for {}",
            path.display()
        ))
    })?;
    let mut temp_file = temp_file.expect("staging path and file are set together");
    let write_result = (|| {
        temp_file.write_all(bytes)?;
        temp_file.sync_all()?;
        drop(temp_file);
        reject_symlink_output(path).map_err(|error| match error {
            CliError::Usage(message) | CliError::Runtime(message) => std::io::Error::other(message),
        })?;
        fs::rename(&temp_path, path)?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(CliError::runtime(format!(
            "failed to atomically write {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

fn reject_symlink_output(path: &Path) -> Result<(), CliError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CliError::runtime(format!(
            "refusing to replace symlink output {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::runtime(format!(
            "failed to inspect output {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(unix)]
fn set_no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const O_NOFOLLOW: i32 = 0x100;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    const O_NOFOLLOW: i32 = 0x20000;
    options.custom_flags(O_NOFOLLOW);
}

#[cfg(not(unix))]
fn set_no_follow(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), CliError> {
    let output = bounded_json(value, MAX_CLI_OUTPUT_BYTES)?;
    std::io::stdout()
        .lock()
        .write_all(&output)
        .map_err(|error| CliError::runtime(format!("failed to write JSON to stdout: {error}")))
}

struct BoundedOutput {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let new_len = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= self.limit)
            .ok_or_else(|| std::io::Error::other("JSON output byte limit exceeded"))?;
        self.bytes.reserve(new_len - self.bytes.len());
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_json<T: serde::Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, CliError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer_pretty(&mut output, value)
        .map_err(|error| CliError::runtime(format!("failed to serialize JSON: {error}")))?;
    output.write_all(b"\n").map_err(|error| {
        CliError::runtime(format!(
            "JSON output exceeds the {limit} byte limit: {error}"
        ))
    })?;
    Ok(output.bytes)
}

fn merge_bundles(paths: &[String]) -> Result<Bundle, CliError> {
    if paths.len() > MAX_MERGE_INPUT_FILES {
        return Err(CliError::runtime(format!(
            "merge accepts at most {MAX_MERGE_INPUT_FILES} input files"
        )));
    }
    let mut aggregate_bytes = 0;
    let bundles = paths
        .iter()
        .map(|path| {
            let json =
                read_bounded_text_accounted(path, &mut aggregate_bytes, MAX_CLI_INPUT_BYTES)?;
            Bundle::from_json(&json)
                .map_err(|error| CliError::runtime(format!("failed to parse {path}: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    merge_bundle_values(bundles)
}

fn merge_bundle_values(bundles: Vec<Bundle>) -> Result<Bundle, CliError> {
    let mut bundles = bundles.into_iter();
    let mut merged = bundles.next().unwrap_or_default();
    let mut input_slices = merged.slices.clone();
    for bundle in bundles {
        if bundle.schema != merged.schema {
            return Err(CliError::runtime(format!(
                "cannot merge bundle with schema {} into {}",
                bundle.schema, merged.schema
            )));
        }
        if bundle.ontology != merged.ontology {
            return Err(CliError::runtime(format!(
                "cannot merge bundle with ontology {} into {}",
                bundle.ontology, merged.ontology
            )));
        }
        merged.producers.extend(bundle.producers);
        merged.subjects.extend(bundle.subjects);
        merged.subject_chains.extend(bundle.subject_chains);
        merged.facts.extend(bundle.facts);
        merged.edges.extend(bundle.edges);
        merged.reconciliations.extend(bundle.reconciliations);
        input_slices.extend(bundle.slices);
        merged.policy_results.extend(bundle.policy_results);
        merged.profiles.extend(bundle.profiles);
        merged.diffs.extend(bundle.diffs);
        merged.exceptions.extend(bundle.exceptions);
    }

    dedupe_bundle(&mut merged);
    rebuild_subject_index(&mut merged);
    merged.slices = merged_slices(input_slices, &merged);
    Ok(merged)
}

fn reconcile_bundle(bundle: &mut Bundle, target: Option<&str>) {
    let required = bundle
        .facts
        .iter()
        .filter(|fact| fact.role == Some(FactRole::Required))
        .cloned()
        .collect::<Vec<_>>();
    let granted = bundle
        .facts
        .iter()
        .filter(|fact| fact.role == Some(FactRole::Granted))
        .cloned()
        .collect::<Vec<_>>();
    bundle.reconciliations = reconcile_capabilities_for_target(&required, &granted, target);
    bundle.slices = slice_by_kind(bundle);
}

fn exit_for_reconciliations(reconciliations: &[Reconciliation]) -> ExitCode {
    if reconciliations
        .iter()
        .any(|reconciliation| reconciliation.status == ReconciliationStatus::Fail)
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn exit_for_diff(diff: &Diff, fail_on_change: bool) -> ExitCode {
    if fail_on_change && !diff.items.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn dedupe_bundle(bundle: &mut Bundle) {
    dedupe_by_key(&mut bundle.producers, producer_key);
    dedupe_by_key(&mut bundle.subject_chains, |chain| chain.id.clone());
    dedupe_by_key(&mut bundle.facts, |fact| fact.id.clone());
    dedupe_by_key(&mut bundle.edges, |edge| edge.id.clone());
    dedupe_by_key(&mut bundle.reconciliations, |reconciliation| {
        reconciliation.id.clone()
    });
    dedupe_by_key(&mut bundle.policy_results, |result| result.id.clone());
    dedupe_by_key(&mut bundle.profiles, |profile| profile.kind.clone());
    dedupe_by_key(&mut bundle.diffs, |diff| diff.id.clone());
    dedupe_by_key(&mut bundle.exceptions, |exception| exception.id.clone());
}

fn rebuild_subject_index(bundle: &mut Bundle) {
    let mut subjects = BTreeMap::new();
    for subject in &bundle.subjects {
        subjects.insert(subject.id.clone(), subject.clone());
    }
    for fact in &bundle.facts {
        subjects.insert(fact.subject.id.clone(), fact.subject.clone());
    }
    for edge in &bundle.edges {
        subjects.insert(edge.from.id.clone(), edge.from.clone());
        subjects.insert(edge.to.id.clone(), edge.to.clone());
    }
    for chain in &bundle.subject_chains {
        for subject in &chain.nodes {
            subjects.insert(subject.id.clone(), subject.clone());
        }
    }
    bundle.subjects = subjects.into_values().collect();
}

fn merged_slices(input_slices: Vec<Slice>, bundle: &Bundle) -> Vec<Slice> {
    let mut slices = BTreeMap::new();
    for slice in input_slices {
        slices.insert(slice.id.clone(), slice);
    }
    for slice in slice_by_kind(bundle) {
        slices.insert(slice.id.clone(), slice);
    }
    slices.into_values().collect()
}

fn dedupe_by_key<T>(items: &mut Vec<T>, key: impl Fn(&T) -> String) {
    let mut unique = BTreeMap::new();
    for item in items.drain(..) {
        unique.entry(key(&item)).or_insert(item);
    }
    *items = unique.into_values().collect();
}

fn producer_key(producer: &reir::subject::Producer) -> String {
    serde_json::to_string(producer).expect("producer should serialize")
}

fn print_reconciliation_text(
    reconciliations: &[Reconciliation],
    required: &Bundle,
    granted: &Bundle,
) {
    println!("REIR RECONCILIATION\n");
    println!("status: {}", overall_reconciliation_status(reconciliations));

    if reconciliations.is_empty() {
        println!("\n(no reconciliation items)");
        return;
    }

    for reconciliation in reconciliations {
        println!();
        println!("{}:", reconciliation_kind_label(&reconciliation.kind));
        if let Some(target) = &reconciliation.target {
            println!("  target: {target}");
        }
        if let Some(capability) = &reconciliation.capability {
            println!("  {}", display_capability(capability));
        }
        if let Some(required_fact_id) = &reconciliation.required_fact {
            if let Some(fact) = find_fact(&required.facts, required_fact_id) {
                println!("  required by: {}", display_fact_subject(fact));
            } else {
                println!("  required fact: {required_fact_id}");
            }
        }
        if !reconciliation.granted_facts.is_empty() {
            for granted_fact_id in &reconciliation.granted_facts {
                if let Some(fact) = find_fact(&granted.facts, granted_fact_id) {
                    println!("  granted to: {}", display_fact_subject(fact));
                } else {
                    println!("  granted fact: {granted_fact_id}");
                }
            }
        }
        if let Some(observed_fact_id) = &reconciliation.observed_fact {
            println!("  observed fact: {observed_fact_id}");
        }
        if let Some(subject_chain) = &reconciliation.subject_chain {
            println!("  subject chain: {subject_chain}");
        }
        if let Some(evidence) = display_evidence(&reconciliation.evidence) {
            println!("  evidence: {evidence}");
        }
        if let Some(reason) = reconciliation
            .risk
            .as_ref()
            .and_then(|risk| risk.reason.as_deref())
        {
            println!("  reason: {reason}");
        }
    }
}

fn print_diff_text(diff: &Diff) {
    println!("REIR DIFF\n");
    println!("items: {}", diff.items.len());

    if diff.items.is_empty() {
        println!("\n(no diff items)");
        return;
    }

    for item in &diff.items {
        print_diff_item(item);
    }
}

fn print_diff_item(item: &DiffItem) {
    println!();
    println!("{}: {}", diff_item_kind_label(&item.kind), item.id);
    if let Some(subject) = &item.subject {
        println!("  subject: {}", subject.id);
    }
    if let Some(description) = &item.description {
        println!("  description: {description}");
    }
    if let Some(evidence) = display_evidence(&item.evidence) {
        println!("  evidence: {evidence}");
    }
}

fn print_slice_text(slices: &[Slice]) {
    println!("REIR SLICES\n");
    println!("slices: {}", slices.len());

    if slices.is_empty() {
        println!("\n(no slices)");
        return;
    }

    for slice in slices {
        println!();
        println!("{}:", slice_kind_label(&slice.kind));
        println!("  id: {}", slice.id);
        println!("  facts: {}", slice.facts.len());
        for fact in &slice.facts {
            println!("    - {fact}");
        }
        println!("  reconciliations: {}", slice.reconciliations.len());
        for reconciliation in &slice.reconciliations {
            println!("    - {reconciliation}");
        }
        if let Some(evidence) = display_evidence(&slice.evidence) {
            println!("  evidence: {evidence}");
        }
    }
}

fn print_bundle_text(bundle: &Bundle) {
    println!("REIR BUNDLE\n");
    println!("schema: {}", bundle.schema);
    println!("ontology: {}", bundle.ontology);
    print_bundle_summary(bundle);
}

fn print_bundle_summary(bundle: &Bundle) {
    println!("producers: {}", bundle.producers.len());
    println!("subjects: {}", bundle.subjects.len());
    println!("subject_chains: {}", bundle.subject_chains.len());
    println!("facts: {}", bundle.facts.len());
    println!("edges: {}", bundle.edges.len());
    println!("reconciliations: {}", bundle.reconciliations.len());
    println!("slices: {}", bundle.slices.len());
    println!("policy_results: {}", bundle.policy_results.len());
    println!("profiles: {}", bundle.profiles.len());
    println!("diffs: {}", bundle.diffs.len());
    println!("exceptions: {}", bundle.exceptions.len());
}

fn display_capability(capability: &Capability) -> String {
    let capability_name = capability
        .action
        .clone()
        .unwrap_or_else(|| String::from(capability.category.clone()));

    match &capability.resource {
        Some(resource) => format!("{capability_name} on {resource}"),
        None => capability_name,
    }
}

fn display_fact_subject(fact: &Fact) -> String {
    fact.subject
        .name
        .clone()
        .unwrap_or_else(|| fact.subject.id.clone())
}

fn display_evidence(evidence: &[Evidence]) -> Option<String> {
    evidence
        .iter()
        .find_map(|entry| {
            entry
                .file
                .as_ref()
                .map(|file| match (entry.line, entry.column) {
                    (Some(line), Some(column)) => format!("{file}:{line}:{column}"),
                    (Some(line), None) => format!("{file}:{line}"),
                    _ => file.clone(),
                })
        })
        .or_else(|| {
            evidence
                .iter()
                .find_map(|entry| entry.symbol.clone().or_else(|| entry.reason.clone()))
        })
}

fn find_fact<'a>(facts: &'a [Fact], id: &str) -> Option<&'a Fact> {
    facts.iter().find(|fact| fact.id == id)
}

fn overall_reconciliation_status(reconciliations: &[Reconciliation]) -> &'static str {
    if reconciliations
        .iter()
        .any(|reconciliation| reconciliation.status == ReconciliationStatus::Fail)
    {
        "fail"
    } else if reconciliations
        .iter()
        .any(|reconciliation| reconciliation.status == ReconciliationStatus::Warn)
    {
        "warn"
    } else if reconciliations
        .iter()
        .any(|reconciliation| reconciliation.status == ReconciliationStatus::Unknown)
    {
        "unknown"
    } else {
        "pass"
    }
}

fn reconciliation_kind_label(kind: &ReconciliationKind) -> String {
    match kind {
        ReconciliationKind::Covered => "covered capability".to_owned(),
        ReconciliationKind::MissingCapability => "missing capability".to_owned(),
        ReconciliationKind::ExcessCapability => "excess capability".to_owned(),
        ReconciliationKind::UnexpectedObservation => "unexpected observation".to_owned(),
        ReconciliationKind::UnauthorizedObservation => "unauthorized observation".to_owned(),
        ReconciliationKind::UnusedCapability => "unused capability".to_owned(),
        ReconciliationKind::PartialCoverage => "partial coverage".to_owned(),
        ReconciliationKind::UnknownCoverage => "unknown coverage".to_owned(),
        ReconciliationKind::ChainIncomplete => "chain incomplete".to_owned(),
        ReconciliationKind::Extension(value) => value.replace('_', " "),
    }
}

fn diff_item_kind_label(kind: &DiffItemKind) -> String {
    match kind {
        DiffItemKind::FactAdded => "fact added".to_owned(),
        DiffItemKind::FactRemoved => "fact removed".to_owned(),
        DiffItemKind::FactChanged => "fact changed".to_owned(),
        DiffItemKind::EdgeAdded => "edge added".to_owned(),
        DiffItemKind::EdgeRemoved => "edge removed".to_owned(),
        DiffItemKind::EdgeChanged => "edge changed".to_owned(),
        DiffItemKind::SubjectChainAdded => "subject chain added".to_owned(),
        DiffItemKind::SubjectChainRemoved => "subject chain removed".to_owned(),
        DiffItemKind::SubjectChainChanged => "subject chain changed".to_owned(),
        DiffItemKind::ReconciliationAdded => "reconciliation added".to_owned(),
        DiffItemKind::ReconciliationRemoved => "reconciliation removed".to_owned(),
        DiffItemKind::ReconciliationChanged => "reconciliation changed".to_owned(),
        DiffItemKind::SliceAdded => "slice added".to_owned(),
        DiffItemKind::SliceRemoved => "slice removed".to_owned(),
        DiffItemKind::SliceChanged => "slice changed".to_owned(),
        DiffItemKind::DiffAdded => "diff added".to_owned(),
        DiffItemKind::DiffRemoved => "diff removed".to_owned(),
        DiffItemKind::DiffChanged => "diff changed".to_owned(),
        DiffItemKind::PolicyResultAdded => "policy result added".to_owned(),
        DiffItemKind::PolicyResultRemoved => "policy result removed".to_owned(),
        DiffItemKind::PolicyResultChanged => "policy result changed".to_owned(),
        DiffItemKind::ProfileRuleChanged => "profile rule changed".to_owned(),
        DiffItemKind::ExceptionAdded => "exception added".to_owned(),
        DiffItemKind::ExceptionExpired => "exception expired".to_owned(),
        DiffItemKind::ExceptionChanged => "exception changed".to_owned(),
        DiffItemKind::SchemaChanged => "schema changed".to_owned(),
        DiffItemKind::ProducerChanged => "producer changed".to_owned(),
        DiffItemKind::OntologyChanged => "ontology changed".to_owned(),
        DiffItemKind::Extension(value) => value.replace('_', " "),
    }
}

fn slice_kind_label(kind: &SliceKind) -> String {
    match kind {
        SliceKind::MissingCapabilitySlice => "missing_capability".to_owned(),
        SliceKind::ExcessCapabilitySlice => "excess_capability".to_owned(),
        SliceKind::UnexpectedObservationSlice => "unexpected_observation".to_owned(),
        SliceKind::NetworkSlice => "network".to_owned(),
        SliceKind::PublicIngressSlice => "public_ingress".to_owned(),
        SliceKind::ObjectStorageSlice => "object_storage".to_owned(),
        SliceKind::DatabaseSlice => "database".to_owned(),
        SliceKind::SecretSlice => "secret".to_owned(),
        SliceKind::EnvSlice => "env".to_owned(),
        SliceKind::TimeSlice => "time".to_owned(),
        SliceKind::RandomnessSlice => "randomness".to_owned(),
        SliceKind::ComputeSlice => "compute".to_owned(),
        SliceKind::TelemetrySlice => "telemetry".to_owned(),
        SliceKind::ProcessSlice => "process".to_owned(),
        SliceKind::AsyncSlice => "async".to_owned(),
        SliceKind::DiagnosticSlice => "diagnostic".to_owned(),
        SliceKind::PackageFeatureSlice => "package_feature".to_owned(),
        SliceKind::ProviderImplementationSlice => "provider_implementation".to_owned(),
        SliceKind::FilesystemSlice => "filesystem".to_owned(),
        SliceKind::IdentitySlice => "identity".to_owned(),
        SliceKind::RbacSlice => "rbac".to_owned(),
        SliceKind::StorageSlice => "storage".to_owned(),
        SliceKind::BuildTimeSlice => "build_time".to_owned(),
        SliceKind::NativeUnsafeSlice => "native_unsafe".to_owned(),
        SliceKind::PackageRiskSlice => "package_risk".to_owned(),
        SliceKind::RuntimeDriftSlice => "runtime_drift".to_owned(),
        SliceKind::SubjectChainSlice => "subject_chain".to_owned(),
        SliceKind::DiffSlice => "diff".to_owned(),
        SliceKind::UnknownSlice => "unknown".to_owned(),
        SliceKind::Extension(value) => value.clone(),
    }
}

fn parse_slice_kind(value: &str) -> Result<SliceKind, CliError> {
    match value {
        "missing_capability" | "missing_capability_slice" => Ok(SliceKind::MissingCapabilitySlice),
        "excess_capability" | "excess_capability_slice" => Ok(SliceKind::ExcessCapabilitySlice),
        "unexpected_observation" | "unexpected_observation_slice" => {
            Ok(SliceKind::UnexpectedObservationSlice)
        }
        "network" | "network_slice" => Ok(SliceKind::NetworkSlice),
        "public_ingress" | "public_ingress_slice" => Ok(SliceKind::PublicIngressSlice),
        "object_storage" | "object_storage_slice" => Ok(SliceKind::ObjectStorageSlice),
        "database" | "database_slice" => Ok(SliceKind::DatabaseSlice),
        "secret" | "secret_slice" => Ok(SliceKind::SecretSlice),
        "env" | "env_slice" => Ok(SliceKind::EnvSlice),
        "time" | "time_slice" => Ok(SliceKind::TimeSlice),
        "randomness" | "randomness_slice" => Ok(SliceKind::RandomnessSlice),
        "compute" | "compute_slice" => Ok(SliceKind::ComputeSlice),
        "telemetry" | "telemetry_slice" => Ok(SliceKind::TelemetrySlice),
        "process" | "process_slice" => Ok(SliceKind::ProcessSlice),
        "async" | "async_slice" => Ok(SliceKind::AsyncSlice),
        "diagnostic" | "diagnostic_slice" => Ok(SliceKind::DiagnosticSlice),
        "package_feature" | "package_feature_slice" => Ok(SliceKind::PackageFeatureSlice),
        "provider_implementation" | "provider_implementation_slice" => {
            Ok(SliceKind::ProviderImplementationSlice)
        }
        "filesystem" | "filesystem_slice" => Ok(SliceKind::FilesystemSlice),
        "identity" | "identity_slice" => Ok(SliceKind::IdentitySlice),
        "rbac" | "rbac_slice" => Ok(SliceKind::RbacSlice),
        "storage" | "storage_slice" => Ok(SliceKind::StorageSlice),
        "build_time" | "build_time_slice" => Ok(SliceKind::BuildTimeSlice),
        "native_unsafe" | "native_unsafe_slice" => Ok(SliceKind::NativeUnsafeSlice),
        "package_risk" | "package_risk_slice" => Ok(SliceKind::PackageRiskSlice),
        "runtime_drift" | "runtime_drift_slice" => Ok(SliceKind::RuntimeDriftSlice),
        "subject_chain" | "subject_chain_slice" => Ok(SliceKind::SubjectChainSlice),
        "diff" | "diff_slice" => Ok(SliceKind::DiffSlice),
        "unknown" | "unknown_slice" => Ok(SliceKind::UnknownSlice),
        _ => Err(CliError::usage(format!("unknown slice kind: {value}"))),
    }
}

fn report_error(error: CliError) -> ExitCode {
    match error {
        CliError::Usage(message) => {
            eprintln!("{message}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        CliError::Runtime(message) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

fn print_usage() {
    println!("{USAGE}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use reir::{
        AcquisitionMode, CapabilityCategory, Confidence, ConfidenceLevel, Edge, EdgeKind,
        EvidenceKind, FactKind, FactRole, FactValue, Precision, Profile, ProfileBudget, Subject,
        SubjectKind,
    };
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn subject(id: &str) -> Subject {
        Subject {
            kind: SubjectKind::Package,
            id: id.to_owned(),
            name: Some(id.to_owned()),
            package: None,
        }
    }

    fn confidence() -> Confidence {
        Confidence {
            level: ConfidenceLevel::Computed,
            source: Some("test".to_owned()),
        }
    }

    fn package_risk_fact(id: &str, subject: Subject) -> Fact {
        Fact {
            schema: "reir.fact.v0.1".to_owned(),
            id: id.to_owned(),
            kind: FactKind::PackageRisk,
            role: Some(FactRole::Observed),
            subject,
            capability: None,
            value: FactValue::True,
            confidence: confidence(),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Exact,
            evidence: Vec::new(),
            unknown_reason: None,
        }
    }

    fn capability_fact(id: &str, role: FactRole, subject: Subject, action: &str) -> Fact {
        capability_fact_with_category(
            id,
            role,
            subject,
            CapabilityCategory::ObjectStorageWrite,
            action,
        )
    }

    fn capability_fact_with_category(
        id: &str,
        role: FactRole,
        subject: Subject,
        category: CapabilityCategory,
        action: &str,
    ) -> Fact {
        let principal =
            matches!(role, FactRole::Granted | FactRole::Denied).then(|| subject.id.clone());
        Fact {
            schema: "reir.fact.v0.1".to_owned(),
            id: id.to_owned(),
            kind: FactKind::Capability,
            role: Some(role),
            subject,
            capability: Some(Capability {
                category,
                provider: Some("aws".to_owned()),
                service: Some("s3".to_owned()),
                action: Some(action.to_owned()),
                resource: Some("arn:aws:s3:::reports-prod/*".to_owned()),
                constraints: std::collections::HashMap::new(),
            }),
            value: FactValue::True,
            confidence: confidence(),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::ResourceScoped,
            evidence: vec![Evidence {
                kind: EvidenceKind::PackageMetadata,
                file: Some("test-fixture".to_owned()),
                line: None,
                column: None,
                length: None,
                symbol: None,
                reason: Some("test capability evidence".to_owned()),
                json_pointer: None,
                resource: None,
                provider: None,
                value: None,
                event_id: None,
                time: None,
                source: Some("test".to_owned()),
                event_name: None,
                principal,
                account: None,
                policy_arn: None,
                statement_index: None,
                action: Some(action.to_owned()),
            }],
            unknown_reason: None,
        }
    }

    fn edge(id: &str, from: Subject, to: Subject) -> Edge {
        Edge {
            schema: "reir.edge.v0.1".to_owned(),
            id: id.to_owned(),
            kind: EdgeKind::DependsOn,
            from,
            to,
            confidence: confidence(),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Exact,
            evidence: Vec::new(),
        }
    }

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        path.push(format!("reir-{name}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).expect("temp dir should be created");
        path
    }

    #[test]
    fn take_value_rejects_next_flag_as_missing_value() {
        let args = vec!["--out".to_string(), "--json".to_string()];
        let mut index = 0;

        let error = take_value(&args, &mut index, "--out").expect_err("flag value should fail");

        assert!(matches!(error, CliError::Usage(message) if message == "missing value for --out"));
    }

    #[test]
    fn merge_bundle_values_dedupes_and_rebuilds_subject_index_and_slices() {
        let package = subject("pkg-a@0.1.0");
        let dependency = subject("pkg-b@0.1.0");
        let risk = package_risk_fact("fact.package_risk.pkg-a", package.clone());
        let dependency_edge = edge(
            "edge.depends.pkg-a.pkg-b",
            package.clone(),
            dependency.clone(),
        );

        let mut first = Bundle::new();
        first.producers.push(reir::subject::Producer {
            name: "rsscript".to_owned(),
            version: "0.1.0".to_owned(),
            adapter: Some("rsscript".to_owned()),
            adapter_version: None,
            source: None,
        });
        first.subjects.push(package.clone());
        first.facts.push(risk.clone());
        first.profiles.push(Profile {
            kind: "prod".to_owned(),
            allow: HashMap::new(),
            budget: Some(ProfileBudget {
                max_missing_capabilities: 0,
                max_unknown_coverage: 0,
                max_excess_grants: 1,
            }),
        });
        first.slices.push(Slice {
            schema: "reir.slice.v0.1".to_owned(),
            id: "slice.package_risk".to_owned(),
            kind: SliceKind::PackageRiskSlice,
            facts: vec!["stale.fact".to_owned()],
            edges: Vec::new(),
            reconciliations: Vec::new(),
            evidence: Vec::new(),
        });

        let mut second = Bundle::new();
        second.producers = first.producers.clone();
        second.subjects.push(package.clone());
        second.facts.push(risk);
        second.profiles = first.profiles.clone();
        second.edges.push(dependency_edge);

        let merged = merge_bundle_values(vec![first, second]).expect("merge should succeed");

        assert_eq!(merged.producers.len(), 1);
        assert_eq!(merged.facts.len(), 1);
        assert_eq!(merged.edges.len(), 1);
        assert_eq!(merged.profiles.len(), 1);
        assert_eq!(merged.profiles[0].kind, "prod");
        assert!(
            merged
                .subjects
                .iter()
                .any(|subject| subject.id == package.id)
        );
        assert!(
            merged
                .subjects
                .iter()
                .any(|subject| subject.id == dependency.id)
        );
        assert_eq!(merged.subjects.len(), 2);

        let package_slice = merged
            .slices
            .iter()
            .find(|slice| slice.kind == SliceKind::PackageRiskSlice)
            .expect("package risk slice should be recomputed");
        assert_eq!(
            package_slice.facts,
            vec!["fact.package_risk.pkg-a".to_owned()]
        );
        assert!(!package_slice.facts.contains(&"stale.fact".to_owned()));
    }

    #[test]
    fn collect_rsscript_bundle_merges_package_manager_inputs() {
        let package_review = r#"{
            "package": { "name": "demo", "version": "0.1.0" },
            "risk": "low",
            "summary": {
                "public_apis": 1,
                "mutating_apis": 0,
                "retaining_apis": 0,
                "resource_apis": 0,
                "native_apis": 0,
                "unsafe_apis": 0,
                "unknown_apis": 0
            },
            "exports": [
                { "name": "Api.run", "kind": "function", "classification": "low_semantic_risk" }
            ]
        }"#;
        let package_check = r#"{
            "package": { "name": "demo", "version": "0.1.0", "edition": "2026" },
            "package_dir": "/tmp/demo",
            "ok": false,
            "risk": "elevated",
            "reasons": ["rsspkg.lock missing"],
            "summary": { "diagnostics": 0, "errors": 0, "dependencies": 0 },
            "graph": { "ok": true, "risk": "low", "reasons": [] },
            "lock": {
                "path": "/tmp/demo/rsspkg.lock",
                "present": false,
                "matches": false,
                "risk": "elevated",
                "reasons": ["rsspkg.lock missing"],
                "package_changes": []
            },
            "diagnostics": []
        }"#;
        let package_lock = r#"{
            "version": 1,
            "package": [
                {
                    "name": "demo",
                    "version": "0.1.0",
                    "source": "path+/tmp/demo",
                    "checksum": "sha256:pkg",
                    "interface_hash": "sha256:interface",
                    "review_hash": "sha256:review",
                    "features": []
                }
            ]
        }"#;

        let bundle = collect_rsscript_bundle(RsscriptCollectInputs {
            review_map_json: None,
            package_review_json: Some(package_review),
            package_check_json: Some(package_check),
            package_lock_json: Some(package_lock),
            package_lock_path: None,
            lock_update_json: None,
            package_tree_json: None,
            package_publish_json: None,
            package_metadata_json: None,
            package_vendor_json: None,
            package_name: None,
        })
        .expect("RSScript package-manager JSON should collect");

        assert_eq!(bundle.producers.len(), 3);
        assert!(bundle.facts.iter().any(|fact| {
            fact.id == "fact.package.demo_0_1_0.risk" && fact.kind == FactKind::PackageRisk
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.id == "fact.package_check.demo_0_1_0.status" && fact.kind == FactKind::PolicyResult
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.id == "fact.lockfile.demo_0_1_0.effective_interface_hash"
                && fact.kind == FactKind::SupplyChain
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.lockfile.demo_0_1_0.effective_interface_hash".to_owned())
        }));
    }

    #[test]
    fn collect_rsscript_cli_reads_package_manager_inputs_and_writes_bundle() {
        let temp_dir = unique_temp_dir("collect-rsscript-cli");
        let review_path = temp_dir.join("package-review.json");
        let check_path = temp_dir.join("package-check.json");
        let lock_path = temp_dir.join("rsspkg.lock.json");
        let out_path = temp_dir.join("bundle.json");
        std::fs::write(
            &review_path,
            r#"{
                "package": { "name": "demo", "version": "0.1.0" },
                "risk": "low",
                "summary": {
                    "public_apis": 1,
                    "mutating_apis": 0,
                    "retaining_apis": 0,
                    "resource_apis": 0,
                    "native_apis": 0,
                    "unsafe_apis": 0,
                    "unknown_apis": 0
                },
                "exports": [
                    {
                        "name": "Api.run",
                        "kind": "function",
                        "classification": "low_semantic_risk"
                    }
                ]
            }"#,
        )
        .expect("package review fixture should be written");
        std::fs::write(
            &check_path,
            r#"{
                "package": { "name": "demo", "version": "0.1.0", "edition": "2026" },
                "package_dir": "/tmp/demo",
                "ok": false,
                "risk": "elevated",
                "reasons": ["rsspkg.lock missing"],
                "summary": { "diagnostics": 0, "errors": 0, "dependencies": 0 },
                "graph": { "ok": true, "risk": "low", "reasons": [] },
                "lock": {
                    "path": "/tmp/demo/rsspkg.lock",
                    "present": false,
                    "matches": false,
                    "risk": "elevated",
                    "reasons": ["rsspkg.lock missing"],
                    "package_changes": []
                },
                "diagnostics": []
            }"#,
        )
        .expect("package check fixture should be written");
        std::fs::write(
            &lock_path,
            r#"{
                "version": 1,
                "package": [
                    {
                        "name": "demo",
                        "version": "0.1.0",
                        "source": "path+/tmp/demo",
                        "checksum": "sha256:pkg",
                        "interface_hash": "sha256:interface",
                        "review_hash": "sha256:review",
                        "features": []
                    }
                ]
            }"#,
        )
        .expect("package lock fixture should be written");

        let args = vec![
            "--producer".to_owned(),
            "rsscript".to_owned(),
            "--package-review".to_owned(),
            review_path.to_string_lossy().into_owned(),
            "--package-check".to_owned(),
            check_path.to_string_lossy().into_owned(),
            "--package-lock".to_owned(),
            lock_path.to_string_lossy().into_owned(),
            "--out".to_owned(),
            out_path.to_string_lossy().into_owned(),
            "--json".to_owned(),
        ];

        let code = try_run_collect(&args).expect("collect command should succeed");
        assert_eq!(code, ExitCode::SUCCESS);

        let bundle_json =
            std::fs::read_to_string(&out_path).expect("collect command should write bundle");
        let bundle = Bundle::from_json(&bundle_json).expect("written bundle should parse");
        assert_eq!(bundle.producers.len(), 3);
        assert!(bundle.facts.iter().any(|fact| {
            fact.id == "fact.package.demo_0_1_0.risk" && fact.kind == FactKind::PackageRisk
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.id == "fact.package_check.demo_0_1_0.status" && fact.kind == FactKind::PolicyResult
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.id == "fact.lockfile.demo_0_1_0.effective_interface_hash"
                && fact.kind == FactKind::SupplyChain
                && fact.evidence[0].file.as_deref() == Some(lock_path.to_string_lossy().as_ref())
        }));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn report_pr_cli_reads_bundles_and_outputs_all_missing_capabilities() {
        let temp_dir = unique_temp_dir("report-pr");
        let required_path = temp_dir.join("required.json");
        let granted_path = temp_dir.join("granted.json");
        let ci_json_path = temp_dir.join("decision.json");
        let sarif_path = temp_dir.join("decision.sarif");
        let function = Subject {
            kind: SubjectKind::CodeFunction,
            id: "reports::Reports.cleanup_old_reports".to_owned(),
            name: Some("Reports.cleanup_old_reports".to_owned()),
            package: Some("reports".to_owned()),
        };
        let role = Subject {
            kind: SubjectKind::CloudRole,
            id: "arn:aws:iam::123456789012:role/report-uploader".to_owned(),
            name: Some("report-uploader".to_owned()),
            package: Some("reports".to_owned()),
        };
        let required = Bundle {
            facts: vec![
                capability_fact_with_category(
                    "fact.reports.cleanup.requires.s3_put_object",
                    FactRole::Required,
                    function.clone(),
                    CapabilityCategory::ObjectStorageWrite,
                    "s3:PutObject",
                ),
                capability_fact_with_category(
                    "fact.reports.cleanup.requires.s3_delete_object",
                    FactRole::Required,
                    function,
                    CapabilityCategory::ObjectStorageDelete,
                    "s3:DeleteObject",
                ),
            ],
            ..Bundle::new()
        };
        let granted = Bundle {
            facts: vec![capability_fact_with_category(
                "fact.reports.role.grants.s3_get_object",
                FactRole::Granted,
                role,
                CapabilityCategory::ObjectStorageRead,
                "s3:GetObject",
            )],
            ..Bundle::new()
        };
        std::fs::write(
            &required_path,
            required
                .to_json()
                .expect("required bundle should serialize"),
        )
        .expect("required bundle should be written");
        std::fs::write(
            &granted_path,
            granted.to_json().expect("granted bundle should serialize"),
        )
        .expect("granted bundle should be written");

        let args = vec![
            "--required".to_owned(),
            required_path.to_string_lossy().into_owned(),
            "--granted".to_owned(),
            granted_path.to_string_lossy().into_owned(),
            "--target".to_owned(),
            "prod".to_owned(),
            "--principal".to_owned(),
            "arn:aws:iam::123456789012:role/report-uploader".to_owned(),
            "--ci-json-out".to_owned(),
            ci_json_path.to_string_lossy().into_owned(),
            "--sarif-out".to_owned(),
            sarif_path.to_string_lossy().into_owned(),
        ];
        let (code, comment) = try_run_report_pr(&args).expect("report-pr should run");
        let ci_json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&ci_json_path).expect("CI JSON should be written"),
        )
        .expect("CI output should be JSON");
        let sarif: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sarif_path).expect("SARIF should be written"))
                .expect("SARIF output should be JSON");
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert_eq!(code, ExitCode::from(1));
        assert_eq!(ci_json["status"], "fail");
        assert_eq!(sarif["version"], "2.1.0");
        assert!(comment.contains("Status: FAIL"));
        assert!(comment.contains("### Required capabilities needing deployment grant"));
        assert!(comment.contains("object_storage.write aws/s3 s3:PutObject"));
        assert!(comment.contains("object_storage.delete aws/s3 s3:DeleteObject"));
        assert!(comment.contains("s3:PutObject on arn:aws:s3:::reports-prod/*"));
        assert!(comment.contains("s3:DeleteObject on arn:aws:s3:::reports-prod/*"));
    }

    #[test]
    fn collect_cli_rejects_planned_non_rsscript_producers() {
        let args = vec![
            "--producer".to_owned(),
            "k8s".to_owned(),
            "--from".to_owned(),
            "rendered/prod".to_owned(),
        ];

        let error = try_run_collect(&args).expect_err("planned producer should fail");

        match error {
            CliError::Usage(message) => assert_eq!(
                message,
                "`reir collect --producer k8s` is planned; this build supports `rsscript` and `terraform`."
            ),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn collect_terraform_cli_reads_iam_policy_and_writes_granted_bundle() {
        let temp_dir = unique_temp_dir("collect-terraform-cli");
        let out_path = temp_dir.join("terraform.reir.json");
        std::fs::write(
            temp_dir.join("main.tf"),
            r#"resource "aws_iam_role_policy" "report_uploader" {
  role = "report-uploader"
  policy = <<POLICY
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": "s3:PutObject",
      "Resource": "arn:aws:s3:::reports-prod/*"
    }
  ]
}
POLICY
}
"#,
        )
        .expect("Terraform fixture should be written");

        let args = vec![
            "--producer".to_owned(),
            "terraform".to_owned(),
            "--from".to_owned(),
            temp_dir.to_string_lossy().into_owned(),
            "--out".to_owned(),
            out_path.to_string_lossy().into_owned(),
            "--json".to_owned(),
        ];

        let code = try_run_collect(&args).expect("Terraform collect should succeed");
        assert_eq!(code, ExitCode::SUCCESS);

        let bundle_json =
            std::fs::read_to_string(&out_path).expect("collect command should write bundle");
        let bundle = Bundle::from_json(&bundle_json).expect("written bundle should parse");
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert!(bundle.facts.iter().any(|fact| {
            fact.role == Some(FactRole::Granted)
                && fact.value == FactValue::Unknown
                && fact.acquisition_mode == AcquisitionMode::SourceScan
                && fact
                    .evidence
                    .iter()
                    .all(|evidence| evidence.kind == EvidenceKind::SourceTemplatePointer)
                && fact.capability.as_ref().is_some_and(|capability| {
                    capability.action.as_deref() == Some("s3:PutObject")
                        && capability.resource.as_deref() == Some("arn:aws:s3:::reports-prod/*")
                })
        }));
    }

    #[test]
    fn collect_cli_rejects_from_for_rsscript_producer() {
        let args = vec![
            "--producer".to_owned(),
            "rsscript".to_owned(),
            "--from".to_owned(),
            "review".to_owned(),
        ];

        let error = try_run_collect(&args).expect_err("--from should not be a RSScript input");

        match error {
            CliError::Usage(message) => assert_eq!(
                message,
                "`--from` is only supported by Terraform/OpenTofu collection; use RSScript JSON input flags with `--producer rsscript`."
            ),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn usage_documents_reconcile_target_for_both_modes() {
        assert!(USAGE.contains(
            "reir reconcile --required required.json --granted granted.json [--target name] [--json]"
        ));
        assert!(USAGE.contains(
            "reir reconcile [--bundle bundle.json] [--target name] [--out reconciled.json] [--json]"
        ));
    }

    #[test]
    fn usage_documents_generic_slice_kind_filter() {
        assert!(USAGE.contains("reir slice --bundle bundle.json [--kind <slice-kind>] [--json]"));
        assert!(matches!(
            parse_slice_kind("package_risk"),
            Ok(SliceKind::PackageRiskSlice)
        ));
        assert!(matches!(
            parse_slice_kind("native_unsafe_slice"),
            Ok(SliceKind::NativeUnsafeSlice)
        ));
    }

    #[test]
    fn merge_bundle_values_rejects_mismatched_ontology() {
        let first = Bundle::new();
        let mut second = Bundle::new();
        second.ontology = "reir.other_ontology.v0".to_owned();

        let error =
            merge_bundle_values(vec![first, second]).expect_err("ontology mismatch should fail");

        assert!(
            matches!(error, CliError::Runtime(message) if message.contains("cannot merge bundle with ontology"))
        );
    }

    #[test]
    fn reconcile_bundle_records_reconciliations_and_rebuilds_slices() {
        let required = capability_fact(
            "fact.required.report_upload",
            FactRole::Required,
            subject("report-uploader"),
            "s3:PutObject",
        );
        let unrelated_grant = capability_fact(
            "fact.granted.report_read",
            FactRole::Granted,
            subject("report-reader"),
            "s3:GetObject",
        );
        let mut bundle = Bundle {
            facts: vec![required.clone(), unrelated_grant],
            ..Bundle::new()
        };

        reconcile_bundle(&mut bundle, Some("prod"));

        assert!(bundle.reconciliations.iter().any(|reconciliation| {
            reconciliation.kind == ReconciliationKind::MissingCapability
                && reconciliation.required_fact.as_deref() == Some(required.id.as_str())
                && reconciliation.target.as_deref() == Some("prod")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::MissingCapabilitySlice && slice.facts.contains(&required.id)
        }));
    }

    #[test]
    fn exit_for_diff_fails_only_when_requested() {
        let unchanged = Diff {
            schema: "reir.diff.v0.1".to_owned(),
            id: "diff.empty".to_owned(),
            items: Vec::new(),
        };
        let changed = Diff {
            schema: "reir.diff.v0.1".to_owned(),
            id: "diff.changed".to_owned(),
            items: vec![DiffItem {
                kind: DiffItemKind::FactAdded,
                id: "fact.added".to_owned(),
                subject: None,
                description: None,
                evidence: Vec::new(),
            }],
        };

        assert_eq!(exit_for_diff(&unchanged, true), ExitCode::SUCCESS);
        assert_eq!(exit_for_diff(&changed, false), ExitCode::SUCCESS);
        assert_eq!(exit_for_diff(&changed, true), ExitCode::from(1));
    }

    #[test]
    fn cli_text_reads_enforce_the_byte_limit() {
        let temp_dir = unique_temp_dir("bounded-read");
        let input = temp_dir.join("input.json");
        std::fs::write(&input, b"12345").expect("fixture should be written");

        let error =
            read_bounded_text_with_limit(&input, 4).expect_err("oversized input should fail");

        let _ = std::fs::remove_dir_all(&temp_dir);
        assert!(
            matches!(error, CliError::Runtime(message) if message.contains("exceeding the 4 byte limit"))
        );
    }

    #[test]
    fn aggregate_input_budget_applies_across_multiple_files() {
        let temp_dir = unique_temp_dir("aggregate-input-budget");
        let first = temp_dir.join("first.json");
        let second = temp_dir.join("second.json");
        std::fs::write(&first, b"123").expect("first fixture");
        std::fs::write(&second, b"456").expect("second fixture");
        let mut aggregate_bytes = 0;

        read_bounded_text_accounted(first.to_str().expect("UTF-8 path"), &mut aggregate_bytes, 5)
            .expect("first input fits");
        let error = read_bounded_text_accounted(
            second.to_str().expect("UTF-8 path"),
            &mut aggregate_bytes,
            5,
        )
        .expect_err("combined inputs exceed the limit");

        let _ = std::fs::remove_dir_all(&temp_dir);
        assert_eq!(aggregate_bytes, 3);
        assert!(
            matches!(error, CliError::Runtime(message) if message.contains("aggregate input exceeds the 5 byte limit"))
        );
    }

    #[test]
    fn bounded_json_rejects_stdout_payloads_over_the_limit() {
        let error = bounded_json(&"12345", 4).expect_err("JSON exceeds the output limit");
        assert!(
            matches!(error, CliError::Runtime(message) if message.contains("failed to serialize JSON"))
        );
        assert_eq!(
            bounded_json(&"ok", 5).expect("quoted string plus newline fits"),
            b"\"ok\"\n"
        );
    }

    #[test]
    fn merge_rejects_too_many_input_files_before_reading() {
        let paths = vec!["does-not-exist.json".to_owned(); MAX_MERGE_INPUT_FILES + 1];
        let error = merge_bundles(&paths).expect_err("input count must be bounded");
        assert!(
            matches!(error, CliError::Runtime(message) if message.contains("at most 1024 input files"))
        );
    }

    #[test]
    fn atomic_output_replaces_regular_file() {
        let temp_dir = unique_temp_dir("atomic-output");
        let output = temp_dir.join("bundle.json");
        std::fs::write(&output, b"old").expect("fixture should be written");

        atomic_write_no_follow(&output, b"new").expect("regular output should be replaced");

        assert_eq!(std::fs::read(&output).expect("output should exist"), b"new");
        assert!(
            std::fs::read_dir(&temp_dir)
                .expect("directory should be readable")
                .all(|entry| !entry
                    .expect("entry should be readable")
                    .file_name()
                    .to_string_lossy()
                    .contains("reir-tmp"))
        );
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[test]
    fn cli_reads_and_outputs_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let temp_dir = unique_temp_dir("no-follow");
        let target = temp_dir.join("target.json");
        let link = temp_dir.join("link.json");
        std::fs::write(&target, b"unchanged").expect("fixture should be written");
        symlink(&target, &link).expect("symlink should be created");

        let read_error =
            read_bounded_text_with_limit(&link, 64).expect_err("symlink input should be rejected");
        let write_error = atomic_write_no_follow(&link, b"replacement")
            .expect_err("symlink output should be rejected");

        assert!(
            matches!(read_error, CliError::Runtime(message) if message.contains("symlink input"))
        );
        assert!(
            matches!(write_error, CliError::Runtime(message) if message.contains("symlink output"))
        );
        assert_eq!(
            std::fs::read(&target).expect("target should remain readable"),
            b"unchanged"
        );
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
