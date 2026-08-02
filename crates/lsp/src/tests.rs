use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rsscript::{Diagnostic as RsDiagnostic, PackageReviewFileKind, Span, symbol_index};
use tokio::sync::{Semaphore, oneshot};
use tower_lsp::lsp_types::*;

use crate::backend::snapshot_documents;
use crate::diagnostics::*;
use crate::documents::*;
use crate::features::*;
use crate::publication::*;
use crate::scheduler::*;
use crate::scope::*;
use crate::source_index::SourceIndexCache;
use crate::text::*;
use crate::workspace::*;

fn file_url(name: &str) -> Url {
    Url::parse(&format!("file:///workspace/{name}")).expect("valid file URL")
}

fn document(text: &str) -> Document {
    Document {
        text: Arc::from(text),
        diagnostics: Arc::new(Vec::new()),
        revision: 0,
        version: 0,
        sync_state: DocumentSyncState::Synchronized,
        source_index: Arc::new(SourceIndexCache::default()),
    }
}

fn workspace_document(uri: Url, text: &str) -> WorkspaceDocument {
    WorkspaceDocument {
        uri,
        text: Arc::from(text),
        kind: Some(PackageReviewFileKind::Source),
        revision: 0,
        semantic_generation: 0,
        source_index: Arc::new(SourceIndexCache::default()),
    }
}

#[test]
fn source_index_is_reused_within_a_document_revision_and_rebuilt_after_edit() {
    let uri = file_url("revision-cache.rss");
    let mut documents = DocumentStore::new();
    open_document(
        &mut documents,
        uri.clone(),
        "fn old() -> Unit {}\n".to_owned(),
        1,
    )
    .expect("document should open");

    let old_document = documents.get(&uri).expect("open document").clone();
    let first = old_document.symbol_index(uri.path());
    let second = old_document.symbol_index(uri.path());
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(old_document.source_index.build_count(), 1);
    assert_eq!(first.definitions()[0].name, "old");

    let replacement = TextDocumentContentChangeEvent {
        range: None,
        range_length: None,
        text: "fn new() -> Unit {}\n".to_owned(),
    };
    change_document(&mut documents, uri.clone(), 2, &[replacement])
        .expect_applied("full edit should apply");
    let new_document = documents.get(&uri).expect("changed document");
    let changed = new_document.symbol_index(uri.path());

    assert!(!Arc::ptr_eq(&first, &changed));
    assert_eq!(new_document.source_index.build_count(), 1);
    assert_eq!(changed.definitions()[0].name, "new");
    assert_eq!(
        old_document.symbol_index(uri.path()).definitions()[0].name,
        "old",
        "an in-flight immutable snapshot must retain its revision index"
    );
}

#[tokio::test]
async fn slow_diagnostic_client_coalesces_without_blocking_document_progress() {
    let (publisher, _blocked_receiver) = diagnostics_publication_queue();
    let first_uri = file_url("first.rss");
    let second_uri = file_url("second.rss");
    let documents = tokio::sync::Mutex::new(DocumentStore::new());

    // The first enqueue fills the capacity-one wake channel. Subsequent
    // publications must only update the bounded coalescing map.
    enqueue_diagnostics(&publisher, first_uri.clone(), Vec::new(), Some(1));
    enqueue_diagnostics(&publisher, first_uri.clone(), Vec::new(), Some(2));
    enqueue_diagnostics(&publisher, second_uri.clone(), Vec::new(), Some(3));

    tokio::time::timeout(Duration::from_millis(100), async {
        let mut documents = documents.lock().await;
        open_document(
            &mut documents,
            second_uri.clone(),
            "fn next() {}".to_owned(),
            3,
        );
    })
    .await
    .expect("a blocked diagnostics client must not block document state");

    let pending = publisher.take_pending();
    assert_eq!(pending.len(), 2);
    assert!(
        pending
            .iter()
            .any(|publication| { publication.uri == first_uri && publication.version == Some(2) })
    );
    assert!(
        pending
            .iter()
            .any(|publication| { publication.uri == second_uri && publication.version == Some(3) })
    );
}

#[test]
fn diagnostic_publication_backlog_has_a_hard_uri_limit() {
    let (publisher, _blocked_receiver) = diagnostics_publication_queue();
    for index in 0..(MAX_PENDING_DIAGNOSTIC_PUBLICATIONS + 32) {
        enqueue_diagnostics(
            &publisher,
            file_url(&format!("queued-{index}.rss")),
            Vec::new(),
            Some(index as i32),
        );
    }

    assert_eq!(
        publisher.take_pending().len(),
        MAX_PENDING_DIAGNOSTIC_PUBLICATIONS
    );
}

#[test]
fn rejects_reversed_and_invalid_utf16_incremental_ranges() {
    let mut text = "a😀b\n".to_owned();
    let original = text.clone();
    let reversed = TextDocumentContentChangeEvent {
        range: Some(Range::new(Position::new(0, 3), Position::new(0, 1))),
        range_length: None,
        text: "x".to_owned(),
    };
    assert!(!apply_change(&mut text, &reversed));
    assert_eq!(text, original);

    let split_surrogate = TextDocumentContentChangeEvent {
        range: Some(Range::new(Position::new(0, 2), Position::new(0, 2))),
        range_length: None,
        text: "x".to_owned(),
    };
    assert!(!apply_change(&mut text, &split_surrogate));
    assert_eq!(text, original);
}

#[test]
fn rejects_documents_and_changes_over_the_document_byte_cap() {
    let uri = file_url("oversized.rss");
    let mut documents = DocumentStore::new();
    assert!(
        open_document(
            &mut documents,
            uri.clone(),
            "x".repeat(MAX_DOCUMENT_BYTES + 1),
            1,
        )
        .is_none()
    );
    open_document(&mut documents, uri.clone(), "small".to_owned(), 1)
        .expect("small document should open");
    let replacement = TextDocumentContentChangeEvent {
        range: None,
        range_length: None,
        text: "x".repeat(MAX_DOCUMENT_BYTES + 1),
    };
    assert!(matches!(
        change_document(&mut documents, uri.clone(), 2, &[replacement]),
        ChangeOutcome::Desynchronized(ChangeFailure::OversizedDocument)
    ));
    assert_eq!(
        documents.get(&uri).expect("document remains").text.as_ref(),
        "small"
    );
    assert_eq!(
        documents.get(&uri).expect("document remains").sync_state,
        DocumentSyncState::Desynchronized
    );
}

#[tokio::test]
async fn invalid_change_suspends_semantics_until_full_sync() {
    let uri = file_url("desynchronized.rss");
    let mut documents = DocumentStore::new();
    open_document(
        &mut documents,
        uri.clone(),
        "fn current() -> Unit {}\n".to_owned(),
        1,
    )
    .expect("document should open");

    let reversed = TextDocumentContentChangeEvent {
        range: Some(Range::new(Position::new(0, 8), Position::new(0, 3))),
        range_length: None,
        text: "broken".to_owned(),
    };
    assert!(matches!(
        change_document(&mut documents, uri.clone(), 2, &[reversed]),
        ChangeOutcome::Desynchronized(ChangeFailure::InvalidRange)
    ));
    let document = documents.get(&uri).expect("document remains");
    assert_eq!(document.sync_state, DocumentSyncState::Desynchronized);
    assert_eq!(document.version, 2);
    let revision = document.revision;
    let generation = documents.generation(&analysis_key_for_uri(&uri));
    assert!(!commit_diagnostics_if_current(
        &mut documents,
        &uri,
        revision,
        2,
        generation,
        generation,
        Vec::new(),
    ));

    let shared = tokio::sync::Mutex::new(documents);
    assert!(
        !snapshot_documents(&shared).await.contains_key(&uri),
        "desynchronized text must not serve semantic requests"
    );

    let mut documents = shared.into_inner();
    let incremental = TextDocumentContentChangeEvent {
        range: Some(Range::new(Position::new(0, 0), Position::new(0, 0))),
        range_length: None,
        text: "x".to_owned(),
    };
    assert!(matches!(
        change_document(&mut documents, uri.clone(), 3, &[incremental]),
        ChangeOutcome::Desynchronized(ChangeFailure::FullSyncRequired)
    ));
    assert!(matches!(
        change_document(&mut documents, uri.clone(), 2, &[]),
        ChangeOutcome::IgnoredStale
    ));

    let full_sync = TextDocumentContentChangeEvent {
        range: None,
        range_length: None,
        text: "fn recovered() -> Unit {}\n".to_owned(),
    };
    let job = change_document(&mut documents, uri.clone(), 4, &[full_sync])
        .expect_applied("full sync should recover the document");
    assert!(job.open_documents.contains_key(&uri));
    let document = documents.get(&uri).expect("document remains");
    assert_eq!(document.sync_state, DocumentSyncState::Synchronized);
    assert_eq!(document.text.as_ref(), "fn recovered() -> Unit {}\n");
}

#[test]
fn invalid_utf16_change_marks_document_desynchronized() {
    let uri = file_url("invalid-utf16.rss");
    let mut documents = DocumentStore::new();
    open_document(&mut documents, uri.clone(), "a😀b\n".to_owned(), 1)
        .expect("document should open");
    let split_surrogate = TextDocumentContentChangeEvent {
        range: Some(Range::new(Position::new(0, 2), Position::new(0, 2))),
        range_length: None,
        text: "x".to_owned(),
    };
    assert!(matches!(
        change_document(&mut documents, uri.clone(), 2, &[split_surrogate]),
        ChangeOutcome::Desynchronized(ChangeFailure::InvalidRange)
    ));
    assert_eq!(
        documents.get(&uri).expect("document remains").sync_state,
        DocumentSyncState::Desynchronized
    );
}

#[test]
fn analysis_jobs_share_immutable_document_snapshots() {
    let uri = file_url("snapshot.rss");
    let mut documents = DocumentStore::new();
    let first = open_document(&mut documents, uri.clone(), "first".to_owned(), 1)
        .expect("document should open");
    let first_text = Arc::clone(
        &first
            .open_documents
            .get(&uri)
            .expect("snapshot contains document")
            .text,
    );

    let replacement = TextDocumentContentChangeEvent {
        range: None,
        range_length: None,
        text: "second".to_owned(),
    };
    let second = change_document(&mut documents, uri.clone(), 2, &[replacement])
        .expect_applied("new version should produce a job");

    assert_eq!(
        first.open_documents.get(&uri).unwrap().text.as_ref(),
        "first"
    );
    assert_eq!(
        second.open_documents.get(&uri).unwrap().text.as_ref(),
        "second"
    );
    assert!(Arc::ptr_eq(
        &first_text,
        &first.open_documents.get(&uri).unwrap().text
    ));
}

#[tokio::test]
async fn replacing_pending_analysis_aborts_superseded_task() {
    let uri = file_url("debounce.rss");
    let analysis_key = analysis_key_for_uri(&uri);
    let mut pending = HashMap::new();
    let first_cancellation = Arc::new(AnalysisCancellation::default());
    let first = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    replace_pending_analysis(
        &mut pending,
        analysis_key.clone(),
        PendingAnalysis {
            task: first.abort_handle(),
            cancellation: Arc::clone(&first_cancellation),
        },
    );

    let second_cancellation = Arc::new(AnalysisCancellation::default());
    let second = tokio::spawn(async {});
    replace_pending_analysis(
        &mut pending,
        analysis_key,
        PendingAnalysis {
            task: second.abort_handle(),
            cancellation: Arc::clone(&second_cancellation),
        },
    );

    assert!(first_cancellation.is_cancelled());
    assert!(!second_cancellation.is_cancelled());
    assert!(
        first
            .await
            .expect_err("superseded task should abort")
            .is_cancelled()
    );
    second.await.expect("latest task should complete");
}

#[tokio::test]
async fn package_edits_cancel_superseded_jobs_and_keep_latest_generation() {
    let package_dir = unique_temp_dir("rss-lsp-package-generation");
    fs::create_dir_all(package_dir.join("src")).expect("create package src");
    fs::write(
        package_dir.join("rsspkg.toml"),
        "[package]\nname = \"generation\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write package manifest");

    let mut documents = DocumentStore::new();
    let mut pending = HashMap::new();
    let mut cancellations = Vec::new();
    let mut tasks = Vec::new();
    let mut first_job_state = None;
    let mut latest_job_state = None;

    for index in 0..32 {
        let uri = Url::from_file_path(package_dir.join("src").join(format!("{index}.rss")))
            .expect("source URL");
        let job = open_document(
            &mut documents,
            uri.clone(),
            format!("fn value_{index}() -> Int {{ return {index} }}\n"),
            1,
        )
        .expect("new package document should schedule analysis");
        if index == 0 {
            first_job_state = Some((
                uri.clone(),
                job.revision,
                job.version,
                job.generation,
                job.analysis_key.clone(),
            ));
        }
        latest_job_state = Some((
            uri,
            job.revision,
            job.version,
            job.generation,
            job.analysis_key.clone(),
        ));
        cancellations.push(Arc::clone(&job.cancellation));
        let task = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        replace_pending_analysis(
            &mut pending,
            job.analysis_key,
            PendingAnalysis {
                task: task.abort_handle(),
                cancellation: job.cancellation,
            },
        );
        tasks.push(task);
    }

    assert_eq!(pending.len(), 1);
    assert!(cancellations[..31].iter().all(|item| item.is_cancelled()));
    assert!(!cancellations[31].is_cancelled());

    let (first_uri, first_revision, first_version, first_generation, analysis_key) =
        first_job_state.expect("first job state");
    let (latest_uri, latest_revision, latest_version, latest_generation, latest_key) =
        latest_job_state.expect("latest job state");
    assert_eq!(analysis_key, latest_key);
    assert!(latest_generation > first_generation);
    let current_generation = documents.generation(&analysis_key);
    assert!(!commit_diagnostics_if_current(
        &mut documents,
        &first_uri,
        first_revision,
        first_version,
        first_generation,
        current_generation,
        Vec::new(),
    ));
    assert!(commit_diagnostics_if_current(
        &mut documents,
        &latest_uri,
        latest_revision,
        latest_version,
        latest_generation,
        current_generation,
        Vec::new(),
    ));

    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        let _ = task.await;
    }
    fs::remove_dir_all(package_dir).expect("cleanup package");
}

#[tokio::test]
async fn blocking_work_is_bounded_under_stress() {
    let permits = Arc::new(Semaphore::new(MAX_BLOCKING_ANALYSES));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();

    for _ in 0..16 {
        let permits = Arc::clone(&permits);
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        tasks.push(tokio::spawn(async move {
            run_bounded_blocking(permits, move || {
                let current = active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                peak.fetch_max(current, AtomicOrdering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                active.fetch_sub(1, AtomicOrdering::SeqCst);
            })
            .await
            .expect("blocking task should finish");
        }));
    }

    for task in tasks {
        task.await.expect("bounded task should finish");
    }
    assert_eq!(peak.load(AtomicOrdering::SeqCst), MAX_BLOCKING_ANALYSES);
}

#[tokio::test]
async fn feature_snapshot_releases_document_lock_during_symbol_scans() {
    let uri = file_url("lock-release.rss");
    let source = (0..512)
        .map(|index| format!("fn value_{index}() -> Int {{ return {index} }}\n"))
        .collect::<String>();
    let mut initial = DocumentStore::new();
    open_document(&mut initial, uri.clone(), source, 1).expect("document should open");
    let documents = Arc::new(tokio::sync::Mutex::new(initial));
    let snapshot = snapshot_documents(&documents).await;
    let scan = tokio::task::spawn_blocking(move || {
        for _ in 0..16 {
            let document = snapshot.get(&uri).expect("snapshot document");
            assert!(
                !symbol_index(uri.path(), &document.text)
                    .definitions()
                    .is_empty()
            );
        }
    });

    for _ in 0..32 {
        let guard = tokio::time::timeout(Duration::from_millis(100), documents.lock())
            .await
            .expect("feature scan must not retain the document lock");
        drop(guard);
        tokio::task::yield_now().await;
    }
    scan.await.expect("symbol scan should finish");
}

#[test]
fn blocking_analysis_stops_at_cooperative_checkpoint() {
    let documents = (0..8)
        .map(|index| {
            workspace_document(
                file_url(&format!("cancel-{index}.rss")),
                "fn broken( -> Unit {}\n",
            )
        })
        .collect::<Vec<_>>();
    let mut checkpoints = 0;

    let diagnostics = package_frontend_diagnostics_cancellable(&documents, &mut || {
        checkpoints += 1;
        checkpoints >= 3
    });

    assert!(diagnostics.is_none());
    assert_eq!(checkpoints, 3);
}

#[test]
fn cancelled_snapshot_cannot_replace_existing_diagnostics() {
    let uri = file_url("cancelled-stale.rss");
    let mut current = document("fn current() -> Unit {}\n");
    current.revision = 2;
    current.version = 2;
    let mut documents = HashMap::from([(uri.clone(), current)]);
    let snapshot = HashMap::from([(
        uri.clone(),
        Document {
            text: Arc::from("fn stale( -> Unit {}\n"),
            diagnostics: Arc::new(Vec::new()),
            revision: 1,
            version: 1,
            sync_state: DocumentSyncState::Synchronized,
            source_index: Arc::new(SourceIndexCache::default()),
        },
    )]);
    let cancellation = AnalysisCancellation::default();
    cancellation.cancel();

    let result =
        diagnostics_for_uri_cancellable(&uri, &snapshot, &PackageInputCache::default(), || {
            cancellation.is_cancelled()
        });

    assert!(result.is_none());
    assert!(!commit_diagnostics_if_current(
        &mut documents,
        &uri,
        1,
        1,
        1,
        2,
        Vec::new(),
    ));
    let current = documents.get(&uri).expect("current document remains");
    assert_eq!(current.revision, 2);
    assert_eq!(current.version, 2);
}

#[test]
fn stale_analysis_cannot_replace_newer_diagnostics() {
    let uri = file_url("stale.rss");
    let mut documents = HashMap::from([(uri.clone(), document("new source"))]);
    documents
        .get_mut(&uri)
        .expect("document should exist")
        .revision = 2;

    assert!(!commit_diagnostics_if_current(
        &mut documents,
        &uri,
        1,
        0,
        1,
        2,
        Vec::new(),
    ));
    assert_eq!(
        documents
            .get(&uri)
            .expect("document should remain")
            .revision,
        2
    );
}

#[test]
fn analysis_for_stale_version_cannot_replace_diagnostics() {
    let uri = file_url("stale-version.rss");
    let mut documents = HashMap::from([(uri.clone(), document("new source"))]);
    let document = documents.get_mut(&uri).expect("document should exist");
    document.revision = 2;
    document.version = 3;

    assert!(!commit_diagnostics_if_current(
        &mut documents,
        &uri,
        2,
        2,
        2,
        2,
        Vec::new(),
    ));
    assert_eq!(
        documents.get(&uri).expect("document should remain").version,
        3
    );
}

#[test]
fn workspace_generation_prevents_cross_document_stale_publish() {
    let uri = file_url("generation.rss");
    let mut documents = HashMap::from([(uri.clone(), document("unchanged target"))]);

    assert!(!commit_diagnostics_if_current(
        &mut documents,
        &uri,
        0,
        0,
        4,
        5,
        Vec::new(),
    ));
    assert!(
        documents
            .get(&uri)
            .expect("target remains open")
            .diagnostics
            .is_empty()
    );
}

#[tokio::test]
async fn concurrent_incremental_changes_apply_to_the_committed_version() {
    let uri = file_url("concurrent.rss");
    let mut initial = DocumentStore::new();
    open_document(&mut initial, uri.clone(), "a".to_string(), 1)
        .expect("initial document should open");
    let documents = Arc::new(tokio::sync::Mutex::new(initial));
    let (version_two_done, wait_for_version_two) = oneshot::channel();

    let first_documents = Arc::clone(&documents);
    let first_uri = uri.clone();
    let version_two = tokio::spawn(async move {
        let change = TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 1), Position::new(0, 1))),
            range_length: None,
            text: "b".to_string(),
        };
        let job = {
            let mut documents = first_documents.lock().await;
            change_document(&mut documents, first_uri, 2, &[change])
        };
        version_two_done
            .send(())
            .expect("version three should still be waiting");
        job
    });

    let second_documents = Arc::clone(&documents);
    let second_uri = uri.clone();
    let version_three = tokio::spawn(async move {
        wait_for_version_two
            .await
            .expect("version two should complete");
        let change = TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 2), Position::new(0, 2))),
            range_length: None,
            text: "c".to_string(),
        };
        let mut documents = second_documents.lock().await;
        change_document(&mut documents, second_uri, 3, &[change])
    });

    assert!(
        version_two
            .await
            .expect("version two task should finish")
            .is_applied()
    );
    assert!(
        version_three
            .await
            .expect("version three task should finish")
            .is_applied()
    );

    let documents = documents.lock().await;
    let document = documents.get(&uri).expect("document should remain open");
    assert_eq!(document.text.as_ref(), "abc");
    assert_eq!(document.version, 3);
    assert_eq!(document.revision, 3);
    assert_eq!(documents.next_revision, 4);
}

#[tokio::test]
async fn late_out_of_order_change_is_ignored_without_allocating_revision() {
    let uri = file_url("out-of-order.rss");
    let mut initial = DocumentStore::new();
    open_document(&mut initial, uri.clone(), "initial".to_string(), 1)
        .expect("initial document should open");
    let documents = Arc::new(tokio::sync::Mutex::new(initial));
    let (newer_done, wait_for_newer) = oneshot::channel();

    let newer_documents = Arc::clone(&documents);
    let newer_uri = uri.clone();
    let newer = tokio::spawn(async move {
        let change = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "newest".to_string(),
        };
        let job = {
            let mut documents = newer_documents.lock().await;
            change_document(&mut documents, newer_uri, 3, &[change])
        };
        newer_done
            .send(())
            .expect("older change should still be waiting");
        job
    });

    let older_documents = Arc::clone(&documents);
    let older_uri = uri.clone();
    let older = tokio::spawn(async move {
        wait_for_newer.await.expect("newer change should complete");
        let change = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "stale".to_string(),
        };
        let mut documents = older_documents.lock().await;
        change_document(&mut documents, older_uri, 2, &[change])
    });

    assert!(newer.await.expect("newer task should finish").is_applied());
    assert!(matches!(
        older.await.expect("older task should finish"),
        ChangeOutcome::IgnoredStale
    ));

    let documents = documents.lock().await;
    let document = documents.get(&uri).expect("document should remain open");
    assert_eq!(document.text.as_ref(), "newest");
    assert_eq!(document.version, 3);
    assert_eq!(document.sync_state, DocumentSyncState::Synchronized);
    assert_eq!(document.revision, 2);
    assert_eq!(documents.next_revision, 3);
}

#[test]
fn workspace_definition_resolves_unresolved_call_in_open_document() {
    let callee_uri = file_url("callee.rss");
    let caller_uri = file_url("caller.rss");
    let mut documents = HashMap::new();
    documents.insert(
        callee_uri.clone(),
        document("fn helper(value: Int) -> Int {\n    return value\n}\n"),
    );
    documents.insert(
        caller_uri.clone(),
        document("fn run() -> Int {\n    return helper(value: 1)\n}\n"),
    );
    let caller = documents.get(&caller_uri).expect("caller document");
    let index = symbol_index("/workspace/caller.rss", &caller.text);
    let lookup = index.lookup_at(2, 12).expect("helper lookup");
    let workspace = workspace_documents(&documents, &PackageInputCache::default());

    let location =
        workspace_definition_location(&workspace, &lookup).expect("workspace definition");

    assert_eq!(location.uri, callee_uri);
    assert_eq!(location.range.start.line, 0);
}

#[test]
fn workspace_references_collect_unresolved_cross_file_calls() {
    let callee_uri = file_url("callee.rss");
    let caller_uri = file_url("caller.rss");
    let mut documents = HashMap::new();
    documents.insert(
        callee_uri.clone(),
        document("fn helper(value: Int) -> Int {\n    return value\n}\n"),
    );
    documents.insert(
        caller_uri.clone(),
        document("fn run() -> Int {\n    return helper(value: 1)\n}\n"),
    );
    let caller = documents.get(&caller_uri).expect("caller document");
    let index = symbol_index("/workspace/caller.rss", &caller.text);
    let lookup = index.lookup_at(2, 12).expect("helper lookup");
    let workspace = workspace_documents(&documents, &PackageInputCache::default());

    let locations = workspace_reference_locations(&workspace, &lookup, true);

    assert!(locations.iter().any(|location| location.uri == callee_uri));
    assert!(locations.iter().any(|location| location.uri == caller_uri));
}

#[test]
fn document_highlight_locations_stay_in_current_document() {
    let uri = file_url("highlight-local.rss");
    let source = concat!(
        "fn run(value: Int) -> Int {\n",
        "    let next = value\n",
        "    return next\n",
        "}\n",
    );
    let mut documents = HashMap::new();
    documents.insert(uri.clone(), document(source));

    let locations = reference_locations_for_position(
        &uri,
        Position {
            line: 1,
            character: 15,
        },
        &documents,
        true,
        &PackageInputCache::default(),
    );

    assert_eq!(locations.len(), 2);
    assert!(locations.iter().all(|location| location.uri == uri));
    assert!(
        locations
            .iter()
            .any(|location| location.range.start.line == 0)
    );
    assert!(
        locations
            .iter()
            .any(|location| location.range.start.line == 1)
    );
}

#[test]
fn prepare_rename_returns_symbol_range_and_placeholder() {
    let uri = file_url("prepare-rename.rss");
    let source = "fn run(value: Int) -> Int {\n    return value\n}\n";
    let mut documents = HashMap::new();
    documents.insert(uri.clone(), document(source));

    let (range, placeholder) = rename_target(
        &uri,
        Position {
            line: 1,
            character: 12,
        },
        &documents,
    )
    .expect("rename target");

    assert_eq!(placeholder, "value");
    assert_eq!(range.start.line, 0);
    assert_eq!(range.start.character, 7);
    assert_eq!(range.end.character, 12);
}

#[test]
fn semantic_tokens_mark_review_relevant_language_roles() {
    let source = concat!(
        "fn transform(path: read String) -> String {\n",
        "    return path\n",
        "}\n",
    );

    let tokens = semantic_tokens_for_source("/workspace/review.rss", source);

    assert!(
        tokens
            .data
            .iter()
            .any(|token| token.token_type == TOKEN_FUNCTION
                && token.token_modifiers_bitset & MOD_DEFINITION != 0)
    );
    assert!(
        tokens
            .data
            .iter()
            .any(|token| token.token_type == TOKEN_KEYWORD)
    );
}

#[test]
fn call_hierarchy_reports_incoming_and_outgoing_calls() {
    let uri = file_url("call-hierarchy.rss");
    let source = concat!(
        "fn leaf() -> Int {\n",
        "    return 1\n",
        "}\n",
        "\n",
        "fn caller() -> Int {\n",
        "    return leaf()\n",
        "}\n",
    );
    let workspace = vec![workspace_document(uri.clone(), source)];
    let (_, leaf_definition) =
        find_function_definition_with_document(&workspace, "leaf").expect("leaf definition");
    let (_, caller_definition) =
        find_function_definition_with_document(&workspace, "caller").expect("caller definition");
    let leaf_item = to_call_hierarchy_item(source, &uri, &leaf_definition);
    let caller_item = to_call_hierarchy_item(source, &uri, &caller_definition);

    let incoming = incoming_call_hierarchy(&workspace, &leaf_item);
    let outgoing = outgoing_call_hierarchy(&workspace, &caller_item);

    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].from.name, "caller");
    assert_eq!(incoming[0].from_ranges[0].start.line, 5);
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].to.name, "leaf");
    assert_eq!(outgoing[0].from_ranges[0].start.line, 5);
}

#[test]
fn workspace_definition_loads_package_sources_from_disk() {
    let package_dir = unique_temp_dir("rss-lsp-package-definition");
    fs::create_dir_all(package_dir.join("src")).expect("create src");
    fs::write(
        package_dir.join("rsspkg.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write manifest");
    fs::write(
        package_dir.join("src/helper.rss"),
        "fn helper(value: Int) -> Int {\n    return value\n}\n",
    )
    .expect("write helper");
    let caller_text = "fn run() -> Int {\n    return helper(value: 1)\n}\n";
    let caller_path = package_dir.join("src/main.rss");
    fs::write(&caller_path, caller_text).expect("write caller");
    let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
    let helper_uri = Url::from_file_path(package_dir.join("src/helper.rss")).expect("helper URL");
    let mut documents = HashMap::new();
    documents.insert(caller_uri.clone(), document(caller_text));
    let index = symbol_index(caller_uri.path(), caller_text);
    let lookup = index.lookup_at(2, 12).expect("helper lookup");
    let workspace =
        workspace_documents_for_uri(&caller_uri, &documents, &PackageInputCache::default());

    let location = workspace_definition_location(&workspace, &lookup).expect("package definition");

    assert_eq!(location.uri, helper_uri);

    fs::remove_dir_all(package_dir).expect("cleanup package");
}

#[test]
fn package_input_cache_reuses_and_invalidates_immutable_inputs() {
    let package_dir = unique_temp_dir("rss-lsp-package-input-cache");
    fs::create_dir_all(package_dir.join("src")).expect("create src");
    fs::write(
        package_dir.join("rsspkg.toml"),
        "[package]\nname = \"cache\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write manifest");
    let source_path = package_dir.join("src/main.rss");
    fs::write(&source_path, "fn old() -> Unit {}\n").expect("write source");
    let source_uri = Url::from_file_path(&source_path).expect("source URL");
    let cache = PackageInputCache::default();

    let first = cache.documents_for_root(&package_dir);
    let first_document = first.get(&source_uri).expect("first source");
    let first_generation = first_document.semantic_generation;
    let first_index = first_document.symbol_index();
    assert_eq!(first_document.source_index.build_count(), 1);
    fs::write(&source_path, "fn new() -> Unit {}\n").expect("rewrite source");
    let cached = cache.documents_for_root(&package_dir);
    assert!(Arc::ptr_eq(&first, &cached));
    assert_eq!(
        cached
            .get(&source_uri)
            .expect("cached source")
            .text
            .as_ref(),
        "fn old() -> Unit {}\n"
    );
    let cached_index = cached
        .get(&source_uri)
        .expect("cached source")
        .symbol_index();
    assert!(Arc::ptr_eq(&first_index, &cached_index));
    assert_eq!(first_document.source_index.build_count(), 1);

    cache.invalidate(&package_dir);
    let refreshed = cache.documents_for_root(&package_dir);
    assert!(!Arc::ptr_eq(&cached, &refreshed));
    assert_eq!(
        refreshed
            .get(&source_uri)
            .expect("refreshed source")
            .text
            .as_ref(),
        "fn new() -> Unit {}\n"
    );
    let refreshed_document = refreshed.get(&source_uri).expect("refreshed source");
    assert_ne!(
        refreshed_document.semantic_generation, first_generation,
        "package invalidation must advance the semantic generation"
    );
    let refreshed_index = refreshed_document.symbol_index();
    assert!(!Arc::ptr_eq(&cached_index, &refreshed_index));
    assert_eq!(refreshed_index.definitions()[0].name, "new");

    assert_eq!(
        cache.invalidate_path(&source_path),
        vec![package_dir.clone()]
    );
    let invalidated = cache.documents_for_root(&package_dir);
    assert!(!Arc::ptr_eq(&refreshed, &invalidated));

    fs::remove_dir_all(package_dir).expect("cleanup package");
}

#[test]
fn package_input_cache_recovers_from_mutex_poisoning() {
    let cache = Arc::new(PackageInputCache::default());
    let poisoning_cache = Arc::clone(&cache);
    let _ = std::thread::spawn(move || {
        let _guard = poisoning_cache
            .documents
            .lock()
            .expect("cache initially unlocked");
        panic!("poison package input cache for recovery test");
    })
    .join();

    cache.invalidate(Path::new("/workspace"));
    assert!(cache.lock_documents().is_empty());
}

#[test]
fn workspace_symbols_include_package_sources_from_disk() {
    let package_dir = unique_temp_dir("rss-lsp-package-symbols");
    fs::create_dir_all(package_dir.join("src")).expect("create src");
    fs::write(
        package_dir.join("rsspkg.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write manifest");
    fs::write(
        package_dir.join("src/helper.rss"),
        "fn helper(value: Int) -> Int {\n    return value\n}\n",
    )
    .expect("write helper");
    let caller_path = package_dir.join("src/main.rss");
    let caller_text = "fn run() -> Int {\n    return helper(value: 1)\n}\n";
    fs::write(&caller_path, caller_text).expect("write caller");
    let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
    let mut documents = HashMap::new();
    documents.insert(caller_uri, document(caller_text));

    let workspace = workspace_documents(&documents, &PackageInputCache::default());

    assert!(
        workspace
            .iter()
            .any(|document| document.uri.path().ends_with("helper.rss"))
    );
    assert!(
        workspace
            .iter()
            .any(|document| document.uri.path().ends_with("main.rss"))
    );

    fs::remove_dir_all(package_dir).expect("cleanup package");
}

#[test]
fn package_diagnostics_use_interface_sources() {
    let package_dir = unique_temp_dir("rss-lsp-package-diagnostics-interface");
    fs::create_dir_all(package_dir.join("interface")).expect("create interface");
    fs::create_dir_all(package_dir.join("src")).expect("create src");
    fs::write(
        package_dir.join("rsspkg.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write manifest");
    fs::write(package_dir.join("interface/api.rssi"), "struct Widget\n").expect("write interface");
    let caller_text = "struct Holder {\n    value: Widget\n}\n";
    let caller_path = package_dir.join("src/main.rss");
    fs::write(&caller_path, caller_text).expect("write caller");
    let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
    let mut documents = HashMap::new();
    documents.insert(caller_uri.clone(), document(caller_text));

    let single_file = single_file_diagnostics(caller_uri.path(), caller_text);
    let package = diagnostics_for_uri(&caller_uri, &documents);

    assert!(
        single_file
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown type `Widget`"))
    );
    assert!(
        !package
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown type `Widget`"))
    );

    fs::remove_dir_all(package_dir).expect("cleanup package");
}

#[test]
fn package_diagnostics_use_dependency_interface_sources() {
    let workspace_dir = unique_temp_dir("rss-lsp-package-dependency-interface");
    let dependency_dir = workspace_dir.join("dep");
    let package_dir = workspace_dir.join("app");
    fs::create_dir_all(dependency_dir.join("interface")).expect("create dependency interface");
    fs::create_dir_all(package_dir.join("src")).expect("create package src");
    fs::write(
        dependency_dir.join("rsspkg.toml"),
        concat!(
            "[package]\n",
            "name = \"dep\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[interfaces]\n",
            "paths = [\"interface\"]\n",
        ),
    )
    .expect("write dependency manifest");
    fs::write(
        dependency_dir.join("interface/api.rssi"),
        "pub fn Dep.helper(value: read Int) -> Int\n",
    )
    .expect("write dependency interface");
    fs::write(
        package_dir.join("rsspkg.toml"),
        concat!(
            "[package]\n",
            "name = \"demo\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[dependencies]\n",
            "dep = { path = \"../dep\" }\n",
        ),
    )
    .expect("write package manifest");
    let caller_text = "fn run() -> Int {\n    return Dep.helper(value: read 1)\n}\n";
    let caller_path = package_dir.join("src/main.rss");
    fs::write(&caller_path, caller_text).expect("write caller");
    let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
    let mut documents = HashMap::new();
    documents.insert(caller_uri.clone(), document(caller_text));

    let single_file = single_file_diagnostics(caller_uri.path(), caller_text);
    let package = diagnostics_for_uri(&caller_uri, &documents);

    assert!(
        single_file
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("Dep.helper"))
    );
    assert!(
        !package
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("Dep.helper"))
    );

    fs::remove_dir_all(workspace_dir).expect("cleanup workspace");
}

#[test]
fn package_diagnostics_overlay_open_interface_document() {
    let package_dir = unique_temp_dir("rss-lsp-package-diagnostics-overlay");
    fs::create_dir_all(package_dir.join("interface")).expect("create interface");
    fs::create_dir_all(package_dir.join("src")).expect("create src");
    fs::write(
        package_dir.join("rsspkg.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write manifest");
    let interface_path = package_dir.join("interface/api.rssi");
    fs::write(&interface_path, "struct OldWidget\n").expect("write interface");
    let caller_text = "struct Holder {\n    value: Widget\n}\n";
    let caller_path = package_dir.join("src/main.rss");
    fs::write(&caller_path, caller_text).expect("write caller");
    let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
    let interface_uri = Url::from_file_path(&interface_path).expect("interface URL");
    let mut documents = HashMap::new();
    documents.insert(caller_uri.clone(), document(caller_text));
    documents.insert(interface_uri, document("struct Widget\n"));

    let package = diagnostics_for_uri(&caller_uri, &documents);

    assert!(
        !package
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown type `Widget`"))
    );

    fs::remove_dir_all(package_dir).expect("cleanup package");
}

#[test]
fn pull_diagnostics_use_structured_lsp_diagnostic_data() {
    let uri = file_url("pull-diagnostics.rss");
    let source = "fn run() -> Int {\n    return missing\n}\n";
    let mut documents = HashMap::new();
    documents.insert(uri.clone(), document(source));

    let diagnostics = lsp_diagnostics_for_uri(&uri, &documents);

    assert_eq!(diagnostics.len(), 1);
    let data = diagnostics[0].data.as_ref().expect("diagnostic data");
    assert_eq!(data["schema"], "rsscript.lsp.diagnostic.v1");
    assert_eq!(data["code"], "RS0026");
    assert_eq!(data["span"]["file"], uri.path());
}

#[test]
fn hover_symbol_info_uses_package_definition_detail() {
    let package_dir = unique_temp_dir("rss-lsp-package-hover");
    fs::create_dir_all(package_dir.join("src")).expect("create src");
    fs::write(
        package_dir.join("rsspkg.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write manifest");
    fs::write(
        package_dir.join("src/helper.rss"),
        "fn helper(value: read Int) -> Int {\n    return value\n}\n",
    )
    .expect("write helper");
    let caller_text = "fn run() -> Int {\n    return helper(value: read 1)\n}\n";
    let caller_path = package_dir.join("src/main.rss");
    fs::write(&caller_path, caller_text).expect("write caller");
    let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
    let mut documents = HashMap::new();
    documents.insert(caller_uri.clone(), document(caller_text));
    let index = symbol_index(caller_uri.path(), caller_text);

    let symbol = hover_symbol_info(
        &caller_uri,
        &documents,
        &index,
        2,
        12,
        &PackageInputCache::default(),
    )
    .expect("helper hover symbol");
    let markdown = symbol_hover_markdown(&symbol);

    assert_eq!(symbol.name, "helper");
    assert!(markdown.contains("fn(value: read Int) -> Int"));

    fs::remove_dir_all(package_dir).expect("cleanup package");
}

#[test]
fn call_context_tracks_active_parameter() {
    let source = "fn run() -> Unit {\n    helper(first: read 1, second: read value\n}\n";
    let context = call_context_at(
        source,
        Position {
            line: 1,
            character: 40,
        },
    )
    .expect("call context");

    assert_eq!(context.callee, "helper");
    assert_eq!(context.active_parameter, 1);
}

#[test]
fn signature_help_uses_package_function_detail() {
    let package_dir = unique_temp_dir("rss-lsp-package-signature");
    fs::create_dir_all(package_dir.join("src")).expect("create src");
    fs::write(
        package_dir.join("rsspkg.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write manifest");
    fs::write(
        package_dir.join("src/helper.rss"),
        concat!(
            "fn helper(first: read Int, second: read String) -> Unit {\n",
            "    return\n",
            "}\n",
        ),
    )
    .expect("write helper");
    let caller_path = package_dir.join("src/main.rss");
    let caller_text = "fn run() -> Unit {\n    helper(first: read 1, second: read \"x\")\n}\n";
    fs::write(&caller_path, caller_text).expect("write caller");
    let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
    let mut documents = HashMap::new();
    documents.insert(caller_uri.clone(), document(caller_text));
    let workspace =
        workspace_documents_for_uri(&caller_uri, &documents, &PackageInputCache::default());
    let context = call_context_at(
        caller_text,
        Position {
            line: 1,
            character: 35,
        },
    )
    .expect("call context");
    let definition =
        workspace_function_definition(&workspace, &context.callee).expect("helper definition");

    let signature = signature_information(&definition, context.active_parameter)
        .expect("signature information");

    assert!(
        signature
            .label
            .contains("fn(first: read Int, second: read String)")
    );
    assert_eq!(signature.active_parameter, Some(1));
    let parameters = signature.parameters.expect("parameters");
    assert_eq!(parameters.len(), 2);

    fs::remove_dir_all(package_dir).expect("cleanup package");
}

#[test]
fn rename_local_symbol_stays_in_current_scope() {
    let uri = file_url("rename-local.rss");
    let source = concat!(
        "fn first(value: Int) -> Int {\n",
        "    return value\n",
        "}\n",
        "\n",
        "fn second(value: Int) -> Int {\n",
        "    return value\n",
        "}\n",
    );
    let mut documents = HashMap::new();
    documents.insert(uri.clone(), document(source));

    let edit = rename_workspace_edit(
        &uri,
        Position {
            line: 1,
            character: 12,
        },
        "amount",
        &documents,
        &PackageInputCache::default(),
    )
    .expect("rename edit");
    let changes = edit.changes.expect("changes");
    let edits = changes.get(&uri).expect("local edits");

    assert_eq!(edits.len(), 2);
    assert!(edits.iter().any(|edit| edit.range.start.line == 0));
    assert!(edits.iter().any(|edit| edit.range.start.line == 1));
    assert!(!edits.iter().any(|edit| edit.range.start.line == 4));
    assert!(!edits.iter().any(|edit| edit.range.start.line == 5));
}

#[test]
fn rename_top_level_symbol_updates_package_references() {
    let callee_uri = file_url("rename-callee.rss");
    let caller_uri = file_url("rename-caller.rss");
    let mut documents = HashMap::new();
    documents.insert(
        callee_uri.clone(),
        document("fn helper(value: Int) -> Int {\n    return value\n}\n"),
    );
    documents.insert(
        caller_uri.clone(),
        document("fn run() -> Int {\n    return helper(value: 1)\n}\n"),
    );

    let edit = rename_workspace_edit(
        &caller_uri,
        Position {
            line: 1,
            character: 12,
        },
        "compute",
        &documents,
        &PackageInputCache::default(),
    )
    .expect("rename edit");
    let changes = edit.changes.expect("changes");
    let callee_edits = changes.get(&callee_uri).expect("callee edits");
    let caller_edits = changes.get(&caller_uri).expect("caller edits");

    assert_eq!(callee_edits.len(), 1);
    assert_eq!(caller_edits.len(), 1);
    assert_eq!(callee_edits[0].new_text, "compute");
    assert_eq!(caller_edits[0].new_text, "compute");
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nanos}"))
}

#[test]
fn lsp_diagnostic_carries_related_causes_fixes_and_explanation() {
    let diagnostic = RsDiagnostic::error(
        "RS0026",
        "unknown value binding `missing`.",
        Span {
            file: "/workspace/main.rss".to_string(),
            line: 2,
            column: 12,
            length: 7,
        },
        "unknown binding",
    )
    .with_cause("RSScript values must resolve before Rust lowering.")
    .with_fix(
        "declare_binding",
        "Declare `missing` before using it.",
        "manual",
    );

    let lsp = to_lsp_diagnostic("fn run() -> Unit {\n    return missing\n}\n", &diagnostic);
    let related = lsp
        .related_information
        .expect("related diagnostic information");

    assert!(
        related
            .iter()
            .any(|info| info.message.starts_with("cause:"))
    );
    assert!(related.iter().any(|info| info.message.starts_with("fix:")));
    assert!(
        related
            .iter()
            .any(|info| info.message.contains("unknown binding"))
    );
    let data = lsp.data.expect("structured diagnostic data");
    assert_eq!(data["schema"], "rsscript.lsp.diagnostic.v1");
    assert_eq!(data["code"], "RS0026");
    assert_eq!(data["severity"], "error");
    assert_eq!(
        data["causes"][0],
        "RSScript values must resolve before Rust lowering."
    );
    assert_eq!(data["fixes"][0]["kind"], "declare_binding");
    assert_eq!(data["fixes"][0]["applicability"], "manual");
    assert_eq!(data["explanation"]["code"], "RS0026");
}
