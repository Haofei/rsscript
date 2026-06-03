use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rsscript::{
    Definition, PackageReviewFileKind, Reference, RssDocumentSymbol, Severity, Span,
    SymbolCompleteness, SymbolKind, SymbolLookup, analyze_source_with_core,
    analyze_source_with_interfaces, analyze_sources_with_interfaces, document_symbols,
    format_diagnostics_json_with_source, lint_source, package_sources_with_dependency_interfaces,
    prefix_status, symbol_index, valid_continuations,
};
use serde_json::{Value, json};

use super::{is_package_directory, print_usage, required_flag_value};

#[derive(Clone)]
struct IdeDocument {
    path: String,
    relative_path: String,
    contents: String,
    kind: Option<PackageReviewFileKind>,
}

#[derive(Debug)]
struct IdeOptions<'a> {
    json: bool,
    action: Option<&'a str>,
    path: Option<&'a str>,
    line: Option<usize>,
    column: Option<usize>,
    query: Option<&'a str>,
    max: usize,
    include_declaration: bool,
}

pub(crate) fn run_ide(args: &[String]) -> ExitCode {
    let options = match parse_ide_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    if !options.json {
        eprintln!("`rss ide` is a structured tool command; pass `--json`.");
        return ExitCode::from(2);
    }
    let Some(action) = options.action else {
        print_usage();
        return ExitCode::from(2);
    };
    let Some(path) = options.path else {
        print_usage();
        return ExitCode::from(2);
    };

    let documents = match load_ide_documents(path) {
        Ok(documents) => documents,
        Err(error) => {
            println!("{}", json!({ "ok": false, "error": error }));
            return ExitCode::from(1);
        }
    };
    let target_path = target_document_path(path, &documents);
    let result = match action {
        "diagnostics" => ide_diagnostics(&documents),
        "symbols" => ide_symbols(&documents, options.query.unwrap_or(""), options.max),
        "outline" => ide_outline(&documents, &target_path),
        "hover" => position_action(&options)
            .and_then(|(line, column)| ide_hover(&documents, &target_path, line, column)),
        "definition" => position_action(&options)
            .and_then(|(line, column)| ide_definition(&documents, &target_path, line, column)),
        "references" => position_action(&options).and_then(|(line, column)| {
            ide_references(
                &documents,
                &target_path,
                line,
                column,
                options.include_declaration,
            )
        }),
        "generate" => ide_generate(&documents, &target_path, options.max),
        _ => Err(format!("unknown rss ide action `{action}`.")),
    };

    match result {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", json!({ "ok": false, "error": error }));
            ExitCode::from(1)
        }
    }
}

fn parse_ide_args(args: &[String]) -> Result<IdeOptions<'_>, String> {
    let mut options = IdeOptions {
        json: false,
        action: None,
        path: None,
        line: None,
        column: None,
        query: None,
        max: 50,
        include_declaration: false,
    };
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--json" => options.json = true,
            "--line" => {
                index += 1;
                options.line = Some(parse_usize_flag(args, index, "--line")?);
            }
            "--column" => {
                index += 1;
                options.column = Some(parse_usize_flag(args, index, "--column")?);
            }
            "--query" => {
                index += 1;
                options.query = Some(required_flag_value(args, index, "--query")?);
            }
            "--max" => {
                index += 1;
                options.max = parse_usize_flag(args, index, "--max")?.clamp(1, 500);
            }
            "--include-declaration" => options.include_declaration = true,
            _ if arg.starts_with("--") => return Err(format!("unknown argument `{arg}`.")),
            _ if options.action.is_none() => options.action = Some(arg),
            _ if options.path.is_none() => options.path = Some(arg),
            _ => return Err(format!("unexpected extra argument `{arg}`.")),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_usize_flag(args: &[String], index: usize, flag: &str) -> Result<usize, String> {
    let value = required_flag_value(args, index, flag)?;
    value
        .parse::<usize>()
        .map_err(|_| format!("`{flag}` expects a positive integer."))
}

fn position_action(options: &IdeOptions<'_>) -> Result<(usize, usize), String> {
    match (options.line, options.column) {
        (Some(line), Some(column)) => Ok((line, column)),
        _ => Err("this rss ide action requires `--line <n> --column <n>`.".to_string()),
    }
}

fn load_ide_documents(path: &str) -> Result<Vec<IdeDocument>, String> {
    if is_package_directory(path) {
        return load_package_documents(Path::new(path));
    }

    let file_path = Path::new(path);
    if let Some(package_dir) = find_package_root(file_path) {
        return load_package_documents(&package_dir);
    }

    let contents =
        fs::read_to_string(path).map_err(|error| format!("failed to read {path}: {error}"))?;
    Ok(vec![IdeDocument {
        path: path.to_string(),
        relative_path: Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path)
            .to_string(),
        contents,
        kind: infer_file_kind(path),
    }])
}

fn load_package_documents(package_dir: &Path) -> Result<Vec<IdeDocument>, String> {
    package_sources_with_dependency_interfaces(package_dir).map(|sources| {
        sources
            .into_iter()
            .map(|source| IdeDocument {
                path: source.path,
                relative_path: source.relative_path,
                contents: source.contents,
                kind: Some(source.kind),
            })
            .collect()
    })
}

fn find_package_root(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() { path } else { path.parent()? };
    loop {
        if current.join("rsspkg.toml").is_file() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn infer_file_kind(path: &str) -> Option<PackageReviewFileKind> {
    if path.ends_with(".rssi") {
        Some(PackageReviewFileKind::Interface)
    } else if path.ends_with(".rss") {
        Some(PackageReviewFileKind::Source)
    } else {
        None
    }
}

fn target_document_path(path: &str, documents: &[IdeDocument]) -> String {
    if documents.len() == 1 {
        return documents[0].path.clone();
    }
    let requested = Path::new(path);
    documents
        .iter()
        .find(|document| {
            let document_path = Path::new(&document.path);
            document_path == requested || document_path.ends_with(requested)
        })
        .map(|document| document.path.clone())
        .unwrap_or_else(|| documents[0].path.clone())
}

fn target_document<'a>(
    documents: &'a [IdeDocument],
    target_path: &str,
) -> Result<&'a IdeDocument, String> {
    documents
        .iter()
        .find(|document| document.path == target_path)
        .ok_or_else(|| format!("target document `{target_path}` is not loaded."))
}

fn ide_diagnostics(documents: &[IdeDocument]) -> Result<Value, String> {
    if documents.len() == 1 {
        let document = &documents[0];
        let mut diagnostics = analyze_source_with_core(&document.path, &document.contents);
        diagnostics.extend(lint_source(&document.path, &document.contents));
        let diagnostics_json =
            format_diagnostics_json_with_source(&document.contents, &diagnostics);
        let parsed: Value = serde_json::from_str(&diagnostics_json)
            .map_err(|error| format!("failed to encode diagnostics: {error}"))?;
        return Ok(json!({
            "ok": true,
            "action": "diagnostics",
            "path": document.path,
            "diagnostics": parsed,
            "error_count": diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Error).count(),
            "warning_count": diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Warning).count(),
        }));
    }

    let diagnostics = package_diagnostics(documents);
    Ok(json!({
        "ok": true,
        "action": "diagnostics",
        "diagnostics": diagnostics.iter().map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "severity": match diagnostic.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                },
                "summary": diagnostic.summary,
                "label": diagnostic.label,
                "span": span_json(&diagnostic.span),
                "causes": diagnostic.causes,
                "fixes": diagnostic.fixes.iter().map(|fix| {
                    json!({
                        "kind": fix.kind,
                        "title": fix.title,
                        "applicability": fix.applicability,
                    })
                }).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
        "error_count": diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Error).count(),
        "warning_count": diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Warning).count(),
    }))
}

fn package_diagnostics(documents: &[IdeDocument]) -> Vec<rsscript::Diagnostic> {
    let interfaces = documents
        .iter()
        .filter(|document| document.kind == Some(PackageReviewFileKind::Interface))
        .map(|document| (document.path.as_str(), document.contents.as_str()))
        .collect::<Vec<_>>();
    let sources = documents
        .iter()
        .filter(|document| document.kind == Some(PackageReviewFileKind::Source))
        .map(|document| (document.path.as_str(), document.contents.as_str()))
        .collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    diagnostics.extend(interfaces.iter().flat_map(|(path, contents)| {
        let visible_interfaces = interfaces
            .iter()
            .filter(|(interface_path, _)| interface_path != path)
            .map(|(interface_path, interface_contents)| (*interface_path, *interface_contents))
            .collect::<Vec<_>>();
        analyze_source_with_interfaces(path, contents, &visible_interfaces)
    }));
    diagnostics.extend(analyze_sources_with_interfaces(&sources, &interfaces));
    diagnostics.extend(
        documents
            .iter()
            .flat_map(|document| lint_source(&document.path, &document.contents)),
    );
    diagnostics
}

fn ide_symbols(documents: &[IdeDocument], query: &str, max: usize) -> Result<Value, String> {
    let query = query.to_lowercase();
    let mut symbols = Vec::new();
    for document in documents {
        let index = symbol_index(&document.path, &document.contents);
        for definition in index.definitions() {
            if !query.is_empty() && !definition.name.to_lowercase().contains(&query) {
                continue;
            }
            symbols.push(definition_json(document, definition));
            if symbols.len() >= max {
                return Ok(json!({ "ok": true, "action": "symbols", "symbols": symbols }));
            }
        }
    }
    Ok(json!({ "ok": true, "action": "symbols", "symbols": symbols }))
}

fn ide_outline(documents: &[IdeDocument], target_path: &str) -> Result<Value, String> {
    let document = target_document(documents, target_path)?;
    let outline = document_symbols(&document.path, &document.contents)
        .into_iter()
        .map(document_symbol_json)
        .collect::<Vec<_>>();
    Ok(json!({
        "ok": true,
        "action": "outline",
        "path": document.path,
        "symbols": outline,
    }))
}

fn ide_generate(documents: &[IdeDocument], target_path: &str, max: usize) -> Result<Value, String> {
    let document = target_document(documents, target_path)?;
    let ctx = rsscript::GenerateContext {
        file: &document.path,
        partial_source: &document.contents,
        symbol_completeness: SymbolCompleteness::Complete,
    };
    let status = prefix_status(&ctx);
    let continuations = valid_continuations(
        &ctx,
        rsscript::ContinuationOptions {
            max_returned_names: max,
            max_probe_candidates: max,
            include_snippets: false,
        },
    );
    Ok(json!({
        "ok": true,
        "action": "generate",
        "path": document.path,
        "prefix_status": status,
        "continuations": continuations,
    }))
}

fn ide_hover(
    documents: &[IdeDocument],
    target_path: &str,
    line: usize,
    column: usize,
) -> Result<Value, String> {
    let document = target_document(documents, target_path)?;
    let index = symbol_index(&document.path, &document.contents);
    let Some(symbol) = index.symbol_at(line, column) else {
        return Ok(json!({ "ok": true, "action": "hover", "symbol": null }));
    };
    let detail = if symbol.detail.is_some() {
        symbol.detail.clone()
    } else {
        index.lookup_at(line, column).and_then(|lookup| {
            workspace_definition(documents, &lookup).and_then(|definition| definition.detail)
        })
    };
    Ok(json!({
        "ok": true,
        "action": "hover",
        "symbol": {
            "name": symbol.name,
            "kind": symbol_kind_label(symbol.kind),
            "span": span_json(&symbol.span),
            "detail": detail,
        },
    }))
}

fn ide_definition(
    documents: &[IdeDocument],
    target_path: &str,
    line: usize,
    column: usize,
) -> Result<Value, String> {
    let document = target_document(documents, target_path)?;
    let index = symbol_index(&document.path, &document.contents);
    if let Some(span) = index.definition_at(line, column) {
        return Ok(json!({
            "ok": true,
            "action": "definition",
            "definition": { "path": document.path, "span": span_json(span) },
        }));
    }
    let Some(lookup) = index.lookup_at(line, column) else {
        return Ok(json!({ "ok": true, "action": "definition", "definition": null }));
    };
    let definition = workspace_definition_with_document(documents, &lookup);
    Ok(json!({
        "ok": true,
        "action": "definition",
        "definition": definition.map(|(doc, def)| json!({
            "path": doc.path,
            "relative_path": doc.relative_path,
            "name": def.name,
            "kind": symbol_kind_label(def.kind),
            "span": span_json(&def.span),
            "detail": def.detail,
        })),
    }))
}

fn ide_references(
    documents: &[IdeDocument],
    target_path: &str,
    line: usize,
    column: usize,
    include_declaration: bool,
) -> Result<Value, String> {
    let document = target_document(documents, target_path)?;
    let index = symbol_index(&document.path, &document.contents);
    let Some(lookup) = index.lookup_at(line, column) else {
        return Ok(json!({ "ok": true, "action": "references", "references": [] }));
    };
    if lookup.local_definition.is_some() {
        let references = index
            .references_at(line, column, include_declaration)
            .into_iter()
            .map(|span| {
                json!({
                    "path": document.path,
                    "relative_path": document.relative_path,
                    "span": span_json(&span),
                })
            })
            .collect::<Vec<_>>();
        return Ok(json!({ "ok": true, "action": "references", "references": references }));
    }

    let mut references = Vec::new();
    for doc in documents {
        let doc_index = symbol_index(&doc.path, &doc.contents);
        if include_declaration {
            for definition in doc_index
                .definitions()
                .iter()
                .filter(|definition| definition_matches_lookup(definition, &lookup))
            {
                references.push(json!({
                    "path": doc.path,
                    "relative_path": doc.relative_path,
                    "span": span_json(&definition.span),
                }));
            }
        }
        for reference in doc_index
            .references()
            .iter()
            .filter(|reference| unresolved_reference_matches_lookup(reference, &lookup))
        {
            references.push(reference_json(doc, reference));
        }
    }
    Ok(json!({ "ok": true, "action": "references", "references": references }))
}

fn workspace_definition(documents: &[IdeDocument], lookup: &SymbolLookup) -> Option<Definition> {
    workspace_definition_with_document(documents, lookup).map(|(_, definition)| definition)
}

fn workspace_definition_with_document<'a>(
    documents: &'a [IdeDocument],
    lookup: &SymbolLookup,
) -> Option<(&'a IdeDocument, Definition)> {
    documents.iter().find_map(|document| {
        let index = symbol_index(&document.path, &document.contents);
        index
            .definitions()
            .iter()
            .find(|definition| definition_matches_lookup(definition, lookup))
            .cloned()
            .map(|definition| (document, definition))
    })
}

fn definition_matches_lookup(definition: &Definition, lookup: &SymbolLookup) -> bool {
    definition.name == lookup.name
        && if lookup.is_type {
            definition.kind == SymbolKind::Type
        } else {
            matches!(
                definition.kind,
                SymbolKind::Function | SymbolKind::Const | SymbolKind::Variant
            )
        }
}

fn unresolved_reference_matches_lookup(reference: &Reference, lookup: &SymbolLookup) -> bool {
    reference.definition.is_none()
        && reference.name == lookup.name
        && reference.is_type == lookup.is_type
}

fn definition_json(document: &IdeDocument, definition: &Definition) -> Value {
    json!({
        "path": document.path,
        "relative_path": document.relative_path,
        "name": definition.name,
        "kind": symbol_kind_label(definition.kind),
        "span": span_json(&definition.span),
        "detail": definition.detail,
    })
}

fn reference_json(document: &IdeDocument, reference: &Reference) -> Value {
    json!({
        "path": document.path,
        "relative_path": document.relative_path,
        "name": reference.name,
        "is_type": reference.is_type,
        "span": span_json(&reference.span),
    })
}

fn document_symbol_json(symbol: RssDocumentSymbol) -> Value {
    json!({
        "name": symbol.name,
        "kind": symbol_kind_label(symbol.kind),
        "detail": symbol.detail,
        "span": span_json(&symbol.span),
        "selection_span": span_json(&symbol.selection_span),
        "children": symbol.children.into_iter().map(document_symbol_json).collect::<Vec<_>>(),
    })
}

fn span_json(span: &Span) -> Value {
    json!({
        "file": span.file,
        "line": span.line,
        "column": span.column,
        "length": span.length,
    })
}

fn symbol_kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Type => "type",
        SymbolKind::Const => "const",
        SymbolKind::Param => "parameter",
        SymbolKind::Local => "local",
        SymbolKind::Field => "field",
        SymbolKind::Variant => "variant",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_target_loads_package_for_cross_file_definition() {
        let package_dir = unique_temp_dir("rss-ide-cross-file");
        fs::create_dir_all(package_dir.join("src")).expect("create src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .expect("write manifest");
        fs::write(
            package_dir.join("src/helper.rss"),
            "fn helper(value: read Int) -> Int {\n    return value\n}\n",
        )
        .expect("write helper");
        let caller_path = package_dir.join("src/main.rss");
        fs::write(
            &caller_path,
            "fn run() -> Int {\n    return helper(value: read 1)\n}\n",
        )
        .expect("write caller");

        let documents =
            load_ide_documents(caller_path.to_str().expect("utf-8 path")).expect("load package");
        let target = target_document_path(caller_path.to_str().expect("utf-8 path"), &documents);
        let result = ide_definition(&documents, &target, 2, 12).expect("definition result");

        assert_eq!(result["ok"], true);
        assert!(
            result["definition"]["path"]
                .as_str()
                .expect("definition path")
                .ends_with("src/helper.rss")
        );

        fs::remove_dir_all(package_dir).expect("cleanup");
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }
}
