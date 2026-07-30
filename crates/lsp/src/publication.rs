//! Coalesced, non-blocking diagnostics publication.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tower_lsp::Client;
use tower_lsp::lsp_types::*;

use crate::scheduler::MAX_PENDING_DIAGNOSTIC_PUBLICATIONS;
pub(crate) struct DiagnosticsPublication {
    pub(crate) uri: Url,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) version: Option<i32>,
}

#[derive(Clone)]
pub(crate) struct DiagnosticsPublisher {
    pub(crate) pending: Arc<Mutex<HashMap<Url, DiagnosticsPublication>>>,
    pub(crate) wake: mpsc::Sender<()>,
}

impl DiagnosticsPublisher {
    pub(crate) fn enqueue(&self, publication: DiagnosticsPublication) {
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !pending.contains_key(&publication.uri)
                && pending.len() >= MAX_PENDING_DIAGNOSTIC_PUBLICATIONS
                && let Some(uri) = pending.keys().next().cloned()
            {
                pending.remove(&uri);
            }
            pending.insert(publication.uri.clone(), publication);
        }
        // Capacity one is enough: a wake means "drain the coalesced map".
        // Full and closed channels require no blocking fallback.
        let _ = self.wake.try_send(());
    }

    pub(crate) fn take_pending(&self) -> Vec<DiagnosticsPublication> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending
            .drain()
            .map(|(_, publication)| publication)
            .collect()
    }
}

pub(crate) fn diagnostics_publication_queue() -> (DiagnosticsPublisher, mpsc::Receiver<()>) {
    let (wake, receiver) = mpsc::channel(1);
    (
        DiagnosticsPublisher {
            pending: Arc::new(Mutex::new(HashMap::new())),
            wake,
        },
        receiver,
    )
}

pub(crate) fn spawn_diagnostics_publisher(client: Client) -> DiagnosticsPublisher {
    let (publisher, mut receiver) = diagnostics_publication_queue();
    let worker = publisher.clone();
    tokio::spawn(async move {
        while receiver.recv().await.is_some() {
            loop {
                let publications = worker.take_pending();
                if publications.is_empty() {
                    break;
                }
                for publication in publications {
                    client
                        .publish_diagnostics(
                            publication.uri,
                            publication.diagnostics,
                            publication.version,
                        )
                        .await;
                }
            }
        }
    });
    publisher
}

pub(crate) fn enqueue_diagnostics(
    publications: &DiagnosticsPublisher,
    uri: Url,
    diagnostics: Vec<Diagnostic>,
    version: Option<i32>,
) {
    publications.enqueue(DiagnosticsPublication {
        uri,
        diagnostics,
        version,
    });
}
