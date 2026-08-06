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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub use rsscript_compiler::language::{
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
    WorkspaceLoader::default().load(package_dir)
}

/// Operating-system adapter that constructs editor snapshots. The language
/// engine below never reads files or manifests itself.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceLoader {
    pub max_files: usize,
    pub max_source_bytes: u64,
}

impl Default for WorkspaceLoader {
    fn default() -> Self {
        Self {
            max_files: MAX_WORKSPACE_FILES,
            max_source_bytes: MAX_WORKSPACE_SOURCE_BYTES,
        }
    }
}

impl WorkspaceLoader {
    pub fn load(&self, package_dir: &Path) -> Result<Vec<PackageSourceFile>, String> {
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
        scan_source_tree(&root, &root, false, self, &mut total_bytes, &mut files)?;

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
            scan_source_tree(
                &dependency,
                &dependency,
                true,
                self,
                &mut total_bytes,
                &mut files,
            )?;
            pending_dependencies.extend(dependency_paths(&dependency)?);
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }
}

fn scan_source_tree(
    root: &Path,
    display_root: &Path,
    interfaces_only: bool,
    limits: &WorkspaceLoader,
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
            if files.len() >= limits.max_files {
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
            if *total_bytes > limits.max_source_bytes {
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

pub struct LanguageService {
    documents: BTreeMap<String, Document>,
    diagnostic_cache: BTreeMap<(String, u64, u64), Arc<[Diagnostic]>>,
    cache_hits: u64,
    cache_misses: u64,
}

impl Default for LanguageService {
    fn default() -> Self {
        Self {
            documents: BTreeMap::new(),
            diagnostic_cache: BTreeMap::new(),
            cache_hits: 0,
            cache_misses: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LanguageRequest<'a> {
    pub cancellation: Option<&'a AtomicBool>,
    pub deadline: Option<Instant>,
    pub max_diagnostics: usize,
}

impl Default for LanguageRequest<'static> {
    fn default() -> Self {
        Self {
            cancellation: None,
            deadline: None,
            max_diagnostics: 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LanguageServiceStats {
    pub cache_hits: u64,
    pub cache_misses: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageServiceError {
    Cancelled,
    DeadlineExceeded,
}

impl LanguageService {
    pub fn set_file(
        &mut self,
        path: impl Into<String>,
        revision: u64,
        kind: DocumentKind,
        text: impl Into<Arc<str>>,
    ) {
        let path = path.into();
        if self
            .documents
            .get(&path)
            .is_some_and(|document| revision <= document.revision)
        {
            return;
        }
        let invalidates_interfaces = kind == DocumentKind::Interface
            || self
                .documents
                .get(&path)
                .is_some_and(|document| document.kind == DocumentKind::Interface);
        self.documents.insert(
            path.clone(),
            Document {
                revision,
                kind,
                text: text.into(),
            },
        );
        if invalidates_interfaces {
            self.diagnostic_cache.clear();
        } else {
            self.diagnostic_cache
                .retain(|(cached_path, _, _), _| cached_path != &path);
        }
    }

    pub fn remove_file(&mut self, path: &str) -> bool {
        let removed = self.documents.remove(path);
        if removed
            .as_ref()
            .is_some_and(|document| document.kind == DocumentKind::Interface)
        {
            self.diagnostic_cache.clear();
        } else {
            self.diagnostic_cache
                .retain(|(cached_path, _, _), _| cached_path != path);
        }
        removed.is_some()
    }

    pub fn snapshot(&self, path: &str) -> Option<DocumentSnapshot> {
        self.documents.get(path).map(|document| DocumentSnapshot {
            path: path.to_string(),
            revision: document.revision,
            kind: document.kind,
            text: Arc::clone(&document.text),
        })
    }

    pub fn diagnostics(&mut self, path: &str) -> Vec<Diagnostic> {
        self.diagnostics_with(path, LanguageRequest::default())
            .unwrap_or_default()
    }

    pub fn diagnostics_with(
        &mut self,
        path: &str,
        request: LanguageRequest<'_>,
    ) -> Result<Vec<Diagnostic>, LanguageServiceError> {
        check_request(request)?;
        let Some(document) = self.documents.get(path) else {
            return Ok(Vec::new());
        };
        let interface_revision = interface_revision(&self.documents);
        let cache_key = (path.to_string(), document.revision, interface_revision);
        if let Some(cached) = self.diagnostic_cache.get(&cache_key) {
            self.cache_hits += 1;
            return Ok(cached
                .iter()
                .take(request.max_diagnostics)
                .cloned()
                .collect());
        }
        self.cache_misses += 1;
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
        check_request(request)?;
        diagnostics.truncate(request.max_diagnostics);
        self.diagnostic_cache
            .insert(cache_key, Arc::from(diagnostics.clone()));
        Ok(diagnostics)
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

    pub fn stats(&self) -> LanguageServiceStats {
        LanguageServiceStats {
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
        }
    }
}

fn check_request(request: LanguageRequest<'_>) -> Result<(), LanguageServiceError> {
    if request
        .cancellation
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
    {
        return Err(LanguageServiceError::Cancelled);
    }
    if request
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        return Err(LanguageServiceError::DeadlineExceeded);
    }
    Ok(())
}

fn interface_revision(documents: &BTreeMap<String, Document>) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for (path, document) in documents
        .iter()
        .filter(|(_, document)| document.kind == DocumentKind::Interface)
    {
        for byte in path.as_bytes() {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        hash = (hash ^ document.revision).wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main() -> Unit {}\n",
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

    #[test]
    fn diagnostics_cache_is_revisioned_and_interfaces_invalidate_dependents() {
        let mut service = LanguageService::default();
        service.set_file(
            "host.rssi",
            1,
            DocumentKind::Interface,
            "module host\npub fn value() -> Int\n",
        );
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "module app\nuse host.*\nfn main() -> Int { return value() }\n",
        );
        service.diagnostics("main.rss");
        service.diagnostics("main.rss");
        assert_eq!(service.stats().cache_misses, 1);
        assert_eq!(service.stats().cache_hits, 1);

        service.set_file(
            "host.rssi",
            2,
            DocumentKind::Interface,
            "module host\npub fn value() -> String\n",
        );
        service.diagnostics("main.rss");
        assert_eq!(service.stats().cache_misses, 2);
    }

    #[test]
    fn cancelled_and_expired_requests_do_not_enter_analysis() {
        let mut service = LanguageService::default();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main() -> Unit { return Unit }\n",
        );
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            service.diagnostics_with(
                "main.rss",
                LanguageRequest {
                    cancellation: Some(&cancelled),
                    ..LanguageRequest::default()
                },
            ),
            Err(LanguageServiceError::Cancelled)
        );
        assert_eq!(
            service.diagnostics_with(
                "main.rss",
                LanguageRequest {
                    deadline: Some(Instant::now() - Duration::from_millis(1)),
                    ..LanguageRequest::default()
                },
            ),
            Err(LanguageServiceError::DeadlineExceeded)
        );
        assert_eq!(service.stats(), LanguageServiceStats::default());
    }
}
