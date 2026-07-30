//! Package discovery, workspace snapshots, and package input caching.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rsscript::*;
use tower_lsp::lsp_types::*;

use crate::documents::*;
use crate::scheduler::*;

#[derive(Clone)]
pub(crate) struct WorkspaceDocument {
    pub(crate) uri: Url,
    pub(crate) text: Arc<str>,
    pub(crate) kind: Option<PackageReviewFileKind>,
}

#[derive(Default)]
pub(crate) struct PackageInputCache {
    pub(crate) documents: Mutex<HashMap<PathBuf, Arc<HashMap<Url, WorkspaceDocument>>>>,
}

impl PackageInputCache {
    pub(crate) fn lock_documents(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Arc<HashMap<Url, WorkspaceDocument>>>> {
        self.documents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn documents_for_root(
        &self,
        package_root: &Path,
    ) -> Arc<HashMap<Url, WorkspaceDocument>> {
        if let Some(documents) = self.lock_documents().get(package_root).cloned() {
            return documents;
        }

        let documents = Arc::new(load_package_documents(package_root));
        let mut cache = self.lock_documents();
        Arc::clone(cache.entry(package_root.to_path_buf()).or_insert(documents))
    }

    pub(crate) fn invalidate(&self, package_root: &Path) {
        self.lock_documents().remove(package_root);
    }

    pub(crate) fn invalidate_path(&self, changed_path: &Path) -> Vec<PathBuf> {
        let mut invalidated = Vec::new();
        self.lock_documents().retain(|package_root, _| {
            let affected = changed_path.starts_with(package_root);
            if affected {
                invalidated.push(package_root.clone());
            }
            !affected
        });
        invalidated
    }
}

pub(crate) fn workspace_documents_for_uri(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
) -> Vec<WorkspaceDocument> {
    let mut documents = package_documents_for_uri(uri);
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

pub(crate) fn workspace_documents_from_base(
    base: &HashMap<Url, WorkspaceDocument>,
    open_documents: &HashMap<Url, Document>,
) -> Vec<WorkspaceDocument> {
    let mut documents = base.clone();
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

pub(crate) fn workspace_documents(
    open_documents: &HashMap<Url, Document>,
) -> Vec<WorkspaceDocument> {
    let mut documents = HashMap::new();
    for uri in open_documents.keys() {
        documents.extend(package_documents_for_uri(uri));
    }
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

pub(crate) fn overlay_open_documents(
    documents: &mut HashMap<Url, WorkspaceDocument>,
    open_documents: &HashMap<Url, Document>,
) {
    for (uri, document) in open_documents {
        documents.insert(
            uri.clone(),
            WorkspaceDocument {
                uri: uri.clone(),
                text: Arc::clone(&document.text),
                kind: infer_document_kind(uri),
            },
        );
    }
}

pub(crate) fn package_documents_for_uri(uri: &Url) -> HashMap<Url, WorkspaceDocument> {
    let Some(package_dir) = package_root_for_uri(uri) else {
        return HashMap::new();
    };
    load_package_documents(&package_dir)
}

pub(crate) fn load_package_documents(package_dir: &Path) -> HashMap<Url, WorkspaceDocument> {
    let Ok(sources) = package_sources_with_dependency_interfaces(package_dir) else {
        return HashMap::new();
    };
    sources
        .into_iter()
        .filter_map(|source| {
            Url::from_file_path(PathBuf::from(&source.path))
                .ok()
                .map(|uri| {
                    (
                        uri.clone(),
                        WorkspaceDocument {
                            uri,
                            text: Arc::from(source.contents),
                            kind: Some(source.kind),
                        },
                    )
                })
        })
        .collect()
}

pub(crate) fn infer_document_kind(uri: &Url) -> Option<PackageReviewFileKind> {
    let path = uri.path();
    if path.ends_with(".rssi") {
        Some(PackageReviewFileKind::Interface)
    } else if path.ends_with(".rss") {
        Some(PackageReviewFileKind::Source)
    } else {
        None
    }
}

pub(crate) fn analysis_key_for_uri(uri: &Url) -> AnalysisKey {
    if let Some(package_root) = package_root_for_uri(uri) {
        return AnalysisKey::Package(package_root);
    }
    if uri.to_file_path().is_ok() {
        return AnalysisKey::Workspace;
    }
    AnalysisKey::Uri(uri.clone())
}

pub(crate) fn package_root_for_uri(uri: &Url) -> Option<PathBuf> {
    let path = uri.to_file_path().ok()?;
    package_root_for_path(&path)
}

pub(crate) fn package_root_for_path(path: &Path) -> Option<PathBuf> {
    find_package_root(path)
}

pub(crate) fn find_package_root(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() { path } else { path.parent()? };
    loop {
        if current.join("rsspkg.toml").is_file() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}
