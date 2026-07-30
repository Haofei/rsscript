use super::CliError;
use super::bundle_ops::{
    RsscriptCollectInputs, collect_rsscript_bundle, exit_for_diff, exit_for_reconciliations,
    merge_bundles, reconcile_bundle,
};
use super::rendering::{
    parse_slice_kind, print_bundle_summary, print_bundle_text, print_diff_text,
    print_reconciliation_text, print_slice_text, print_usage, report_error,
};
use super::safe_io::{
    print_json, read_bounded_text, read_bundle, read_optional_text_accounted,
    write_bounded_text_file, write_json_file,
};
use reir::adapters::terraform::{terraform_dir_to_bundle, terraform_plan_json_to_bundle};
use reir::api::v1::{
    decision::{GatePolicy, GatePolicyFile, GateStatus, TargetGatePolicy, decide_validated_gate},
    model::FactKind,
    reconciliation::{
        compute_diff, reconcile_capabilities_for_gate, reconcile_capabilities_for_target,
        slice_by_kind,
    },
    rendering::{
        format_ci_gate_json, format_ci_gate_output_from_decision, format_pr_review_comment,
        format_sarif,
    },
};
use std::process::ExitCode;

pub(super) fn run_reconcile(args: &[String]) -> ExitCode {
    match try_run_reconcile(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

pub(super) fn run_collect(args: &[String]) -> ExitCode {
    match try_run_collect(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

pub(super) fn run_report_pr(args: &[String]) -> ExitCode {
    match try_run_report_pr(args) {
        Ok((code, comment)) => {
            print!("{comment}");
            code
        }
        Err(error) => report_error(error),
    }
}

pub(super) fn try_run_collect(args: &[String]) -> Result<ExitCode, CliError> {
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
    let mut package_metadata = None;
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
            "--package-metadata" => {
                package_metadata = Some(take_value(args, &mut index, "--package-metadata")?)
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
            || package_metadata.is_some()
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
        && package_metadata.is_none()
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
    let package_metadata_json =
        read_optional_text_accounted(package_metadata.as_deref(), &mut aggregate_input_bytes)?;
    let bundle = collect_rsscript_bundle(RsscriptCollectInputs {
        review_map_json: review_map_json.as_deref(),
        package_review_json: package_review_json.as_deref(),
        package_check_json: package_check_json.as_deref(),
        package_lock_json: package_lock_json.as_deref(),
        package_lock_path: package_lock.as_deref(),
        lock_update_json: lock_update_json.as_deref(),
        package_tree_json: package_tree_json.as_deref(),
        package_metadata_json: package_metadata_json.as_deref(),
        package_name: package_name.as_deref(),
    })?;

    if strict {
        let error_diagnostics = bundle
            .facts
            .iter()
            .filter(|fact| fact.kind == FactKind::Diagnostic && fact.unknown_reason.is_some())
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

pub(super) fn try_run_report_pr(args: &[String]) -> Result<(ExitCode, String), CliError> {
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
    let mut cli = TargetGatePolicy::default();
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
            Some(GatePolicyFile::parse(&text).map_err(CliError::usage)?)
        }
        None => None,
    };
    let mut policy = match &policy_config {
        Some(config) => config
            .gate_policy_for(target.as_deref())
            .map_err(CliError::usage)?,
        None => GatePolicy::production(),
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
    let reconciliations = reconcile_capabilities_for_gate(
        &required_bundle.facts,
        &granted_bundle.facts,
        target.as_deref(),
        principal.as_deref(),
    );

    let decision = decide_validated_gate(
        &required_bundle.facts,
        &granted_bundle.facts,
        &reconciliations,
        policy,
    );
    let ci_output = format_ci_gate_output_from_decision(
        &decision,
        &required_bundle.facts,
        &granted_bundle.facts,
        &reconciliations,
    );
    let ci_json_rendered = format_ci_gate_json(&ci_output);
    let sarif_rendered = format_sarif(&decision);
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
    let exit = if decision.status == GateStatus::Fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };
    Ok((exit, output))
}

pub(super) fn set_policy_override(
    slot: &mut Option<bool>,
    value: bool,
    name: &str,
) -> Result<(), CliError> {
    if slot.is_some_and(|existing| existing != value) {
        return Err(CliError::usage(format!(
            "conflicting command-line values for {name}"
        )));
    }
    *slot = Some(value);
    Ok(())
}

pub(super) fn try_run_reconcile(args: &[String]) -> Result<ExitCode, CliError> {
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

pub(super) fn run_diff(args: &[String]) -> ExitCode {
    match try_run_diff(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

pub(super) fn try_run_diff(args: &[String]) -> Result<ExitCode, CliError> {
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

pub(super) fn run_slice(args: &[String]) -> ExitCode {
    match try_run_slice(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

pub(super) fn try_run_slice(args: &[String]) -> Result<ExitCode, CliError> {
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

pub(super) fn run_merge(args: &[String]) -> ExitCode {
    match try_run_merge(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

pub(super) fn try_run_merge(args: &[String]) -> Result<ExitCode, CliError> {
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

pub(super) fn run_show(args: &[String]) -> ExitCode {
    match try_run_show(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

pub(super) fn try_run_show(args: &[String]) -> Result<ExitCode, CliError> {
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

pub(super) fn take_value(
    args: &[String],
    index: &mut usize,
    flag: &str,
) -> Result<String, CliError> {
    *index += 1;
    let Some(value) = args.get(*index) else {
        return Err(CliError::usage(format!("missing value for {flag}")));
    };
    if value.starts_with("--") {
        return Err(CliError::usage(format!("missing value for {flag}")));
    }
    Ok(value.clone())
}

pub(super) fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
}
