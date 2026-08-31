//! `CompilationSession` method implementation, split out of `database.rs` for
//! module-size partitioning. A second `impl CompilationSession` block; the type
//! and its fields remain defined in `database.rs`.

use super::*;

impl CompilationSession {
    /// Create a session with an explicit immutable interface policy.
    pub fn with_interface_policy(interface_policy: SessionInterfacePolicy) -> Self {
        Self {
            sources: SessionSourceStore::default(),
            interfaces: SessionSourceStore::default(),
            interface_policy,
            parse_cache: BTreeMap::new(),
            hir_cache: BTreeMap::new(),
            workspace_hir_cache: None,
            workspace_type_cache: None,
            workspace_analysis_cache: None,
            semantic_document_analysis_cache: BTreeMap::new(),
            semantic_document_diagnostic_cache: BTreeMap::new(),
            syntax_diagnostic_cache: BTreeMap::new(),
            lint_cache: BTreeMap::new(),
            format_cache: BTreeMap::new(),
            symbol_cache: BTreeMap::new(),
            document_symbol_cache: BTreeMap::new(),
            module_header_cache: BTreeMap::new(),
            workspace_module_graph_cache: None,
            workspace_diagnostic_cache: None,
            parse_cache_hits: 0,
            parse_cache_misses: 0,
            hir_cache_hits: 0,
            hir_cache_misses: 0,
            workspace_hir_cache_hits: 0,
            workspace_hir_cache_misses: 0,
            workspace_type_cache_hits: 0,
            workspace_type_cache_misses: 0,
            workspace_analysis_cache_hits: 0,
            workspace_analysis_cache_misses: 0,
            lint_cache_hits: 0,
            lint_cache_misses: 0,
            format_cache_hits: 0,
            format_cache_misses: 0,
            symbol_cache_hits: 0,
            symbol_cache_misses: 0,
            document_symbol_cache_hits: 0,
            document_symbol_cache_misses: 0,
            module_header_cache_hits: 0,
            module_header_cache_misses: 0,
            workspace_module_graph_cache_hits: 0,
            workspace_module_graph_cache_misses: 0,
            workspace_diagnostic_cache_hits: 0,
            workspace_diagnostic_cache_misses: 0,
            semantic_document_analysis_cache_hits: 0,
            semantic_document_analysis_cache_misses: 0,
            semantic_document_diagnostic_cache_hits: 0,
            semantic_document_diagnostic_cache_misses: 0,
        }
    }

    /// Create a session that analyzes only explicitly supplied interfaces.
    pub fn without_core() -> Self {
        Self::with_interface_policy(SessionInterfacePolicy::WithoutCore)
    }

    /// Create a session whose analysis exactly matches the legacy standard
    /// package prelude. Explicit interfaces are rejected because mixing them
    /// with that prelude would select a different semantic contract.
    pub fn with_standard_packages() -> Self {
        Self::with_interface_policy(SessionInterfacePolicy::WithStandardPackages)
    }

    /// Return the semantic policy fixed when this session was created.
    pub const fn interface_policy(&self) -> SessionInterfacePolicy {
        self.interface_policy
    }

    pub fn set_file(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<SourceUpdate, SourceStoreError> {
        let path = path.into();
        let previous_header = self.cached_module_header(SessionFileRole::Source, &path);
        let update = self.sources.set_file(path.clone(), text)?;
        let graph_unchanged = update.changed
            && self.module_header_is_unchanged(
                SessionFileRole::Source,
                &path,
                previous_header.as_deref(),
            );
        if update.changed {
            self.invalidate_parse_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_syntax_diagnostic_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_editor_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_semantic_document_cache_for_source(update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Source, update.file_id);
            if !graph_unchanged {
                self.workspace_module_graph_cache = None;
            }
            self.workspace_diagnostic_cache = None;
        }
        Ok(update)
    }

    pub fn remove_file(&mut self, path: &str) -> Option<SourceUpdate> {
        let update = self.sources.remove_file(path);
        if let Some(update) = update {
            self.invalidate_parse_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_syntax_diagnostic_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_editor_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_semantic_document_cache_for_source(update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Source, update.file_id);
            self.workspace_module_graph_cache = None;
            self.workspace_diagnostic_cache = None;
        }
        update
    }

    pub fn set_interface(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<SourceUpdate, SourceStoreError> {
        if self.interface_policy == SessionInterfacePolicy::WithStandardPackages {
            return Err(SourceStoreError::InterfacesForbiddenByPolicy {
                policy: self.interface_policy,
            });
        }
        let path = path.into();
        let previous_header = self.cached_module_header(SessionFileRole::Interface, &path);
        let update = self.interfaces.set_file(path.clone(), text)?;
        let graph_unchanged = update.changed
            && self.module_header_is_unchanged(
                SessionFileRole::Interface,
                &path,
                previous_header.as_deref(),
            );
        if update.changed {
            self.invalidate_parse_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_syntax_diagnostic_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_editor_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_semantic_document_cache_for_interface(update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Interface, update.file_id);
            if !graph_unchanged {
                self.workspace_module_graph_cache = None;
            }
            self.workspace_diagnostic_cache = None;
        }
        Ok(update)
    }

    pub fn remove_interface(&mut self, path: &str) -> Option<SourceUpdate> {
        let update = self.interfaces.remove_file(path);
        if let Some(update) = update {
            self.invalidate_parse_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_syntax_diagnostic_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_editor_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_semantic_document_cache_for_interface(update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Interface, update.file_id);
            self.workspace_module_graph_cache = None;
            self.workspace_diagnostic_cache = None;
        }
        update
    }

    pub fn source_snapshot(&self) -> SourceSnapshot {
        self.sources.snapshot()
    }

    pub fn interface_snapshot(&self) -> SourceSnapshot {
        self.interfaces.snapshot()
    }

    /// Return one immutable source-file revision owned by this session.
    pub fn source_file_snapshot(&self, path: &str) -> Option<SourceFileSnapshot> {
        self.source_snapshot()
            .files()
            .iter()
            .find(|file| file.path() == path)
            .cloned()
    }

    /// Return one immutable interface-file revision owned by this session.
    pub fn interface_file_snapshot(&self, path: &str) -> Option<SourceFileSnapshot> {
        self.interface_snapshot()
            .files()
            .iter()
            .find(|file| file.path() == path)
            .cloned()
    }

    /// Capture both session-owned input roles for one workspace semantic query.
    ///
    /// The returned input owns immutable bytes and stable file revisions. A
    /// caller can therefore not analyze a document set different from the one
    /// whose query result the session caches.
    pub fn frontend_input_snapshot(&self) -> FrontendInputSnapshot {
        FrontendInputSnapshot::from_snapshots(self.source_snapshot(), self.interface_snapshot())
    }

    /// Run the complete source/interface semantic analysis for the current
    /// immutable session revision set. This is the single cached query for
    /// resolve, type, checked-HIR, and source diagnostics; callers cannot
    /// accidentally assemble an equivalent analysis from independently read
    /// files or a second analyzer instance.
    pub fn workspace_analysis(&mut self) -> Arc<AnalysisResult> {
        self.workspace_analysis_inner(None)
            .expect("an unchecked workspace analysis query cannot abort")
    }

    fn workspace_analysis_inner(
        &mut self,
        operation: Option<&OperationContext>,
    ) -> Result<Arc<AnalysisResult>, OperationAbort> {
        if let Some(operation) = operation {
            operation.check()?;
        }
        if let Some(analysis) = &self.workspace_analysis_cache {
            self.workspace_analysis_cache_hits =
                self.workspace_analysis_cache_hits.saturating_add(1);
            if let Some(operation) = operation {
                operation.check()?;
            }
            return Ok(Arc::clone(analysis));
        }

        let input = self.frontend_input_snapshot();
        let sources = input
            .sources()
            .files()
            .iter()
            .map(|file| (file.path(), file.text()))
            .collect::<Vec<_>>();
        let interfaces = input
            .interfaces()
            .files()
            .iter()
            .map(|file| (file.path(), file.text()))
            .collect::<Vec<_>>();
        // A session owns one immutable workspace input, but a one-source
        // workspace must retain the historical single-document semantic path.
        // That path carries source-local protocol and generic facts which are
        // not yet represented by the package merge query. Selecting it here
        // preserves semantics without letting callers bypass the session's
        // revision, cancellation, or cache boundary.
        let analysis = Arc::new(
            match (sources.as_slice(), operation, self.interface_policy) {
                (
                    [(path, source)],
                    Some(operation),
                    SessionInterfacePolicy::WithStandardPackages,
                ) => crate::analyze_source_result_with_operation(path, source, operation),
                ([(path, source)], None, SessionInterfacePolicy::WithStandardPackages) => {
                    crate::analyze_source_result(path, source)
                }
                (_, Some(operation), SessionInterfacePolicy::WithStandardPackages) => {
                    crate::analyze_sources_with_standard_packages_result_with_operation(
                        &sources, operation,
                    )
                }
                (_, None, SessionInterfacePolicy::WithStandardPackages) => {
                    crate::analyze_sources_with_standard_packages_result(&sources)
                }
                ([(path, source)], Some(operation), SessionInterfacePolicy::WithCore) => {
                    crate::analyze_source_with_interfaces_result_with_operation(
                        path,
                        source,
                        &interfaces,
                        operation,
                    )
                }
                ([(path, source)], None, SessionInterfacePolicy::WithCore) => {
                    crate::analyze_source_with_interfaces_result(path, source, &interfaces)
                }
                (_, Some(operation), SessionInterfacePolicy::WithCore) => {
                    crate::analyze_sources_with_interfaces_result_with_operation(
                        &sources,
                        &interfaces,
                        operation,
                    )
                }
                (_, None, SessionInterfacePolicy::WithCore) => {
                    crate::analyze_sources_with_interfaces_result(&sources, &interfaces)
                }
                (_, Some(operation), SessionInterfacePolicy::WithoutCore) => {
                    crate::analyze_sources_with_interfaces_without_core_result_with_operation(
                        &sources,
                        &interfaces,
                        operation,
                    )
                }
                (_, None, SessionInterfacePolicy::WithoutCore) => {
                    crate::analyze_sources_with_interfaces_without_core_result(
                        &sources,
                        &interfaces,
                    )
                }
            },
        );
        if let Some(operation) = operation {
            operation.check()?;
        }
        self.workspace_analysis_cache_misses =
            self.workspace_analysis_cache_misses.saturating_add(1);
        self.workspace_analysis_cache = Some(Arc::clone(&analysis));
        Ok(analysis)
    }

    /// Operation-aware complete semantic analysis. Both cold and cached paths
    /// check the shared cancellation/deadline boundary before returning facts.
    pub fn workspace_analysis_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<AnalysisResult>, OperationAbort> {
        self.workspace_analysis_inner(Some(operation))
    }

    /// Convert the cached workspace analysis into the phase-gated valid
    /// program. Invalid source stays an ordinary diagnostic result; only
    /// cancellation and deadline expiry abort the query itself.
    pub fn workspace_validated_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Result<ValidatedProgram, Vec<Diagnostic>>, OperationAbort> {
        let analysis = self.workspace_analysis_with_operation(operation)?;
        Ok((*analysis).clone().into_validated())
    }

    /// Convert the current cached workspace analysis into its checked phase
    /// without introducing an unchecked analyzer entry point for callers that
    /// do not need cancellation or a deadline.
    pub fn workspace_validated(&mut self) -> Result<ValidatedProgram, Vec<Diagnostic>> {
        (*self.workspace_analysis()).clone().into_validated()
    }

    /// Diagnose the current immutable workspace through the semantic-owned
    /// frontend implementation, caching by the session's source/interface
    /// revisions. No caller can inject a competing diagnostic pipeline.
    pub fn semantic_workspace_diagnostics_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<[Diagnostic]>, OperationAbort> {
        operation.check()?;
        if let Some(diagnostics) = &self.workspace_diagnostic_cache {
            self.workspace_diagnostic_cache_hits =
                self.workspace_diagnostic_cache_hits.saturating_add(1);
            operation.check()?;
            return Ok(Arc::clone(diagnostics));
        }

        self.workspace_diagnostic_cache_misses =
            self.workspace_diagnostic_cache_misses.saturating_add(1);
        // The complete analysis query already owns the one immutable
        // source/interface snapshot and its interface policy. Projecting
        // diagnostics from it avoids a second direct analyzer route that
        // could accidentally inject Core for a `WithoutCore` session.
        let diagnostics: Arc<[Diagnostic]> = self
            .workspace_analysis_with_operation(operation)?
            .diagnostics()
            .to_vec()
            .into();
        operation.check()?;
        self.workspace_diagnostic_cache = Some(Arc::clone(&diagnostics));
        Ok(diagnostics)
    }

    /// Diagnose one source document through the session-owned semantic query
    /// cache. The cache key contains only this source revision and the
    /// interface closure selected by its parsed imports, so changing an
    /// unrelated interface cannot invalidate an editor request for this file.
    pub fn semantic_diagnostics_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let snapshot = self.source_snapshot();
        let Some(file) = snapshot.files().iter().find(|file| file.path() == path) else {
            return Ok(None);
        };
        let diagnostics = self
            .semantic_diagnostics_snapshot_file(SessionFileRole::Source, file, operation)
            .map(Some)?;
        operation.check()?;
        Ok(diagnostics)
    }

    /// Resolve, type-check, and build HIR for one source document through the
    /// session-owned dependency query. The result is keyed by the document's
    /// immutable revision plus only the interface files visible from its
    /// parsed imports, so unrelated interface edits retain this work.
    pub fn semantic_analysis_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<AnalysisResult>>, OperationAbort> {
        operation.check()?;
        let snapshot = self.source_snapshot();
        let Some(file) = snapshot.files().iter().find(|file| file.path() == path) else {
            return Ok(None);
        };
        let analysis = self
            .semantic_analysis_snapshot_file(SessionFileRole::Source, file, operation)
            .map(Some)?;
        operation.check()?;
        Ok(analysis)
    }

    /// Diagnose one interface document through the same dependency-precise
    /// semantic query cache used for source files.
    pub fn semantic_diagnostics_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let snapshot = self.interface_snapshot();
        let Some(file) = snapshot.files().iter().find(|file| file.path() == path) else {
            return Ok(None);
        };
        let diagnostics = self
            .semantic_diagnostics_snapshot_file(SessionFileRole::Interface, file, operation)
            .map(Some)?;
        operation.check()?;
        Ok(diagnostics)
    }

    /// Resolve, type-check, and build HIR for one interface document through
    /// the same dependency-precise semantic query used for source files.
    pub fn semantic_analysis_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<AnalysisResult>>, OperationAbort> {
        operation.check()?;
        let snapshot = self.interface_snapshot();
        let Some(file) = snapshot.files().iter().find(|file| file.path() == path) else {
            return Ok(None);
        };
        let analysis = self
            .semantic_analysis_snapshot_file(SessionFileRole::Interface, file, operation)
            .map(Some)?;
        operation.check()?;
        Ok(analysis)
    }

    /// Return the parsed workspace module graph for the current immutable
    /// source/interface revision set. Interface files without an explicit
    /// `module` declaration keep the historical filename fallback, so all
    /// session clients resolve the same graph.
    pub fn workspace_module_graph(&mut self) -> Arc<WorkspaceModuleGraph> {
        self.workspace_module_graph_inner(None)
            .expect("an unchecked workspace module-graph query cannot abort")
    }

    fn workspace_module_graph_inner(
        &mut self,
        operation: Option<&OperationContext>,
    ) -> Result<Arc<WorkspaceModuleGraph>, OperationAbort> {
        if let Some(operation) = operation {
            operation.check()?;
        }
        if let Some(graph) = &self.workspace_module_graph_cache {
            self.workspace_module_graph_cache_hits =
                self.workspace_module_graph_cache_hits.saturating_add(1);
            if let Some(operation) = operation {
                operation.check()?;
            }
            return Ok(Arc::clone(graph));
        }

        let mut nodes = Vec::new();
        let sources = self.source_snapshot();
        for file in sources.files() {
            if let Some(operation) = operation {
                operation.check()?;
            }
            let header = self
                .module_header_snapshot_file(SessionFileRole::Source, file)
                .expect("session source snapshot file must remain parseable");
            nodes.push(WorkspaceModuleNode {
                path: Arc::clone(&file.path),
                is_interface: false,
                modules: Arc::clone(&header.modules),
                imports: Arc::clone(&header.imports),
            });
        }
        let interfaces = self.interface_snapshot();
        for file in interfaces.files() {
            if let Some(operation) = operation {
                operation.check()?;
            }
            let header = self
                .module_header_snapshot_file(SessionFileRole::Interface, file)
                .expect("session interface snapshot file must remain parseable");
            let modules = if header.modules.is_empty() {
                interface_filename_module(file.path())
                    .into_iter()
                    .collect::<Vec<_>>()
                    .into()
            } else {
                Arc::clone(&header.modules)
            };
            nodes.push(WorkspaceModuleNode {
                path: Arc::clone(&file.path),
                is_interface: true,
                modules,
                imports: Arc::clone(&header.imports),
            });
        }
        let graph = Arc::new(WorkspaceModuleGraph {
            nodes: nodes.into(),
        });
        if let Some(operation) = operation {
            operation.check()?;
        }
        self.workspace_module_graph_cache_misses =
            self.workspace_module_graph_cache_misses.saturating_add(1);
        self.workspace_module_graph_cache = Some(Arc::clone(&graph));
        Ok(graph)
    }

    /// Operation-aware module graph query. Cached syntax facts still obey a
    /// caller's cancellation and deadline boundary before being returned.
    pub fn workspace_module_graph_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<WorkspaceModuleGraph>, OperationAbort> {
        self.workspace_module_graph_inner(Some(operation))
    }

    /// Parse one source revision exactly once. Replacing or removing a file
    /// evicts its cached syntax tree; identical writes preserve the cached
    /// query result because the revision is unchanged.
    pub fn parse_file(&mut self, path: &str) -> Option<Arc<Program>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.parse_snapshot_file(SessionFileRole::Source, file)
    }

    /// Parse a source revision while honoring the shared frontend operation
    /// boundary. The post-query check is deliberate: a cached result must not
    /// escape after a caller has cancelled or timed out the request.
    pub fn parse_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<Program>>, OperationAbort> {
        operation.check()?;
        let program = self.parse_file(path);
        operation.check()?;
        Ok(program)
    }

    /// Return syntax-only diagnostics for one source-file revision. This is a
    /// session-owned query for formatter/editor preflight: it shares stable
    /// file identity and invalidation with parse/HIR queries, but deliberately
    /// does not pull in workspace resolution or builtin interfaces.
    pub fn syntax_diagnostics_file(&mut self, path: &str) -> Option<Arc<[Diagnostic]>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.syntax_diagnostics_snapshot_file(SessionFileRole::Source, file)
    }

    /// Operation-aware syntax-diagnostics query. A cached result cannot escape
    /// after cancellation or deadline expiry, matching the other session
    /// query boundaries.
    pub fn syntax_diagnostics_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let diagnostics = self.syntax_diagnostics_file(path);
        operation.check()?;
        Ok(diagnostics)
    }

    /// Return syntax-only diagnostics for one interface-file revision.
    pub fn syntax_diagnostics_interface(&mut self, path: &str) -> Option<Arc<[Diagnostic]>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.syntax_diagnostics_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Interface syntax diagnostics obey the same operation boundary as every
    /// other session-owned query. This matters for editor requests that only
    /// touch an interface document and would otherwise bypass cancellation on
    /// a cached result.
    pub fn syntax_diagnostics_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let diagnostics = self.syntax_diagnostics_interface(path);
        operation.check()?;
        Ok(diagnostics)
    }

    /// Format one source revision through the session-owned editor cache.
    /// Formatting is syntax-only, but its result still belongs to the same
    /// immutable file identity as parse, HIR, lint, and symbols so editor
    /// clients cannot retain a competing revision cache.
    pub fn format_file(&mut self, path: &str) -> Option<Arc<str>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.format_snapshot_file(SessionFileRole::Source, file)
    }

    /// Format one interface revision through the same role-separated cache.
    pub fn format_interface(&mut self, path: &str) -> Option<Arc<str>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.format_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Format one source revision while observing cancellation and deadline
    /// both before a cache lookup and before returning a cached value.
    pub fn format_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<str>>, OperationAbort> {
        operation.check()?;
        let formatted = self.format_file(path);
        operation.check()?;
        Ok(formatted)
    }

    /// Operation-aware interface formatting with the same cache boundary as
    /// source formatting.
    pub fn format_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<str>>, OperationAbort> {
        operation.check()?;
        let formatted = self.format_interface(path);
        operation.check()?;
        Ok(formatted)
    }

    /// Return lint diagnostics for one source revision. Unlike complete
    /// workspace diagnostics this intentionally stays local and syntax-only,
    /// which keeps formatter/editor preflight cheap without creating a second
    /// cache in the language service.
    pub fn lint_file(&mut self, path: &str) -> Option<Arc<[Diagnostic]>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.lint_snapshot_file(SessionFileRole::Source, file)
    }

    /// Interface linting uses the same session cache and stable identity as
    /// source linting.
    pub fn lint_interface(&mut self, path: &str) -> Option<Arc<[Diagnostic]>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.lint_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Operation-aware source lint query.
    pub fn lint_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let diagnostics = self.lint_file(path);
        operation.check()?;
        Ok(diagnostics)
    }

    /// Operation-aware interface lint query.
    pub fn lint_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let diagnostics = self.lint_interface(path);
        operation.check()?;
        Ok(diagnostics)
    }

    /// Return syntax-derived symbols for one source revision from the shared
    /// session. Semantic consumers can layer richer facts over this stable
    /// document query without reparsing the text.
    pub fn symbol_index_file(&mut self, path: &str) -> Option<Arc<SymbolIndex>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.symbol_index_snapshot_file(SessionFileRole::Source, file)
    }

    pub fn symbol_index_interface(&mut self, path: &str) -> Option<Arc<SymbolIndex>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.symbol_index_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Operation-aware source symbol-index query.
    pub fn symbol_index_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<SymbolIndex>>, OperationAbort> {
        operation.check()?;
        let symbols = self.symbol_index_file(path);
        operation.check()?;
        Ok(symbols)
    }

    /// Operation-aware interface symbol-index query.
    pub fn symbol_index_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<SymbolIndex>>, OperationAbort> {
        operation.check()?;
        let symbols = self.symbol_index_interface(path);
        operation.check()?;
        Ok(symbols)
    }

    /// Return document symbols for one source revision from the session-owned
    /// parse-derived cache.
    pub fn document_symbols_file(&mut self, path: &str) -> Option<Arc<[RssDocumentSymbol]>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.document_symbols_snapshot_file(SessionFileRole::Source, file)
    }

    pub fn document_symbols_interface(&mut self, path: &str) -> Option<Arc<[RssDocumentSymbol]>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.document_symbols_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Operation-aware source document-symbol query.
    pub fn document_symbols_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[RssDocumentSymbol]>>, OperationAbort> {
        operation.check()?;
        let symbols = self.document_symbols_file(path);
        operation.check()?;
        Ok(symbols)
    }

    /// Operation-aware interface document-symbol query.
    pub fn document_symbols_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[RssDocumentSymbol]>>, OperationAbort> {
        operation.check()?;
        let symbols = self.document_symbols_interface(path);
        operation.check()?;
        Ok(symbols)
    }

    /// Parse one interface revision through the same cache. File IDs are
    /// session-local across both stores, so the query key also records its role.
    pub fn parse_interface(&mut self, path: &str) -> Option<Arc<Program>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.parse_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Interface parsing uses the same cancellation/deadline contract as
    /// source parsing; callers cannot accidentally bypass it through the
    /// separate interface store.
    pub fn parse_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<Program>>, OperationAbort> {
        operation.check()?;
        let program = self.parse_interface(path);
        operation.check()?;
        Ok(program)
    }

    /// Return source parse trees in canonical snapshot path order.
    pub fn parsed_sources(&mut self) -> Vec<Arc<Program>> {
        self.source_snapshot()
            .files()
            .iter()
            .filter_map(|file| self.parse_snapshot_file(SessionFileRole::Source, file))
            .collect()
    }

    /// Return canonical source parse trees while polling the shared operation
    /// context between individual file queries.
    pub fn parsed_sources_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Vec<Arc<Program>>, OperationAbort> {
        let paths = self
            .source_snapshot()
            .files()
            .iter()
            .map(|file| file.path().to_string())
            .collect::<Vec<_>>();
        let mut programs = Vec::with_capacity(paths.len());
        for path in paths {
            if let Some(program) = self.parse_file_with_operation(&path, operation)? {
                programs.push(program);
            }
        }
        Ok(programs)
    }

    /// Build the source-shaped HIR for one immutable source revision exactly
    /// once. Interface-aware workspace HIR remains a higher-level query, but
    /// this per-file cache gives editor and incremental callers a stable local
    /// semantic fact boundary without re-parsing unchanged text.
    pub fn hir_file(&mut self, path: &str) -> Option<Arc<Hir>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.hir_snapshot_file(SessionFileRole::Source, file)
    }

    /// Interface HIR uses a role-separated key because source and interface
    /// stores allocate file IDs independently.
    pub fn hir_interface(&mut self, path: &str) -> Option<Arc<Hir>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.hir_snapshot_file(SessionFileRole::Interface, file)
    }

    pub fn hir_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<Hir>>, OperationAbort> {
        operation.check()?;
        let hir = self.hir_file(path);
        operation.check()?;
        Ok(hir)
    }

    pub fn hir_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<Hir>>, OperationAbort> {
        operation.check()?;
        let hir = self.hir_interface(path);
        operation.check()?;
        Ok(hir)
    }

    /// Return the checked, namespace-isolated workspace HIR for the current
    /// immutable source/interface revisions.
    ///
    /// The HIR is projected from the session's complete analysis query rather
    /// than independently merging parse trees. That makes resolve, type, HIR,
    /// and diagnostics facts originate from one immutable revision set and
    /// prevents editor consumers from accidentally observing a second,
    /// structurally-similar but semantically different workspace HIR.
    pub fn workspace_hir(&mut self) -> Arc<Hir> {
        self.workspace_hir_inner(None)
            .expect("an unchecked workspace HIR query cannot abort")
    }

    fn workspace_hir_inner(
        &mut self,
        operation: Option<&OperationContext>,
    ) -> Result<Arc<Hir>, OperationAbort> {
        if let Some(operation) = operation {
            operation.check()?;
        }
        if let Some(hir) = &self.workspace_hir_cache {
            self.workspace_hir_cache_hits = self.workspace_hir_cache_hits.saturating_add(1);
            if let Some(operation) = operation {
                operation.check()?;
            }
            return Ok(Arc::clone(hir));
        }

        let analysis = match operation {
            Some(operation) => self.workspace_analysis_with_operation(operation)?,
            None => self.workspace_analysis(),
        };
        let hir = Arc::new(analysis.database().hir().clone());
        if let Some(operation) = operation {
            operation.check()?;
        }
        self.workspace_hir_cache_misses = self.workspace_hir_cache_misses.saturating_add(1);
        self.workspace_hir_cache = Some(Arc::clone(&hir));
        Ok(hir)
    }

    /// Operation-aware workspace-HIR query. Cached HIR cannot escape a
    /// cancelled or expired request boundary.
    pub fn workspace_hir_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<Hir>, OperationAbort> {
        self.workspace_hir_inner(Some(operation))
    }

    /// Return structural type facts for the current namespace-isolated
    /// workspace HIR. The facts share the HIR's immutable type arena, so the
    /// session never reparses or re-interns types for a second consumer.
    ///
    /// Full resolve/type diagnostics are still migrating into semantics. This
    /// query is the stable cache boundary for already-resolved declaration and
    /// signature facts, not a replacement for those remaining checks.
    pub fn workspace_type_facts(&mut self) -> Arc<SemanticTypeFacts> {
        self.workspace_type_facts_inner(None)
            .expect("an unchecked workspace type-fact query cannot abort")
    }

    fn workspace_type_facts_inner(
        &mut self,
        operation: Option<&OperationContext>,
    ) -> Result<Arc<SemanticTypeFacts>, OperationAbort> {
        if let Some(operation) = operation {
            operation.check()?;
        }
        if let Some(types) = &self.workspace_type_cache {
            self.workspace_type_cache_hits = self.workspace_type_cache_hits.saturating_add(1);
            if let Some(operation) = operation {
                operation.check()?;
            }
            return Ok(Arc::clone(types));
        }

        let hir = match operation {
            Some(operation) => self.workspace_hir_with_operation(operation)?,
            None => self.workspace_hir(),
        };
        let types = hir.semantic_types_arc();
        if let Some(operation) = operation {
            operation.check()?;
        }
        self.workspace_type_cache_misses = self.workspace_type_cache_misses.saturating_add(1);
        self.workspace_type_cache = Some(Arc::clone(&types));
        Ok(types)
    }

    /// Operation-aware type-fact query. Cached facts must not escape a
    /// cancellation or deadline boundary after they have been obtained.
    pub fn workspace_type_facts_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<SemanticTypeFacts>, OperationAbort> {
        self.workspace_type_facts_inner(Some(operation))
    }

    /// Return parsed module and import paths for one source revision.
    pub fn module_header(&mut self, path: &str) -> Option<Arc<ModuleHeader>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.module_header_snapshot_file(SessionFileRole::Source, file)
    }

    /// Query one source module header while observing the shared operation
    /// boundary. Cached syntax facts must not escape after cancellation or a
    /// deadline, just like parse and HIR query results.
    pub fn module_header_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<ModuleHeader>>, OperationAbort> {
        operation.check()?;
        let header = self.module_header(path);
        operation.check()?;
        Ok(header)
    }

    /// Return parsed module and import paths for one interface revision.
    pub fn interface_module_header(&mut self, path: &str) -> Option<Arc<ModuleHeader>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.module_header_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Interface headers use the same cancellation/deadline contract as
    /// source headers, so editor clients cannot bypass it through the separate
    /// interface store.
    pub fn interface_module_header_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<ModuleHeader>>, OperationAbort> {
        operation.check()?;
        let header = self.interface_module_header(path);
        operation.check()?;
        Ok(header)
    }

    pub fn stats(&self) -> CompilationSessionStats {
        CompilationSessionStats {
            parse_cache_hits: self.parse_cache_hits,
            parse_cache_misses: self.parse_cache_misses,
            hir_cache_hits: self.hir_cache_hits,
            hir_cache_misses: self.hir_cache_misses,
            workspace_hir_cache_hits: self.workspace_hir_cache_hits,
            workspace_hir_cache_misses: self.workspace_hir_cache_misses,
            workspace_type_cache_hits: self.workspace_type_cache_hits,
            workspace_type_cache_misses: self.workspace_type_cache_misses,
            workspace_analysis_cache_hits: self.workspace_analysis_cache_hits,
            workspace_analysis_cache_misses: self.workspace_analysis_cache_misses,
            lint_cache_hits: self.lint_cache_hits,
            lint_cache_misses: self.lint_cache_misses,
            format_cache_hits: self.format_cache_hits,
            format_cache_misses: self.format_cache_misses,
            symbol_cache_hits: self.symbol_cache_hits,
            symbol_cache_misses: self.symbol_cache_misses,
            document_symbol_cache_hits: self.document_symbol_cache_hits,
            document_symbol_cache_misses: self.document_symbol_cache_misses,
            module_header_cache_hits: self.module_header_cache_hits,
            module_header_cache_misses: self.module_header_cache_misses,
            workspace_module_graph_cache_hits: self.workspace_module_graph_cache_hits,
            workspace_module_graph_cache_misses: self.workspace_module_graph_cache_misses,
            workspace_diagnostic_cache_hits: self.workspace_diagnostic_cache_hits,
            workspace_diagnostic_cache_misses: self.workspace_diagnostic_cache_misses,
            semantic_document_analysis_cache_hits: self.semantic_document_analysis_cache_hits,
            semantic_document_analysis_cache_misses: self.semantic_document_analysis_cache_misses,
            semantic_document_diagnostic_cache_hits: self.semantic_document_diagnostic_cache_hits,
            semantic_document_diagnostic_cache_misses: self
                .semantic_document_diagnostic_cache_misses,
        }
    }

    fn parse_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<Program>> {
        // Source and interface stores allocate FileId independently, so role
        // is part of the private cache key.
        let key = (role, file.file_id(), file.revision());
        if let Some(program) = self.parse_cache.get(&key) {
            self.parse_cache_hits = self.parse_cache_hits.saturating_add(1);
            return Some(Arc::clone(program));
        }
        let program = Arc::new(parse_source(file.path(), file.text()));
        self.parse_cache.insert(key, Arc::clone(&program));
        self.parse_cache_misses = self.parse_cache_misses.saturating_add(1);
        Some(program)
    }

    fn invalidate_parse_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.parse_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    fn hir_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<Hir>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(hir) = self.hir_cache.get(&key) {
            self.hir_cache_hits = self.hir_cache_hits.saturating_add(1);
            return Some(Arc::clone(hir));
        }
        let program = self.parse_snapshot_file(role, file)?;
        let hir = Arc::new(Hir::from_syntax(&program));
        self.hir_cache.insert(key, Arc::clone(&hir));
        self.hir_cache_misses = self.hir_cache_misses.saturating_add(1);
        Some(hir)
    }

    fn invalidate_hir_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.hir_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    fn invalidate_syntax_diagnostic_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.syntax_diagnostic_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    /// Evict every syntax-derived editor query for one changed/deleted file.
    /// The language service deliberately does not mirror these maps: file ID,
    /// role, and revision remain the single cache key owned by the session.
    fn invalidate_editor_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.lint_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
        self.format_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
        self.symbol_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
        self.document_symbol_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    fn invalidate_semantic_document_cache_for_source(&mut self, file_id: FileId) {
        let retains_source = |key: &SemanticDocumentCacheKey| {
            !(key.role == SessionFileRole::Source && key.file_id == file_id
                || key
                    .visible_sources
                    .iter()
                    .any(|(dependency_id, _)| *dependency_id == file_id))
        };
        self.semantic_document_analysis_cache
            .retain(|key, _| retains_source(key));
        self.semantic_document_diagnostic_cache
            .retain(|key, _| retains_source(key));
    }

    fn invalidate_semantic_document_cache_for_interface(&mut self, file_id: FileId) {
        self.semantic_document_analysis_cache.retain(|key, _| {
            !(key.role == SessionFileRole::Interface && key.file_id == file_id
                || key
                    .visible_interfaces
                    .iter()
                    .any(|(dependency_id, _)| *dependency_id == file_id))
        });
        self.semantic_document_diagnostic_cache.retain(|key, _| {
            !(key.role == SessionFileRole::Interface && key.file_id == file_id
                || key
                    .visible_interfaces
                    .iter()
                    .any(|(dependency_id, _)| *dependency_id == file_id))
        });
    }

    fn semantic_diagnostics_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
        operation: &OperationContext,
    ) -> Result<Arc<[Diagnostic]>, OperationAbort> {
        let input = self.semantic_document_input(role, file, operation)?;
        if let Some(diagnostics) = self.semantic_document_diagnostic_cache.get(&input.key) {
            self.semantic_document_diagnostic_cache_hits = self
                .semantic_document_diagnostic_cache_hits
                .saturating_add(1);
            operation.check()?;
            return Ok(Arc::clone(diagnostics));
        }

        let diagnostics: Arc<[Diagnostic]> = self
            .semantic_analysis_input(&input, operation)?
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.span.file == input.path.as_ref())
            .cloned()
            .collect::<Vec<_>>()
            .into();
        operation.check()?;
        self.semantic_document_diagnostic_cache_misses = self
            .semantic_document_diagnostic_cache_misses
            .saturating_add(1);
        self.semantic_document_diagnostic_cache
            .insert(input.key, Arc::clone(&diagnostics));
        Ok(diagnostics)
    }

    fn semantic_analysis_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
        operation: &OperationContext,
    ) -> Result<Arc<AnalysisResult>, OperationAbort> {
        let input = self.semantic_document_input(role, file, operation)?;
        self.semantic_analysis_input(&input, operation)
    }

    fn semantic_document_input(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
        operation: &OperationContext,
    ) -> Result<SemanticDocumentInput, OperationAbort> {
        operation.check()?;
        let graph = self.workspace_module_graph_with_operation(operation)?;
        let root_imports = match role {
            SessionFileRole::Source => graph.source(file.path()),
            SessionFileRole::Interface => graph.interface(file.path()),
        }
        .map(|node| node.imports().to_vec())
        .unwrap_or_default();
        let visible_paths = graph.visible_interface_paths(file.path(), root_imports);
        let source_paths = if role == SessionFileRole::Source {
            let root_imports = graph
                .source(file.path())
                .map(|node| node.imports().to_vec())
                .unwrap_or_default();
            graph.visible_source_paths(file.path(), root_imports)
        } else {
            BTreeSet::new()
        };
        let sources = self
            .source_snapshot()
            .files()
            .iter()
            .filter(|candidate| candidate.path() != file.path())
            .filter(|candidate| source_paths.contains(candidate.path()))
            .cloned()
            .collect::<Vec<_>>();
        let interfaces = self
            .interface_snapshot()
            .files()
            .iter()
            .filter(|candidate| candidate.path() != file.path())
            .filter(|candidate| visible_paths.contains(candidate.path()))
            .cloned()
            .collect::<Vec<_>>();
        Ok(SemanticDocumentInput {
            key: SemanticDocumentCacheKey {
                role,
                file_id: file.file_id(),
                revision: file.revision(),
                visible_sources: sources
                    .iter()
                    .map(|dependency| (dependency.file_id(), dependency.revision()))
                    .collect(),
                visible_interfaces: interfaces
                    .iter()
                    .map(|dependency| (dependency.file_id(), dependency.revision()))
                    .collect(),
            },
            path: Arc::clone(&file.path),
            text: Arc::clone(&file.text),
            sources,
            interfaces,
        })
    }

    fn semantic_analysis_input(
        &mut self,
        input: &SemanticDocumentInput,
        operation: &OperationContext,
    ) -> Result<Arc<AnalysisResult>, OperationAbort> {
        operation.check()?;
        if let Some(analysis) = self.semantic_document_analysis_cache.get(&input.key) {
            self.semantic_document_analysis_cache_hits =
                self.semantic_document_analysis_cache_hits.saturating_add(1);
            operation.check()?;
            return Ok(Arc::clone(analysis));
        }
        let interface_slices = input
            .interfaces
            .iter()
            .map(|dependency| (dependency.path(), dependency.text()))
            .collect::<Vec<_>>();
        let analysis = if input.sources.is_empty()
            && self.interface_policy == SessionInterfacePolicy::WithCore
        {
            Arc::new(crate::analyze_source_with_interfaces_result_with_operation(
                input.path.as_ref(),
                input.text.as_ref(),
                &interface_slices,
                operation,
            ))
        } else {
            let mut source_slices = Vec::with_capacity(input.sources.len().saturating_add(1));
            source_slices.push((input.path.as_ref(), input.text.as_ref()));
            source_slices.extend(
                input
                    .sources
                    .iter()
                    .map(|dependency| (dependency.path(), dependency.text())),
            );
            Arc::new(match self.interface_policy {
                SessionInterfacePolicy::WithCore => {
                    crate::analyze_sources_with_interfaces_result_with_operation(
                        &source_slices,
                        &interface_slices,
                        operation,
                    )
                }
                SessionInterfacePolicy::WithoutCore => {
                    crate::analyze_sources_with_interfaces_without_core_result_with_operation(
                        &source_slices,
                        &interface_slices,
                        operation,
                    )
                }
                SessionInterfacePolicy::WithStandardPackages => {
                    crate::analyze_sources_with_standard_packages_result_with_operation(
                        &source_slices,
                        operation,
                    )
                }
            })
        };
        operation.check()?;
        self.semantic_document_analysis_cache_misses = self
            .semantic_document_analysis_cache_misses
            .saturating_add(1);
        self.semantic_document_analysis_cache
            .insert(input.key.clone(), Arc::clone(&analysis));
        Ok(analysis)
    }

    fn syntax_diagnostics_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<[Diagnostic]>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(diagnostics) = self.syntax_diagnostic_cache.get(&key) {
            return Some(Arc::clone(diagnostics));
        }
        let diagnostics = Arc::from(crate::analyze_syntax_source(file.path(), file.text()));
        self.syntax_diagnostic_cache
            .insert(key, Arc::clone(&diagnostics));
        Some(diagnostics)
    }

    fn lint_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<[Diagnostic]>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(diagnostics) = self.lint_cache.get(&key) {
            self.lint_cache_hits = self.lint_cache_hits.saturating_add(1);
            return Some(Arc::clone(diagnostics));
        }
        let diagnostics = Arc::from(lint_source(file.path(), file.text()));
        self.lint_cache.insert(key, Arc::clone(&diagnostics));
        self.lint_cache_misses = self.lint_cache_misses.saturating_add(1);
        Some(diagnostics)
    }

    fn format_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<str>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(formatted) = self.format_cache.get(&key) {
            self.format_cache_hits = self.format_cache_hits.saturating_add(1);
            return Some(Arc::clone(formatted));
        }
        let formatted: Arc<str> = format_source(file.path(), file.text()).into();
        self.format_cache.insert(key, Arc::clone(&formatted));
        self.format_cache_misses = self.format_cache_misses.saturating_add(1);
        Some(formatted)
    }

    fn symbol_index_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<SymbolIndex>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(symbols) = self.symbol_cache.get(&key) {
            self.symbol_cache_hits = self.symbol_cache_hits.saturating_add(1);
            return Some(Arc::clone(symbols));
        }
        let program = self.parse_snapshot_file(role, file)?;
        let symbols = Arc::new(symbol_index_from_program(file.text(), &program));
        self.symbol_cache.insert(key, Arc::clone(&symbols));
        self.symbol_cache_misses = self.symbol_cache_misses.saturating_add(1);
        Some(symbols)
    }

    fn document_symbols_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<[RssDocumentSymbol]>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(symbols) = self.document_symbol_cache.get(&key) {
            self.document_symbol_cache_hits = self.document_symbol_cache_hits.saturating_add(1);
            return Some(Arc::clone(symbols));
        }
        let program = self.parse_snapshot_file(role, file)?;
        let symbols: Arc<[RssDocumentSymbol]> = document_symbols_from_program(&program).into();
        self.document_symbol_cache.insert(key, Arc::clone(&symbols));
        self.document_symbol_cache_misses = self.document_symbol_cache_misses.saturating_add(1);
        Some(symbols)
    }

    fn module_header_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<ModuleHeader>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(header) = self.module_header_cache.get(&key) {
            self.module_header_cache_hits = self.module_header_cache_hits.saturating_add(1);
            return Some(Arc::clone(header));
        }
        let program = self.parse_snapshot_file(role, file)?;
        let header = Arc::new(module_header_from_program(&program));
        self.module_header_cache.insert(key, Arc::clone(&header));
        self.module_header_cache_misses = self.module_header_cache_misses.saturating_add(1);
        Some(header)
    }

    fn invalidate_module_header_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.module_header_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    /// Return the current header only when a workspace graph is already cached.
    ///
    /// The graph contains exactly the path, role, module declarations and import
    /// declarations represented by this header. Recomputing a graph after an
    /// implementation-only edit therefore wastes a whole-workspace query and,
    /// for editor clients, causes avoidable dependency churn. We intentionally
    /// keep the comparison local to this syntax query: type/HIR/diagnostic
    /// caches still invalidate on every changed byte until their dependency
    /// edges are independently precise.
    fn cached_module_header(
        &mut self,
        role: SessionFileRole,
        path: &str,
    ) -> Option<Arc<ModuleHeader>> {
        self.workspace_module_graph_cache.as_ref()?;
        match role {
            SessionFileRole::Source => self.module_header(path),
            SessionFileRole::Interface => self.interface_module_header(path),
        }
    }

    fn module_header_is_unchanged(
        &mut self,
        role: SessionFileRole,
        path: &str,
        previous: Option<&ModuleHeader>,
    ) -> bool {
        let Some(previous) = previous else {
            return false;
        };
        let current = match role {
            SessionFileRole::Source => self.module_header(path),
            SessionFileRole::Interface => self.interface_module_header(path),
        };
        current.as_deref() == Some(previous)
    }
}
