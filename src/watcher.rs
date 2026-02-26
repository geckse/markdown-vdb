use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_full::notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify_debouncer_full::notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::config::Config;
use crate::discovery::FileDiscovery;
use crate::embedding::provider::EmbeddingProvider;
use crate::error::{Error, Result};
use crate::index::state::Index;

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
}

/// Watches configured source directories for markdown file changes and
/// triggers incremental re-indexing.
pub struct Watcher {
    config: Config,
    project_root: PathBuf,
    index: Arc<Index>,
    provider: Arc<dyn EmbeddingProvider>,
    #[allow(dead_code)]
    discovery: FileDiscovery,
}

impl Watcher {
    /// Create a new `Watcher`.
    pub fn new(
        config: Config,
        project_root: &Path,
        index: Arc<Index>,
        provider: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        let discovery = FileDiscovery::new(project_root, &config);
        Self {
            config,
            project_root: project_root.to_path_buf(),
            index,
            provider,
            discovery,
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
                    let file_events =
                        classify_event(&event.event.kind, &event.paths, &project_root, &cb_discovery);
                    for fe in file_events {
                        if tx.send(fe).is_err() {
                            debug!("watcher channel closed, stopping event forwarding");
                            return;
                        }
                    }
                }
            },
        )
        .map_err(|e| Error::Watch(format!("failed to create debouncer: {e}")))?;

        // Watch each configured source directory.
        for source_dir in &self.config.source_dirs {
            let abs_dir = self.project_root.join(source_dir);
            if !abs_dir.is_dir() {
                debug!("skipping non-existent source dir: {}", abs_dir.display());
                continue;
            }
            debouncer
                .watch(&abs_dir, RecursiveMode::Recursive)
                .map_err(|e| {
                    Error::Watch(format!("failed to watch {}: {e}", abs_dir.display()))
                })?;
            info!("watching directory: {}", abs_dir.display());
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
        match event {
            FileEvent::Created(path) | FileEvent::Modified(path) => {
                debug!(path = %path.display(), "processing created/modified event");
                self.process_file(path).await
            }
            FileEvent::Deleted(path) => {
                let relative = path.to_string_lossy().to_string();
                info!(path = %relative, "removing deleted file from index");
                self.index.remove_file(&relative)?;
                self.index.save()?;
                Ok(())
            }
            FileEvent::Renamed { from, to } => {
                let from_str = from.to_string_lossy().to_string();
                debug!(from = %from_str, to = %to.display(), "processing rename event");
                self.index.remove_file(&from_str)?;
                self.process_file(to).await
            }
        }
    }

    /// Parse, chunk, embed, and upsert a single file.
    async fn process_file(&self, relative_path: &Path) -> Result<()> {
        let abs_path = self.project_root.join(relative_path);

        // If the file no longer exists (deleted between event and processing, or the
        // OS sent a Modify event for a removal), treat it as a deletion.
        if !abs_path.is_file() {
            let relative = relative_path.to_string_lossy().to_string();
            info!(path = %relative, "file no longer exists, removing from index");
            self.index.remove_file(&relative)?;
            self.index.save()?;
            return Ok(());
        }

        // Check content hash to skip unchanged files.
        let stored_hash = self
            .index
            .get_file(&relative_path.to_string_lossy())
            .map(|f| f.content_hash.clone());

        let file = crate::parser::parse_markdown_file(&self.project_root, relative_path)?;

        if let Some(ref hash) = stored_hash {
            if hash == &file.content_hash {
                debug!(path = %relative_path.display(), "content unchanged, skipping");
                return Ok(());
            }
        }

        let chunks = crate::chunker::chunk_document(
            &file,
            self.config.chunk_max_tokens,
            self.config.chunk_overlap_tokens,
        )?;

        if chunks.is_empty() {
            debug!(path = %relative_path.display(), "no chunks produced, skipping");
            return Ok(());
        }

        // Embed all chunk texts.
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = self.provider.embed_batch(&texts).await?;

        // Upsert and save.
        self.index.upsert(&file, &chunks, &embeddings)?;

        // Update schema inference with the new/changed file's frontmatter.
        if file.frontmatter.is_some() {
            let schema = crate::schema::Schema::infer(&[file]);
            let overlay = crate::schema::Schema::load_overlay(&self.project_root)
                .unwrap_or(None);
            let merged = if let Some(existing) = self.index.get_schema() {
                // Merge new schema fields into existing schema.
                let combined = crate::schema::Schema::merge(existing, None);
                // Re-infer to include all frontmatter from the index is not
                // practical here, so we merge the new inferences into existing.
                crate::schema::Schema::merge(combined, overlay)
            } else {
                crate::schema::Schema::merge(schema, overlay)
            };
            self.index.set_schema(Some(merged));
        }

        self.index.save()?;
        info!(
            path = %relative_path.display(),
            chunks = chunks.len(),
            "indexed file"
        );

        Ok(())
    }
}

/// Classify a notify event into zero or more `FileEvent` values.
fn classify_event(
    kind: &EventKind,
    paths: &[PathBuf],
    project_root: &Path,
    discovery: &FileDiscovery,
) -> Vec<FileEvent> {
    let mut result = Vec::new();

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
                if let Some(rel) = to_relative(path) {
                    result.push(FileEvent::Created(rel));
                }
            }
        }
        EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Any)
        | EventKind::Modify(ModifyKind::Other) => {
            for path in paths {
                if let Some(rel) = to_relative(path) {
                    result.push(FileEvent::Modified(rel));
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            if paths.len() >= 2 {
                let from_rel = paths[0].strip_prefix(project_root).ok().map(Path::to_path_buf);
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
                if let Ok(rel) = path.strip_prefix(project_root) {
                    if rel.extension().and_then(|e| e.to_str()) == Some("md") {
                        result.push(FileEvent::Deleted(rel.to_path_buf()));
                    }
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            for path in paths {
                if let Some(rel) = to_relative(path) {
                    result.push(FileEvent::Created(rel));
                }
            }
        }
        EventKind::Remove(RemoveKind::File) | EventKind::Remove(RemoveKind::Any) => {
            for path in paths {
                if let Ok(rel) = path.strip_prefix(project_root) {
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
            source_dirs: vec![PathBuf::from(".")],
            index_file: PathBuf::from(".markdownvdb.index"),
            ignore_patterns: vec![],
            watch_enabled: true,
            watch_debounce_ms: 300,
            chunk_max_tokens: 512,
            chunk_overlap_tokens: 50,
            clustering_enabled: false,
            clustering_rebalance_threshold: 50,
            search_default_limit: 10,
            search_min_score: 0.0,
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
        assert!(
            matches!(&events[0], FileEvent::Created(p) if p == Path::new("docs/hello.md"))
        );
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
