//! Editor-oriented prefix generation commands.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use rsscript_semantics::{
    Completeness, ContinuationOptions, Continuations, GenerationCoreInterfacePolicy,
    GenerationSession, ParserTerminal, PrefixStatus,
};
use rsscript_syntax::{PrefixParseState, parse_source_prefix};
use serde_json::{Value, json};

use super::{print_usage, read_cli_source, read_interface_sources, required_flag_value};

const DEFAULT_MAX_NAMES: usize = 50;
/// A CLI response must remain bounded even when Core and interface namespaces
/// contain an unexpectedly large number of matching symbols.
const MAX_NAMES_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenerateCommand {
    PrefixStatus,
    Continuations,
}

#[derive(Debug)]
struct GenerateOptions<'a> {
    json: bool,
    use_core: bool,
    interfaces: Vec<&'a str>,
    max_names: usize,
    path: Option<&'a str>,
}

pub(crate) fn run_generate(args: &[String]) -> ExitCode {
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return ExitCode::from(2);
    };
    let command = match command {
        "prefix-status" => GenerateCommand::PrefixStatus,
        "continuations" => GenerateCommand::Continuations,
        other => {
            eprintln!("unknown generate command `{other}`.");
            return ExitCode::from(2);
        }
    };
    let options = match parse_generate_args(command, &args[1..]) {
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
    let source = match read_cli_source(Path::new(path)) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("{error}");
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
    let unique_paths = interfaces
        .iter()
        .map(|interface| interface.path.as_str())
        .collect::<BTreeSet<_>>();
    if unique_paths.len() != interfaces.len() {
        eprintln!("duplicate interface paths are not valid for `rss generate`.");
        return ExitCode::from(2);
    }

    match command {
        GenerateCommand::PrefixStatus => {
            let prefix = parse_source_prefix(path, &source);
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string(&prefix_status_json(path, &source, &prefix))
                        .expect("generation prefix status serializes")
                );
            } else {
                print_prefix_status_text(path, source.len(), &prefix);
            }
        }
        GenerateCommand::Continuations => {
            let mut session = GenerationSession::with_source(path, source.as_str());
            if !options.use_core {
                session.set_core_interface_policy(GenerationCoreInterfacePolicy::WithoutCore);
            }
            for interface in &interfaces {
                session.set_interface(interface.path.as_str(), interface.contents.as_str());
            }
            let snapshot = session.query_snapshot();
            let continuations = session.query(ContinuationOptions {
                max_names: options.max_names,
            });
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string(&continuations_json(
                        path,
                        source.len(),
                        options.use_core,
                        &snapshot,
                        &continuations,
                    ))
                    .expect("generation continuations serialize")
                );
            } else {
                print_continuations_text(path, source.len(), &continuations);
            }
        }
    }
    ExitCode::SUCCESS
}

fn parse_generate_args(
    command: GenerateCommand,
    args: &[String],
) -> Result<GenerateOptions<'_>, String> {
    let mut json = false;
    let mut use_core = true;
    let mut interfaces = Vec::new();
    let mut max_names = DEFAULT_MAX_NAMES;
    let mut path = None;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        match argument.as_str() {
            "--json" => json = true,
            "--no-core" => use_core = false,
            "--interface" => {
                index += 1;
                interfaces.push(required_flag_value(args, index, "--interface")?);
            }
            "--max-names" if command == GenerateCommand::Continuations => {
                index += 1;
                let value = required_flag_value(args, index, "--max-names")?;
                max_names = value.parse::<usize>().map_err(|_| {
                    format!("`--max-names` must be an integer from 0 to {MAX_NAMES_LIMIT}.")
                })?;
                if max_names > MAX_NAMES_LIMIT {
                    return Err(format!("`--max-names` must not exceed {MAX_NAMES_LIMIT}."));
                }
            }
            "--max-names" => {
                return Err("`--max-names` is only valid for `rss generate continuations`.".into());
            }
            value if value.starts_with("--") => return Err(format!("unknown argument `{value}`.")),
            value if path.is_none() => path = Some(value),
            value => return Err(format!("unexpected extra path `{value}`.")),
        }
        index += 1;
    }
    Ok(GenerateOptions {
        json,
        use_core,
        interfaces,
        max_names,
        path,
    })
}

fn prefix_status_json(
    path: &str,
    source: &str,
    prefix: &rsscript_syntax::PrefixParseResult,
) -> Value {
    json!({
        "schema": "rsscript.generate.prefix_status.v1",
        "path": path,
        "source_bytes": source.len(),
        "status": prefix_status_name(prefix.state),
        "replace": { "start": prefix.replace_range.start, "end": prefix.replace_range.end },
        "current_terminal_completeness": terminal_completeness_name(prefix.current_terminal_completeness),
        "terminal_completeness": terminal_completeness_name(prefix.expected_terminals_completeness),
        "terminals": prefix.expected_terminals.iter().map(terminal_json).collect::<Vec<_>>(),
        "syntax_complete": prefix.state == PrefixParseState::Complete,
    })
}

fn continuations_json(
    path: &str,
    source_bytes: usize,
    use_core: bool,
    snapshot: &rsscript_semantics::GenerationQuerySnapshot,
    continuations: &Continuations,
) -> Value {
    let mut value = serde_json::to_value(continuations).expect("continuations serialize");
    let response = value
        .as_object_mut()
        .expect("continuations serialize as object");
    response.insert(
        "schema".into(),
        Value::String("rsscript.generate.continuations.v1".into()),
    );
    response.insert("path".into(), Value::String(path.into()));
    response.insert("source_bytes".into(), json!(source_bytes));
    response.insert(
        "core_interfaces".into(),
        Value::String(
            if use_core {
                "with_core"
            } else {
                "without_core"
            }
            .into(),
        ),
    );
    response.insert(
        "interface_revision".into(),
        json!(snapshot.interfaces.revision),
    );
    value
}

fn terminal_json(terminal: &rsscript_syntax::ExpectedTerminal) -> Value {
    match terminal {
        rsscript_syntax::ExpectedTerminal::Fixed { text, completeness } => json!({
            "kind": "fixed",
            "text": text,
            "completeness": terminal_completeness_name(*completeness),
        }),
        rsscript_syntax::ExpectedTerminal::Identifier { role, completeness } => json!({
            "kind": "identifier",
            "role": format!("{role:?}").to_lowercase(),
            "completeness": terminal_completeness_name(*completeness),
        }),
        rsscript_syntax::ExpectedTerminal::Literal { kind, completeness } => json!({
            "kind": "literal",
            "literal": format!("{kind:?}").to_lowercase(),
            "completeness": terminal_completeness_name(*completeness),
        }),
    }
}

fn print_prefix_status_text(
    path: &str,
    source_bytes: usize,
    prefix: &rsscript_syntax::PrefixParseResult,
) {
    println!(
        "{path}: {} ({source_bytes} bytes)\nreplace: {}..{}\nterminal completeness: {}\nsyntax complete: {}",
        prefix_status_name(prefix.state),
        prefix.replace_range.start,
        prefix.replace_range.end,
        terminal_completeness_name(prefix.expected_terminals_completeness),
        prefix.state == PrefixParseState::Complete,
    );
    for terminal in &prefix.expected_terminals {
        println!("terminal: {}", terminal_display(terminal));
    }
}

fn print_continuations_text(path: &str, source_bytes: usize, continuations: &Continuations) {
    println!(
        "{path}: {} ({source_bytes} bytes)\nreplace: {}..{}\nterminal completeness: {}\nname completeness: {}\nmay stop: {}\nnames: {}{}",
        continuation_status_name(continuations.status),
        continuations.replace.start,
        continuations.replace.end,
        completeness_name(continuations.terminal_completeness),
        completeness_name(continuations.name_completeness),
        continuations.may_stop,
        continuations.total_discovered_names,
        if continuations.truncated {
            " (truncated)"
        } else {
            ""
        },
    );
    for terminal in &continuations.terminals {
        println!("terminal: {}", parser_terminal_display(terminal));
    }
    for name in &continuations.names {
        match &name.result_type {
            Some(ty) => println!("name: {}: {}", name.text, ty.display),
            None => println!("name: {}", name.text),
        }
    }
    if let Some(expected_type) = &continuations.expected_type {
        println!("expected type: {}", expected_type.display);
    }
}

fn prefix_status_name(state: PrefixParseState) -> &'static str {
    match state {
        PrefixParseState::Complete => "complete",
        PrefixParseState::Incomplete => "incomplete",
        PrefixParseState::Dead => "dead",
    }
}

fn continuation_status_name(status: PrefixStatus) -> &'static str {
    match status {
        PrefixStatus::Complete => "complete",
        PrefixStatus::Incomplete => "incomplete",
        PrefixStatus::Dead => "dead",
    }
}

fn terminal_completeness_name(completeness: rsscript_syntax::TerminalCompleteness) -> &'static str {
    match completeness {
        rsscript_syntax::TerminalCompleteness::Complete => "complete",
        rsscript_syntax::TerminalCompleteness::Partial => "partial",
    }
}

fn completeness_name(completeness: Completeness) -> &'static str {
    match completeness {
        Completeness::Complete => "complete",
        Completeness::Partial => "partial",
    }
}

fn terminal_display(terminal: &rsscript_syntax::ExpectedTerminal) -> String {
    match terminal {
        rsscript_syntax::ExpectedTerminal::Fixed { text, .. } => (*text).into(),
        rsscript_syntax::ExpectedTerminal::Identifier { role, .. } => {
            format!("identifier {role:?}")
        }
        rsscript_syntax::ExpectedTerminal::Literal { kind, .. } => format!("literal {kind:?}"),
    }
}

fn parser_terminal_display(terminal: &ParserTerminal) -> String {
    match terminal {
        ParserTerminal::Fixed { text, .. } => text.clone(),
        ParserTerminal::Identifier { role, .. } => format!("identifier {role:?}"),
        ParserTerminal::Literal { literal, .. } => format!("literal {literal:?}"),
    }
}

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn max_names_is_bounded_and_rejected_for_prefix_status() {
        let error = super::parse_generate_args(
            super::GenerateCommand::Continuations,
            &args(&["--max-names", "1001", "main.rss"]),
        )
        .expect_err("unbounded name request must fail");
        assert!(error.contains("must not exceed"));

        let error = super::parse_generate_args(
            super::GenerateCommand::PrefixStatus,
            &args(&["--max-names", "5", "main.rss"]),
        )
        .expect_err("prefix status does not produce names");
        assert!(error.contains("only valid"));
    }

    #[test]
    fn parse_generate_args_accepts_interface_and_no_core() {
        let values = args(&[
            "--json",
            "--no-core",
            "--interface",
            "host.rssi",
            "main.rss",
        ]);
        let parsed = super::parse_generate_args(super::GenerateCommand::Continuations, &values)
            .expect("arguments should parse");
        assert!(parsed.json);
        assert!(!parsed.use_core);
        assert_eq!(parsed.interfaces, ["host.rssi"]);
    }
}
