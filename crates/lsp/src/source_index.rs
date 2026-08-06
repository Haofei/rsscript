//! Revision-scoped immutable source indexes.

use std::sync::{Arc, Mutex};

use rsscript_language_service::SymbolIndex;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourceIndexIdentity {
    pub(crate) document_revision: u64,
    pub(crate) semantic_generation: u64,
}

struct CachedSourceIndex {
    identity: SourceIndexIdentity,
    index: Arc<SymbolIndex>,
}

#[derive(Default)]
pub(crate) struct SourceIndexCache {
    cached: Mutex<Option<CachedSourceIndex>>,
    #[cfg(test)]
    builds: std::sync::atomic::AtomicUsize,
}

impl SourceIndexCache {
    pub(crate) fn get(
        &self,
        identity: SourceIndexIdentity,
        path: &str,
        source: &str,
    ) -> Arc<SymbolIndex> {
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cached.as_ref()
            && cached.identity == identity
        {
            return Arc::clone(&cached.index);
        }

        let index = Arc::new(rsscript_language_service::symbol_index(path, source));
        #[cfg(test)]
        self.builds
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *cached = Some(CachedSourceIndex {
            identity,
            index: Arc::clone(&index),
        });
        index
    }

    #[cfg(test)]
    pub(crate) fn build_count(&self) -> usize {
        self.builds.load(std::sync::atomic::Ordering::Relaxed)
    }
}
