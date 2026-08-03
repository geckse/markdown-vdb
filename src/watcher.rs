use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify_debouncer_full::notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify_debouncer_full::notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::discovery::FileDiscovery;
use crate::embedding::provider::EmbeddingProvider;
use crate::error::{Error, Result};
use crate::fts::{FtsChunkData, FtsIndex};
use crate::index::state::Index;
use crate::modules::{ModuleEvent, ModuleReport, ModuleRunner};

use serde::Serialize;

const SCHEMA_OVERLAY_PATH: &str = ".markdownvdb.schema.yml";

/// Type of watch event for reporting.
#[derive(Debug, Clone, Serialize)]
pub enum WatchEventType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

/// Report generated after processing a single watch event.
#[derive(Debug, Clone, Serialize)]
pub struct WatchEventReport {
    /// The type of filesystem event.
    pub event_type: WatchEventType,
    /// Relative path of the affected file.
    pub path: String,
    /// Previous relative path for a rename event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    /// Number of chunks processed (0 for deletions or skipped files).
    pub chunks_processed: usize,
    /// Provider-independent local count of inputs successfully embedded.
    pub estimated_input_tokens: usize,
    /// Number of embedding API calls made for this event.
    pub api_calls: usize,
    /// Duration in milliseconds to process this event.
    pub duration_ms: u64,
    /// Whether the event was processed successfully.
    pub success: bool,
    /// Error message, if processing failed.
    pub error: Option<String>,
    /// Reports from always-on derived-data modules executed for this event.
    pub module_reports: Vec<ModuleReport>,
}

/// Callback invoked after each watch event is processed.
pub type WatchEventCallback = Box<dyn Fn(&WatchEventReport) + Send + Sync>;

/// A filesystem event relevant to the index.
#[derive(Debug, Clone)]
pub enum FileEvent {
    /// A new markdown file was created.
    Created(PathBuf),
    /// An existing markdown file was modified.
    Modified(PathBuf),
    /// A markdown file was deleted.
    Deleted(PathBuf),
    /// A markdown file was renamed from one path to another.
    Renamed { from: PathBuf, to: PathBuf },
    /// The project schema overlay was created, changed, replaced, or removed.
    SchemaChanged(PathBuf),
}

#[derive(Debug)]
struct EventOutcome {
    chunks_processed: usize,
    estimated_input_tokens: usize,
    api_calls: usize,
    module_reports: Vec<ModuleReport>,
}

/// Watches configured source directories for markdown file changes and
/// triggers incremental re-indexing.
pub struct Watcher {
    config: Config,
    project_root: PathBuf,
    index: Arc<Index>,
    fts_index: Arc<FtsIndex>,
    provider: Arc<dyn EmbeddingProvider>,
    #[allow(dead_code)]
    discovery: FileDiscovery,
    event_callback: Option<WatchEventCallback>,
}

impl Watcher {
    /// Create a new `Watcher`.
    pub fn new(
        config: Config,
        project_root: &Path,
        index: Arc<Index>,
        fts_index: Arc<FtsIndex>,
        provider: Arc<dyn EmbeddingProvider>,
        event_callback: Option<WatchEventCallback>,
    ) -> Self {
        let discovery = FileDiscovery::new(project_root, &config);
        Self {
            config,
            project_root: project_root.to_path_buf(),
            index,
            fts_index,
            provider,
            discovery,
            event_callback,
        }
    }

    /// Start watching source directories for changes.
    ///
    /// This method blocks until the `cancel` token is triggered. Events are
    /// debounced according to `config.watch_debounce_ms` and processed
    /// incrementally.
    pub async fn watch(&self, cancel: CancellationToken) -> Result<()> {
        let (tx, mut rx) = mpsc::unbounded_channel::<FileEvent>();

        let debounce_duration = Duration::from_millis(self.config.watch_debounce_ms);
        let project_root = self.project_root.clone();

        // Build a FileDiscovery for the sync callback thread.
        let cb_discovery = FileDiscovery::new(&self.project_root, &self.config);
        let cb_source_dirs = self.config.source_dirs.clone();

        let mut debouncer = new_debouncer(
            debounce_duration,
            None,
            move |result: DebounceEventResult| {
                let events = match result {
                    Ok(events) => events,
                    Err(errs) => {
                        for e in errs {
                            error!("debouncer error: {e}");
                        }
                        return;
                    }
                };

                for event in events {
                    let file_events = classify_event(
                        &event.event.kind,
                        &event.paths,
                        &project_root,
                        &cb_discovery,
                    );
                    for fe in file_events
                        .into_iter()
                        .filter(|event| event_is_in_sources(event, &cb_source_dirs))
                    {
                        if tx.send(fe).is_err() {
                            debug!("watcher channel closed, stopping event forwarding");
                            return;
                        }
                    }
                }
            },
        )
        .map_err(|e| Error::Watch(format!("failed to create debouncer: {e}")))?;

        // Watch each configured source directory. Track whether the project
        // root itself is already covered recursively so we can watch the
        // root-level schema overlay without registering a duplicate watch.
        let mut project_root_watched_recursively = false;
        let canonical_project_root = self.project_root.canonicalize().ok();
        for source_dir in &self.config.source_dirs {
            let abs_dir = self.project_root.join(source_dir);
            if !abs_dir.is_dir() {
                debug!("skipping non-existent source dir: {}", abs_dir.display());
                continue;
            }
            project_root_watched_recursively |= abs_dir
                .canonicalize()
                .ok()
                .is_some_and(|path| canonical_project_root.as_ref() == Some(&path));
            debouncer
                .watch(&abs_dir, RecursiveMode::Recursive)
                .map_err(|e| Error::Watch(format!("failed to watch {}: {e}", abs_dir.display())))?;
            info!("watching directory: {}", abs_dir.display());
        }

        if !project_root_watched_recursively {
            debouncer
                .watch(&self.project_root, RecursiveMode::NonRecursive)
                .map_err(|e| {
                    Error::Watch(format!(
                        "failed to watch schema overlay in {}: {e}",
                        self.project_root.display()
                    ))
                })?;
            info!(
                "watching schema overlay directory: {}",
                self.project_root.display()
            );
        }

        info!(
            "file watcher started, debounce={}ms",
            self.config.watch_debounce_ms
        );

        // Process events until cancellation.
        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    if let Err(e) = self.handle_event(&event).await {
                        error!("error handling event {:?}: {e}", event);
                    }
                }
                _ = cancel.cancelled() => {
                    info!("file watcher shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Process a single file event.
    pub async fn handle_event(&self, event: &FileEvent) -> Result<()> {
        let start = Instant::now();
        let (event_type, path_str) = match event {
            FileEvent::Created(p) => (WatchEventType::Created, crate::path_util::to_slash(p)),
            FileEvent::Modified(p) => (WatchEventType::Modified, crate::path_util::to_slash(p)),
            FileEvent::Deleted(p) => (WatchEventType::Deleted, crate::path_util::to_slash(p)),
            FileEvent::Renamed { to, .. } => {
                (WatchEventType::Renamed, crate::path_util::to_slash(to))
            }
            FileEvent::SchemaChanged(p) => {
                (WatchEventType::Modified, crate::path_util::to_slash(p))
            }
        };
        let previous_path = match event {
            FileEvent::Renamed { from, .. } => Some(crate::path_util::to_slash(from)),
            _ => None,
        };

        // Keep raw index changes, dependency evaluation, materialization, and
        // the final index save in one project-scoped critical section. Reload
        // after waiting so this watcher never mutates an obsolete generation.
        let result = match crate::modules::acquire_module_run_lock(&self.project_root) {
            Ok(module_run_lock) => {
                if let Err(error) = self.index.reload_from_disk_if_clean() {
                    Err(error)
                } else {
                    match crate::fts::recover_if_required(
                        &self.project_root,
                        &self.index,
                        &self.fts_index,
                    ) {
                        Err(error) => Err(error),
                        Ok(_) => match crate::fts::begin_reconciliation(&self.project_root) {
                            Err(error) => Err(error),
                            Ok(()) => {
                                match self.handle_event_inner(event, &module_run_lock).await {
                                    Ok(outcome) => {
                                        crate::fts::finish_reconciliation(&self.project_root)
                                            .map(|()| outcome)
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                        },
                    }
                }
            }
            Err(error) => Err(error),
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        let (success, error, chunks_processed, estimated_input_tokens, api_calls, module_reports) =
            match &result {
                Ok(outcome) => (
                    true,
                    None,
                    outcome.chunks_processed,
                    outcome.estimated_input_tokens,
                    outcome.api_calls,
                    outcome.module_reports.clone(),
                ),
                Err(e) => (false, Some(e.to_string()), 0, 0, 0, Vec::new()),
            };

        if let Some(ref cb) = self.event_callback {
            cb(&WatchEventReport {
                event_type,
                path: path_str,
                previous_path,
                chunks_processed,
                estimated_input_tokens,
                api_calls,
                duration_ms,
                success,
                error,
                module_reports,
            });
        }

        result.map(|_| ())
    }

    /// Inner implementation of event handling.
    async fn handle_event_inner(
        &self,
        event: &FileEvent,
        module_run_lock: &crate::modules::ModuleRunLock,
    ) -> Result<EventOutcome> {
        match event {
            FileEvent::Created(path) | FileEvent::Modified(path) => {
                debug!(path = %path.display(), "processing created/modified event");
                let relative = crate::path_util::to_slash(path);
                let module_event = ModuleEvent::FilesChanged {
                    upserted: vec![relative],
                    removed: Vec::new(),
                    renamed: Vec::new(),
                };
                self.process_file(path, module_event, module_run_lock).await
            }
            FileEvent::Deleted(path) => {
                let relative = crate::path_util::to_slash(path);

                // Atomic replace writes may surface as a removal of the old
                // inode followed by a create/rename for the same path. By the
                // time the debounced event is handled the final file already
                // exists, so reconcile it as an upsert instead of briefly
                // deleting its vectors (and potentially re-embedding it on the
                // following create event).
                if self.project_root.join(path).is_file() {
                    debug!(path = %relative, "delete event target still exists, reconciling replacement");
                    return self
                        .process_file(
                            path,
                            ModuleEvent::FilesChanged {
                                upserted: vec![relative],
                                removed: Vec::new(),
                                renamed: Vec::new(),
                            },
                            module_run_lock,
                        )
                        .await;
                }

                info!(path = %relative, "removing deleted file from index");
                self.index.remove_file(&relative)?;

                // Update link graph: remove links from deleted file.
                let mut graph =
                    self.index
                        .get_link_graph()
                        .unwrap_or_else(|| crate::links::LinkGraph {
                            forward: std::collections::HashMap::new(),
                            last_updated: 0,
                            semantic_edges: None,
                            edge_cluster_state: None,
                        });
                crate::links::remove_file_links(&mut graph, &relative);
                self.index.update_link_graph(Some(graph));

                self.remove_from_clusters(&relative);
                self.fts_index.remove_file(&relative)?;
                self.refresh_schemas();
                let module_reports = self.run_modules(
                    &ModuleEvent::FilesChanged {
                        upserted: Vec::new(),
                        removed: vec![relative],
                        renamed: Vec::new(),
                    },
                    module_run_lock,
                )?;
                self.fts_index.commit()?;
                Ok(EventOutcome {
                    chunks_processed: 0,
                    estimated_input_tokens: 0,
                    api_calls: 0,
                    module_reports,
                })
            }
            FileEvent::Renamed { from, to } => {
                let from_str = crate::path_util::to_slash(from);
                let to_str = crate::path_util::to_slash(to);
                debug!(from = %from_str, to = %to.display(), "processing rename event");
                self.index.remove_file(&from_str)?;

                // Remove old path links from graph before processing new path.
                let mut graph =
                    self.index
                        .get_link_graph()
                        .unwrap_or_else(|| crate::links::LinkGraph {
                            forward: std::collections::HashMap::new(),
                            last_updated: 0,
                            semantic_edges: None,
                            edge_cluster_state: None,
                        });
                crate::links::remove_file_links(&mut graph, &from_str);
                self.index.update_link_graph(Some(graph));

                self.remove_from_clusters(&from_str);
                self.fts_index.remove_file(&from_str)?;
                let module_event = ModuleEvent::FilesChanged {
                    upserted: vec![to_str.clone()],
                    removed: vec![from_str.clone()],
                    renamed: vec![(from_str, to_str)],
                };
                self.process_file(to, module_event, module_run_lock).await
            }
            FileEvent::SchemaChanged(path) => {
                debug!(path = %path.display(), "processing schema overlay event");
                self.refresh_schemas();
                // Computed classification is overlay-driven. Revisit every
                // indexed source before module cleanup so a link-shaped value
                // that just became (or ceased to be) computed cannot leave a
                // stale relation/backlink edge behind.
                let mut indexed_paths: Vec<String> =
                    self.index.get_file_hashes().into_keys().collect();
                indexed_paths.sort();
                for indexed_path in indexed_paths {
                    if let Ok(file) = crate::parser::parse_markdown_file(
                        &self.project_root,
                        Path::new(&indexed_path),
                    ) {
                        self.update_file_links(&file);
                    }
                }
                let module_reports =
                    self.run_modules(&ModuleEvent::SchemaChanged, module_run_lock)?;
                Ok(EventOutcome {
                    chunks_processed: 0,
                    estimated_input_tokens: 0,
                    api_calls: 0,
                    module_reports,
                })
            }
        }
    }

    /// Parse, chunk, embed, and upsert a single file. Returns the number of chunks processed.
    async fn process_file(
        &self,
        relative_path: &Path,
        module_event: ModuleEvent,
        module_run_lock: &crate::modules::ModuleRunLock,
    ) -> Result<EventOutcome> {
        let abs_path = self.project_root.join(relative_path);

        // If the file no longer exists (deleted between event and processing, or the
        // OS sent a Modify event for a removal), treat it as a deletion.
        if !abs_path.is_file() {
            let relative = crate::path_util::to_slash(relative_path);
            info!(path = %relative, "file no longer exists, removing from index");
            self.index.remove_file(&relative)?;
            self.remove_from_clusters(&relative);
            self.fts_index.remove_file(&relative)?;
            self.refresh_schemas();
            let module_reports = self.run_modules(
                &ModuleEvent::FilesChanged {
                    upserted: Vec::new(),
                    removed: vec![relative],
                    renamed: Vec::new(),
                },
                module_run_lock,
            )?;
            self.fts_index.commit()?;
            return Ok(EventOutcome {
                chunks_processed: 0,
                estimated_input_tokens: 0,
                api_calls: 0,
                module_reports,
            });
        }

        let relative = crate::path_util::to_slash(relative_path);
        let stored_file = self.index.get_file(&relative);
        let file = crate::parser::parse_markdown_file(&self.project_root, relative_path)?;
        let body_hash = crate::parser::compute_content_hash(&file.body);

        if let Some(stored) = stored_file.as_ref() {
            let source_unchanged = stored.content_hash == file.content_hash;
            let embedding_unchanged = stored.embedding_body_hash == body_hash;

            // Formula writeback synchronizes the final source hash before its
            // own filesystem event can be processed. Treat that echo (and any
            // duplicate notify event) as a true no-op: running hooks or saving
            // here could produce another write and turn the echo into a loop.
            if source_unchanged && embedding_unchanged {
                debug!(path = %relative_path.display(), "source and embedding body unchanged, skipping");
                return Ok(EventOutcome {
                    chunks_processed: 0,
                    estimated_input_tokens: 0,
                    api_calls: 0,
                    module_reports: Vec::new(),
                });
            }

            // Frontmatter is source data, but it is not part of any document
            // or edge embedding input. Refresh raw metadata and derived state
            // without touching the provider, vectors, FTS, or clusters.
            if embedding_unchanged {
                debug!(path = %relative_path.display(), "frontmatter-only change, refreshing metadata");
                self.index.refresh_source_metadata(&file)?;
                self.update_file_links(&file);
                self.refresh_schemas();
                let module_reports = self.run_modules(&module_event, module_run_lock)?;
                return Ok(EventOutcome {
                    chunks_processed: 0,
                    estimated_input_tokens: 0,
                    api_calls: 0,
                    module_reports,
                });
            }
        }

        let chunks = if file.body.trim().is_empty() {
            Vec::new()
        } else {
            crate::chunker::chunk_document(
                &file,
                self.config.chunk_max_tokens,
                self.config.chunk_overlap_tokens,
            )?
        };

        // Empty-body documents still carry frontmatter and may participate in
        // formulas. Upsert them with zero chunks, without making an empty
        // provider request.
        let (embeddings, api_calls, estimated_input_tokens) = if chunks.is_empty() {
            debug!(path = %relative_path.display(), "document body produced no chunks");
            (Vec::new(), 0, 0)
        } else {
            let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
            crate::embedding::batch::embed_inputs_adaptively(self.provider.as_ref(), texts).await?
        };

        // Upsert vector index and FTS index.
        self.index.upsert(&file, &chunks, &embeddings)?;

        // Update link graph with body links + frontmatter relations from this
        // file. Always runs (not gated on the file having links) so removing a
        // file's last link also removes its stale graph entries.
        self.update_file_links(&file);

        let fts_chunks: Vec<FtsChunkData> = chunks
            .iter()
            .map(|c| FtsChunkData {
                chunk_id: c.id.clone(),
                source_path: crate::path_util::to_slash(&c.source_path),
                content: crate::fts::strip_markdown(&c.content),
                heading_hierarchy: c.heading_hierarchy.join(" > "),
            })
            .collect();
        let path_str_fts = crate::path_util::to_slash(relative_path);
        self.fts_index.upsert_chunks(&path_str_fts, &fts_chunks)?;

        // Rebuild raw global/scoped schema metadata without embedding. This
        // keeps occurrence counts correct and ensures a first file in a new
        // scope has a scoped schema before formula hooks refresh their stats.
        self.refresh_schemas();

        // Keep cluster and topic membership live under watch mode.
        self.update_clusters_for_file(&path_str_fts);

        let module_reports = self.run_modules(&module_event, module_run_lock)?;
        self.fts_index.commit()?;
        let chunk_count = chunks.len();
        info!(
            path = %relative_path.display(),
            chunks = chunk_count,
            "indexed file"
        );

        Ok(EventOutcome {
            chunks_processed: chunk_count,
            estimated_input_tokens,
            api_calls,
            module_reports,
        })
    }

    fn run_modules(
        &self,
        event: &ModuleEvent,
        module_run_lock: &crate::modules::ModuleRunLock,
    ) -> Result<Vec<ModuleReport>> {
        ModuleRunner::builtins().run_locked(
            &self.project_root,
            self.index.as_ref(),
            event,
            module_run_lock,
        )
    }

    fn update_file_links(&self, file: &crate::parser::MarkdownFile) {
        let overlay = crate::schema::Schema::load_overlay(&self.project_root).unwrap_or(None);
        let computed_owners = self
            .index
            .get_all_files()
            .into_iter()
            .filter_map(|(path, file)| {
                let fields: std::collections::HashSet<String> = file
                    .computed_fields
                    .iter()
                    .filter(|(field, entry)| file.materialized_field_matches(field, entry))
                    .map(|(field, _)| field)
                    .cloned()
                    .collect();
                (!fields.is_empty()).then_some((path, fields))
            })
            .collect();
        let relation_ctx = crate::relations::RelationContext::new(
            self.index.get_file_hashes().keys().cloned().collect(),
            overlay,
        )
        .with_computed_field_owners(computed_owners);
        let mut graph = self
            .index
            .get_link_graph()
            .unwrap_or_else(|| crate::links::LinkGraph {
                forward: std::collections::HashMap::new(),
                last_updated: 0,
                semantic_edges: None,
                edge_cluster_state: None,
            });
        crate::links::update_file_links(&mut graph, file, &relation_ctx);
        self.index.update_link_graph(Some(graph));
    }

    /// Rebuild global and scoped schema metadata from indexed Markdown without
    /// embedding or otherwise mutating source-derived index content.
    ///
    /// A malformed overlay is intentionally treated as absent here. The
    /// formula module receives the same `SchemaChanged` event immediately
    /// afterwards and owns clearing cached formula values plus persisting the
    /// parse diagnostic.
    fn refresh_schemas(&self) {
        let mut paths: Vec<String> = self.index.get_file_hashes().into_keys().collect();
        paths.sort();

        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            match crate::parser::parse_markdown_file(&self.project_root, Path::new(&path)) {
                Ok(file) => files.push(file),
                Err(error) => {
                    warn!(path = %path, error = %error, "skipping file during schema refresh");
                }
            }
        }

        let overlay = match crate::schema::Schema::load_overlay(&self.project_root) {
            Ok(overlay) => overlay,
            Err(error) => {
                warn!(error = %error, "schema overlay is malformed; clearing overlay schema metadata");
                None
            }
        };

        let inferred = crate::schema::Schema::infer(&files);
        let global_overlay = overlay.as_ref().map(|schema| schema.fields.clone());
        self.index
            .set_schema(Some(crate::schema::Schema::merge(inferred, global_overlay)));

        let mut scopes = std::collections::BTreeSet::new();
        scopes.extend(crate::schema::Schema::discover_scopes(&files));
        if let Some(overlay) = &overlay {
            scopes.extend(overlay.scopes.keys().cloned());
        }

        let scoped_schemas: Vec<_> = scopes
            .into_iter()
            .map(|scope| {
                let inferred = crate::schema::Schema::infer_scoped(&files, &scope);
                let overlay_fields = overlay
                    .as_ref()
                    .and_then(|schema| schema.scopes.get(&scope))
                    .map(|scope| scope.fields.clone());
                crate::schema::ScopedSchema {
                    scope,
                    schema: crate::schema::Schema::merge(inferred, overlay_fields),
                }
            })
            .collect();
        self.index
            .set_scoped_schemas((!scoped_schemas.is_empty()).then_some(scoped_schemas));
    }

    /// Incrementally assign a new/changed document to the auto clusters and
    /// topics. Skipped when no compatible state exists (a full ingest will
    /// bootstrap or refresh it); all failures are non-fatal.
    fn update_clusters_for_file(&self, relative: &str) {
        let clusterer = crate::clustering::Clusterer::new(&self.config);

        if self.config.clustering_enabled {
            if let Some(mut state) = self.index.get_clusters() {
                if !state.clusters.is_empty() && !clusterer.algorithm_changed(&state) {
                    let doc_vectors = self.index.get_document_vectors();
                    if let Some(vector) = doc_vectors.get(relative) {
                        match clusterer.assign_incremental(
                            &mut state,
                            relative,
                            vector,
                            &doc_vectors,
                        ) {
                            Ok(_) => {
                                let doc_contents = self.index.get_document_contents();
                                if let Err(e) = clusterer.maybe_rebalance(
                                    &mut state,
                                    &doc_vectors,
                                    &doc_contents,
                                ) {
                                    warn!(error = %e, "cluster rebalance failed (non-fatal)");
                                }
                                self.index.update_clusters(Some(state));
                            }
                            Err(e) => {
                                warn!(error = %e, "cluster assignment failed (non-fatal)")
                            }
                        }
                    }
                }
            }
        }

        if !self.config.custom_cluster_defs.is_empty() {
            if let Some(mut state) = self.index.get_custom_clusters() {
                let expected = crate::clustering::topics_fingerprint(
                    &self.config.custom_cluster_defs,
                    self.config.topics_min_similarity,
                    &self.config.embedding_model,
                    self.config.embedding_dimensions,
                );
                if state.fingerprint == expected {
                    let doc_vectors = self.index.get_document_vectors();
                    if let Some(vector) = doc_vectors.get(relative) {
                        match clusterer.assign_single_to_custom(&mut state, relative, vector) {
                            Ok(()) => self.index.update_custom_clusters(Some(state)),
                            Err(e) => {
                                warn!(error = %e, "topic assignment failed (non-fatal)")
                            }
                        }
                    }
                } else {
                    debug!("topic definitions changed; run a full ingest to recompute topics");
                }
            }
        }
    }

    /// Remove a deleted/renamed document from cluster and topic membership.
    fn remove_from_clusters(&self, relative: &str) {
        let clusterer = crate::clustering::Clusterer::new(&self.config);

        if let Some(mut state) = self.index.get_clusters() {
            if clusterer.remove_document(&mut state, relative) {
                self.index.update_clusters(Some(state));
            }
        }
        if let Some(mut state) = self.index.get_custom_clusters() {
            if clusterer.remove_document_from_topics(&mut state, relative) {
                self.index.update_custom_clusters(Some(state));
            }
        }
    }
}

fn event_is_in_sources(event: &FileEvent, source_dirs: &[PathBuf]) -> bool {
    if matches!(event, FileEvent::SchemaChanged(_)) {
        return true;
    }

    let path = match event {
        FileEvent::Created(path) | FileEvent::Modified(path) | FileEvent::Deleted(path) => path,
        FileEvent::Renamed { to, .. } => to,
        FileEvent::SchemaChanged(_) => return true,
    };

    source_dirs.iter().any(|source| {
        let normalized: PathBuf = source
            .components()
            .filter_map(|component| match component {
                std::path::Component::CurDir => None,
                other => Some(other.as_os_str()),
            })
            .collect();
        normalized.as_os_str().is_empty() || path.starts_with(normalized)
    })
}

/// Classify a notify event into zero or more `FileEvent` values.
fn classify_event(
    kind: &EventKind,
    paths: &[PathBuf],
    project_root: &Path,
    discovery: &FileDiscovery,
) -> Vec<FileEvent> {
    let mut result = Vec::new();

    let schema_relative = |abs: &Path| -> Option<PathBuf> {
        let rel = abs.strip_prefix(project_root).ok()?;
        (rel == Path::new(SCHEMA_OVERLAY_PATH)).then(|| rel.to_path_buf())
    };

    let to_relative = |abs: &Path| -> Option<PathBuf> {
        let rel = abs.strip_prefix(project_root).ok()?;
        if discovery.should_index(rel) {
            Some(rel.to_path_buf())
        } else {
            None
        }
    };

    match kind {
        EventKind::Create(CreateKind::File) | EventKind::Create(CreateKind::Any) => {
            for path in paths {
                if let Some(rel) = schema_relative(path) {
                    result.push(FileEvent::SchemaChanged(rel));
                } else if let Some(rel) = to_relative(path) {
                    result.push(FileEvent::Created(rel));
                }
            }
        }
        EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Any)
        | EventKind::Modify(ModifyKind::Other) => {
            for path in paths {
                if let Some(rel) = schema_relative(path) {
                    result.push(FileEvent::SchemaChanged(rel));
                } else if let Some(rel) = to_relative(path) {
                    result.push(FileEvent::Modified(rel));
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            if paths.len() >= 2 {
                let from_schema = schema_relative(&paths[0]);
                let to_schema = schema_relative(&paths[1]);
                if let Some(schema_path) = to_schema.or(from_schema) {
                    result.push(FileEvent::SchemaChanged(schema_path));
                    return result;
                }
                let from_rel = paths[0]
                    .strip_prefix(project_root)
                    .ok()
                    .map(Path::to_path_buf);
                let to_rel = to_relative(&paths[1]);
                match (from_rel, to_rel) {
                    (Some(from), Some(to)) => {
                        result.push(FileEvent::Renamed {
                            from: from.to_path_buf(),
                            to,
                        });
                    }
                    (Some(from), None) => {
                        // Renamed to non-indexable path = delete
                        if from.extension().and_then(|e| e.to_str()) == Some("md") {
                            result.push(FileEvent::Deleted(from.to_path_buf()));
                        }
                    }
                    (None, Some(to)) => {
                        // Renamed from non-indexable path = create
                        result.push(FileEvent::Created(to));
                    }
                    _ => {}
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            for path in paths {
                if let Some(rel) = schema_relative(path) {
                    result.push(FileEvent::SchemaChanged(rel));
                } else if let Ok(rel) = path.strip_prefix(project_root) {
                    if rel.extension().and_then(|e| e.to_str()) == Some("md") {
                        result.push(FileEvent::Deleted(rel.to_path_buf()));
                    }
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            for path in paths {
                if let Some(rel) = schema_relative(path) {
                    result.push(FileEvent::SchemaChanged(rel));
                } else if let Some(rel) = to_relative(path) {
                    result.push(FileEvent::Created(rel));
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(_)) => {
            // Some backends cannot provide paired rename details. A schema
            // overlay path is still enough to trigger a safe full formula
            // recomputation; retain the existing ignore behavior for other
            // ambiguous rename events.
            for path in paths {
                if let Some(rel) = schema_relative(path) {
                    result.push(FileEvent::SchemaChanged(rel));
                }
            }
        }
        EventKind::Remove(RemoveKind::File) | EventKind::Remove(RemoveKind::Any) => {
            for path in paths {
                if let Some(rel) = schema_relative(path) {
                    result.push(FileEvent::SchemaChanged(rel));
                } else if let Ok(rel) = path.strip_prefix(project_root) {
                    if rel.extension().and_then(|e| e.to_str()) == Some("md") {
                        result.push(FileEvent::Deleted(rel.to_path_buf()));
                    }
                }
            }
        }
        _ => {}
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_discovery() -> FileDiscovery {
        let config = Config {
            embedding_provider: crate::config::EmbeddingProviderType::OpenAI,
            embedding_model: String::new(),
            embedding_dimensions: 1536,
            embedding_batch_size: 100,
            openai_api_key: None,
            ollama_host: String::new(),
            embedding_endpoint: None,
            embedding_options: Default::default(),
            source_dirs: vec![PathBuf::from(".")],
            ignore_patterns: vec![],
            watch_enabled: true,
            watch_debounce_ms: 300,
            chunk_max_tokens: 512,
            chunk_overlap_tokens: 50,
            clustering_enabled: false,
            clustering_algorithm: crate::config::ClusteringAlgorithm::Leiden,
            clustering_knn: 15,
            clustering_resolution: 1.0,
            clustering_min_cluster_size: 2,
            topics_min_similarity: 0.30,
            clustering_rebalance_threshold: 50,
            clustering_granularity: 1.0,
            search_default_limit: 10,
            search_min_score: 0.0,
            search_default_mode: crate::search::SearchMode::Hybrid,
            search_rrf_k: 60.0,
            bm25_norm_k: 1.5,
            search_decay_enabled: false,
            search_decay_half_life: 90.0,
            search_decay_exclude: vec![],
            search_decay_include: vec![],
            search_boost_links: false,
            search_boost_hops: 1,
            search_expand_graph: 0,
            search_expand_limit: 3,
            vector_quantization: crate::config::VectorQuantization::F16,
            index_compression: true,
            edge_embeddings: true,
            edge_boost_weight: 0.15,
            edge_cluster_rebalance: 50,
            custom_cluster_defs: Vec::new(),
        };
        FileDiscovery::new(Path::new("/tmp/test"), &config)
    }

    #[test]
    fn classify_create_event() {
        let discovery = test_discovery();
        let root = Path::new("/tmp/test");
        let events = classify_event(
            &EventKind::Create(CreateKind::File),
            &[root.join("docs/hello.md")],
            root,
            &discovery,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], FileEvent::Created(p) if p == Path::new("docs/hello.md")));
    }

    #[test]
    fn classify_create_non_md_filtered() {
        let discovery = test_discovery();
        let root = Path::new("/tmp/test");
        let events = classify_event(
            &EventKind::Create(CreateKind::File),
            &[root.join("docs/hello.txt")],
            root,
            &discovery,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn root_schema_watch_does_not_expand_markdown_sources() {
        let sources = vec![PathBuf::from("docs")];
        assert!(event_is_in_sources(
            &FileEvent::Modified(PathBuf::from("docs/note.md")),
            &sources
        ));
        assert!(!event_is_in_sources(
            &FileEvent::Modified(PathBuf::from("root-note.md")),
            &sources
        ));
        assert!(event_is_in_sources(
            &FileEvent::SchemaChanged(PathBuf::from(SCHEMA_OVERLAY_PATH)),
            &sources
        ));
    }

    #[test]
    fn classify_schema_overlay_create_modify_and_delete() {
        let discovery = test_discovery();
        let root = Path::new("/tmp/test");
        let schema = root.join(SCHEMA_OVERLAY_PATH);

        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(RemoveKind::File),
        ] {
            let events = classify_event(&kind, std::slice::from_ref(&schema), root, &discovery);
            assert_eq!(events.len(), 1, "event kind {kind:?}");
            assert!(matches!(
                &events[0],
                FileEvent::SchemaChanged(path)
                    if path == Path::new(SCHEMA_OVERLAY_PATH)
            ));
        }
    }

    #[test]
    fn classify_atomic_schema_overlay_replacement() {
        let discovery = test_discovery();
        let root = Path::new("/tmp/test");
        let events = classify_event(
            &EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[
                root.join(".markdownvdb.schema.yml.tmp"),
                root.join(SCHEMA_OVERLAY_PATH),
            ],
            root,
            &discovery,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            FileEvent::SchemaChanged(path)
                if path == Path::new(SCHEMA_OVERLAY_PATH)
        ));
    }

    #[test]
    fn classify_modify_event() {
        let discovery = test_discovery();
        let root = Path::new("/tmp/test");
        let events = classify_event(
            &EventKind::Modify(ModifyKind::Data(
                notify_debouncer_full::notify::event::DataChange::Content,
            )),
            &[root.join("notes.md")],
            root,
            &discovery,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], FileEvent::Modified(p) if p == Path::new("notes.md")));
    }

    #[test]
    fn classify_delete_event() {
        let discovery = test_discovery();
        let root = Path::new("/tmp/test");
        let events = classify_event(
            &EventKind::Remove(RemoveKind::File),
            &[root.join("old.md")],
            root,
            &discovery,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], FileEvent::Deleted(p) if p == Path::new("old.md")));
    }

    #[test]
    fn classify_rename_both() {
        let discovery = test_discovery();
        let root = Path::new("/tmp/test");
        let events = classify_event(
            &EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[root.join("old.md"), root.join("new.md")],
            root,
            &discovery,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            FileEvent::Renamed { from, to }
                if from == Path::new("old.md") && to == Path::new("new.md")
        ));
    }

    #[test]
    fn classify_ignored_dir_filtered() {
        let discovery = test_discovery();
        let root = Path::new("/tmp/test");
        let events = classify_event(
            &EventKind::Create(CreateKind::File),
            &[root.join(".git/hooks/readme.md")],
            root,
            &discovery,
        );
        assert!(events.is_empty());
    }
}
