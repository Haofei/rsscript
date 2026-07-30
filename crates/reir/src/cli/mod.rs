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

mod bundle_ops;
mod commands;
mod rendering;
mod safe_io;

#[cfg(test)]
mod tests;

use std::process::ExitCode;

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

pub(super) fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let args = args.into_iter().collect::<Vec<_>>();
    let Some(command) = args.get(1).map(String::as_str) else {
        rendering::print_usage();
        return ExitCode::from(2);
    };

    match command {
        "collect" => commands::run_collect(&args[2..]),
        "reconcile" => commands::run_reconcile(&args[2..]),
        "report-pr" => commands::run_report_pr(&args[2..]),
        "diff" => commands::run_diff(&args[2..]),
        "slice" => commands::run_slice(&args[2..]),
        "merge" => commands::run_merge(&args[2..]),
        "show" => commands::run_show(&args[2..]),
        "--help" | "-h" | "help" => {
            rendering::print_usage();
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("unknown command: {command}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
