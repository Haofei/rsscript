#![forbid(unsafe_code)]

//! Editor-facing RSScript language service.
//!
//! This crate is the only compiler-facing dependency of the LSP. Its API is
//! deliberately document-oriented so editor clients do not couple themselves to
//! analyzer databases, runtime values, VM registers, or optional backends.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use rsscript::{
    Definition, Diagnostic, DiagnosticExplanation, Reference, RssDocumentSymbol, Severity, Span,
    SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup, analyze_source_with_core,
    analyze_source_with_interfaces, analyze_sources_with_interfaces, document_symbols,
    explain_diagnostic_code, format_source, lint_source, symbol_index,
};

const MAX_WORKSPACE_FILES: usize = 20_000;
const MAX_WORKSPACE_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageReviewFileKind {
    Interface,
    Source,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSourceFile {
    pub path: String,
    pub relative_path: String,
    pub contents: String,
    pub kind: PackageReviewFileKind,
}

/// Discover editor documents without loading compiler, runtime or provider
/// packages. Symlinks and generated/native directories are excluded; package
/// dependency interfaces are supplied by the LSP's package roots as they are
/// opened rather than by executing package tooling.
pub fn package_sources_with_dependency_interfaces(
    package_dir: &Path,
) -> Result<Vec<PackageSourceFile>, String> {
    let root = if package_dir.is_absolute() {
        package_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve current directory: {error}"))?
            .join(package_dir)
    };
    if !root.is_dir() {
        return Err(format!(
            "package root is not a directory: {}",
            root.display()
        ));
    }
    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    scan_source_tree(&root, &root, false, &mut total_bytes, &mut files)?;

    let mut visited = BTreeSet::new();
    let mut pending_dependencies = dependency_paths(&root)?;
    while let Some(dependency) = pending_dependencies.pop() {
        let dependency = dependency.canonicalize().map_err(|error| {
            format!(
                "cannot resolve dependency package {}: {error}",
                dependency.display()
            )
        })?;
        if !visited.insert(dependency.clone()) {
            continue;
        }
        scan_source_tree(&dependency, &dependency, true, &mut total_bytes, &mut files)?;
        pending_dependencies.extend(dependency_paths(&dependency)?);
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn scan_source_tree(
    root: &Path,
    display_root: &Path,
    interfaces_only: bool,
    total_bytes: &mut u64,
    files: &mut Vec<PackageSourceFile>,
) -> Result<(), String> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let file_type = entry
                .file_type()
                .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                let name = entry.file_name();
                if !matches!(
                    name.to_str(),
                    Some(".git" | ".claude" | "target" | "native")
                ) {
                    pending.push(path);
                }
                continue;
            }
            let kind = match path.extension().and_then(|extension| extension.to_str()) {
                Some("rssi") => PackageReviewFileKind::Interface,
                Some("rss") if interfaces_only => continue,
                Some("rss") if path.components().any(|part| part.as_os_str() == "tests") => {
                    PackageReviewFileKind::Test
                }
                Some("rss") => PackageReviewFileKind::Source,
                _ => continue,
            };
            if files.len() >= MAX_WORKSPACE_FILES {
                return Err(
                    "workspace source file count exceeds language-service limit".to_string()
                );
            }
            let metadata = entry
                .metadata()
                .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
            *total_bytes = total_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| "workspace source byte count overflow".to_string())?;
            if *total_bytes > MAX_WORKSPACE_SOURCE_BYTES {
                return Err("workspace source bytes exceed language-service limit".to_string());
            }
            let contents = fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let relative_path = path
                .strip_prefix(display_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push(PackageSourceFile {
                path: path.to_string_lossy().into_owned(),
                relative_path,
                contents,
                kind,
            });
        }
    }
    Ok(())
}

fn dependency_paths(package_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let manifest_path = package_dir.join("rsspkg.toml");
    let source = match fs::read_to_string(&manifest_path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", manifest_path.display())),
    };
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("cannot parse {}: {error}", manifest_path.display()))?;
    let mut paths = Vec::new();
    for section in ["dependencies", "dev-dependencies"] {
        let Some(dependencies) = manifest.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for dependency in dependencies.values() {
            if let Some(path) = dependency
                .as_table()
                .and_then(|entry| entry.get("path"))
                .and_then(toml::Value::as_str)
            {
                paths.push(package_dir.join(path));
            }
        }
    }
    paths.sort();
    Ok(paths)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Source,
    Interface,
}

#[derive(Debug, Clone)]
struct Document {
    revision: u64,
    kind: DocumentKind,
    text: Arc<str>,
}

#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    pub path: String,
    pub revision: u64,
    pub kind: DocumentKind,
    pub text: Arc<str>,
}

#[derive(Default)]
pub struct LanguageService {
    documents: BTreeMap<String, Document>,
}

impl LanguageService {
    pub fn set_file(
        &mut self,
        path: impl Into<String>,
        revision: u64,
        kind: DocumentKind,
        text: impl Into<Arc<str>>,
    ) {
        self.documents.insert(
            path.into(),
            Document {
                revision,
                kind,
                text: text.into(),
            },
        );
    }

    pub fn remove_file(&mut self, path: &str) -> bool {
        self.documents.remove(path).is_some()
    }

    pub fn snapshot(&self, path: &str) -> Option<DocumentSnapshot> {
        self.documents.get(path).map(|document| DocumentSnapshot {
            path: path.to_string(),
            revision: document.revision,
            kind: document.kind,
            text: Arc::clone(&document.text),
        })
    }

    pub fn diagnostics(&self, path: &str) -> Vec<Diagnostic> {
        let Some(document) = self.documents.get(path) else {
            return Vec::new();
        };
        let interfaces = self
            .documents
            .iter()
            .filter(|(_, candidate)| candidate.kind == DocumentKind::Interface)
            .map(|(path, candidate)| (path.as_str(), candidate.text.as_ref()))
            .collect::<Vec<_>>();
        let mut diagnostics = match document.kind {
            DocumentKind::Source if interfaces.is_empty() => {
                analyze_source_with_core(path, &document.text)
            }
            DocumentKind::Source => {
                analyze_source_with_interfaces(path, &document.text, &interfaces)
            }
            DocumentKind::Interface => {
                let visible = interfaces
                    .iter()
                    .copied()
                    .filter(|(candidate, _)| *candidate != path)
                    .collect::<Vec<_>>();
                analyze_source_with_interfaces(path, &document.text, &visible)
            }
        };
        diagnostics.extend(lint_source(path, &document.text));
        diagnostics
    }

    pub fn format(&self, path: &str) -> Option<String> {
        let document = self.documents.get(path)?;
        Some(format_source(path, &document.text))
    }

    pub fn symbols(&self, path: &str) -> Option<SymbolIndex> {
        let document = self.documents.get(path)?;
        Some(symbol_index(path, &document.text))
    }

    pub fn document_symbols(&self, path: &str) -> Vec<RssDocumentSymbol> {
        self.documents
            .get(path)
            .map_or_else(Vec::new, |document| document_symbols(path, &document.text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_replace_snapshots_and_removal_is_explicit() {
        let mut service = LanguageService::default();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main() -> Unit {}\n",
        );
        service.set_file(
            "main.rss",
            2,
            DocumentKind::Source,
            "fn main() -> Int { 1 }\n",
        );
        assert_eq!(service.snapshot("main.rss").unwrap().revision, 2);
        assert!(service.remove_file("main.rss"));
        assert!(service.snapshot("main.rss").is_none());
    }

    #[test]
    fn service_reuses_diagnostics_formatting_and_symbols() {
        let mut service = LanguageService::default();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main()->Int{return 1}\n",
        );
        let diagnostics = service.diagnostics("main.rss");
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "unexpected errors: {diagnostics:?}"
        );
        assert_eq!(
            service.format("main.rss").unwrap(),
            "fn main() -> Int {\n    return 1\n}\n"
        );
        assert_eq!(service.document_symbols("main.rss")[0].name, "main");
    }
}
