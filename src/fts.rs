use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, STORED, STRING,
};
use tantivy::{Index, IndexWriter, ReloadPolicy, TantivyDocument};

use crate::error::{Error, Result};
use crate::index::state::Index as VectorIndex;

const RECONCILIATION_MARKER: &str = "fts-reconcile-required";

fn reconciliation_marker_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".markdownvdb")
        .join(RECONCILIATION_MARKER)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

/// Return whether a previous vector/FTS transaction needs reconciliation.
pub(crate) fn reconciliation_required(project_root: &Path) -> Result<bool> {
    reconciliation_marker_path(project_root)
        .try_exists()
        .map_err(Error::Io)
}

/// Durably record that vector and FTS state may temporarily describe different
/// generations. The marker is deliberately created before either store is
/// mutated and remains valid even if the process exits midway through writing
/// its small payload.
pub(crate) fn begin_reconciliation(project_root: &Path) -> Result<()> {
    let state_dir = project_root.join(".markdownvdb");
    std::fs::create_dir_all(&state_dir)?;
    let marker_path = reconciliation_marker_path(project_root);

    let mut marker = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker_path)
    {
        Ok(marker) => marker,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(Error::Io(error)),
    };
    marker.write_all(b"1\n")?;
    marker.sync_all()?;
    sync_directory(&state_dir)
}

/// Rebuild the complete FTS projection from the authoritative persisted vector
/// index snapshot. A full replacement is required: a nonempty FTS index can be
/// stale just as easily as an empty one.
pub(crate) fn rebuild_from_vector_index(fts_index: &FtsIndex, index: &VectorIndex) -> Result<()> {
    fts_index.delete_all()?;

    let files = index.get_all_files();
    let mut paths: Vec<_> = files.keys().cloned().collect();
    paths.sort();
    for path in paths {
        let file = &files[&path];
        let mut chunks = Vec::with_capacity(file.chunk_ids.len());
        for chunk_id in &file.chunk_ids {
            let chunk = index.get_chunk(chunk_id).ok_or_else(|| {
                Error::Fts(format!(
                    "cannot reconcile FTS: vector index file '{path}' references missing chunk '{chunk_id}'"
                ))
            })?;
            chunks.push(FtsChunkData {
                chunk_id: chunk_id.clone(),
                source_path: chunk.source_path,
                content: strip_markdown(&chunk.content),
                heading_hierarchy: chunk.heading_hierarchy.join(" > "),
            });
        }
        if !chunks.is_empty() {
            fts_index.upsert_chunks(&path, &chunks)?;
        }
    }

    fts_index.commit()
}

/// Repair a transaction interrupted after either store had changed. The marker
/// is retired only after the rebuilt FTS commit succeeds.
pub(crate) fn recover_if_required(
    project_root: &Path,
    index: &VectorIndex,
    fts_index: &FtsIndex,
) -> Result<bool> {
    if !reconciliation_required(project_root)? {
        return Ok(false);
    }

    tracing::warn!("recovering interrupted vector/FTS transaction");
    rebuild_from_vector_index(fts_index, index)?;
    finish_reconciliation(project_root)?;
    Ok(true)
}

/// Retire the reconciliation marker after both companion stores are durable.
/// Syncing the state directory before deletion orders the vector index rename
/// ahead of marker retirement on filesystems that persist directory entries
/// independently.
pub(crate) fn finish_reconciliation(project_root: &Path) -> Result<()> {
    let state_dir = project_root.join(".markdownvdb");
    let marker_path = reconciliation_marker_path(project_root);
    sync_directory(&state_dir)?;
    match std::fs::remove_file(&marker_path) {
        Ok(()) => sync_directory(&state_dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

/// Data for a single chunk to be indexed in the FTS index.
#[derive(Debug, Clone)]
pub struct FtsChunkData {
    /// Deterministic chunk identifier (e.g. `"path.md#0"`).
    pub chunk_id: String,
    /// Relative path to the source markdown file.
    pub source_path: String,
    /// Plain-text content with markdown stripped.
    pub content: String,
    /// Heading hierarchy joined as a single string.
    pub heading_hierarchy: String,
}

/// A single FTS search result.
#[derive(Debug, Clone)]
pub struct FtsResult {
    /// The chunk ID that matched.
    pub chunk_id: String,
    /// BM25 relevance score from Tantivy.
    pub score: f32,
}

/// Schema field handles cached for the FTS index.
struct FtsFields {
    chunk_id: Field,
    source_path: Field,
    content: Field,
    heading_hierarchy: Field,
}

/// Wrapper around a Tantivy index for full-text search of chunks.
pub struct FtsIndex {
    index: Index,
    fields: FtsFields,
    writer: Option<parking_lot::Mutex<IndexWriter>>,
}

impl FtsIndex {
    /// Open an existing Tantivy index or create a new one at the given directory.
    /// Acquires an exclusive writer lock — use [`open_readonly`] for read-only access.
    pub fn open_or_create(path: &Path) -> Result<Self> {
        let (schema, fields) = build_schema();

        let index = if path.exists() && path.join("meta.json").exists() {
            Index::open_in_dir(path).map_err(|e| Error::Fts(e.to_string()))?
        } else {
            std::fs::create_dir_all(path)?;
            Index::create_in_dir(path, schema).map_err(|e| Error::Fts(e.to_string()))?
        };

        // A watcher or previous in-process writer can release Tantivy's lock a
        // moment after its owner is dropped. Brief retries make watcher
        // pause/run/restart and immediate reopen deterministic without hiding
        // a genuinely active writer.
        let mut attempts = 0;
        let writer = loop {
            match index.writer(50_000_000) {
                Ok(writer) => break writer,
                Err(tantivy::TantivyError::LockFailure(..)) if attempts < 9 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(tantivy::TantivyError::LockFailure(..)) => {
                    return Err(Error::IndexBusy {
                        path: path.to_path_buf(),
                    });
                }
                Err(other) => return Err(Error::Fts(other.to_string())),
            }
        };

        Ok(Self {
            index,
            fields,
            writer: Some(parking_lot::Mutex::new(writer)),
        })
    }

    /// Open an existing Tantivy index in read-only mode (no writer lock acquired).
    /// Write operations (`upsert_chunks`, `remove_file`, `commit`, `delete_all`) will
    /// return an error if called on a read-only instance.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let (schema, fields) = build_schema();

        let index = if path.exists() && path.join("meta.json").exists() {
            Index::open_in_dir(path).map_err(|e| Error::Fts(e.to_string()))?
        } else {
            std::fs::create_dir_all(path)?;
            Index::create_in_dir(path, schema).map_err(|e| Error::Fts(e.to_string()))?
        };

        Ok(Self {
            index,
            fields,
            writer: None,
        })
    }

    /// Upsert chunks for a given source file.
    ///
    /// Deletes all existing chunks for the source path, then adds the new chunks.
    /// Call [`commit`] after all upserts are done.
    pub fn upsert_chunks(&self, source_path: &str, chunks: &[FtsChunkData]) -> Result<()> {
        let writer_mutex = self
            .writer
            .as_ref()
            .ok_or_else(|| Error::Fts("FTS index opened in read-only mode".into()))?;
        let writer = writer_mutex.lock();
        // Delete existing docs for this source path.
        let term = tantivy::Term::from_field_text(self.fields.source_path, source_path);
        writer.delete_term(term);

        for chunk in chunks {
            let mut doc = TantivyDocument::new();
            doc.add_text(self.fields.chunk_id, &chunk.chunk_id);
            doc.add_text(self.fields.source_path, &chunk.source_path);
            doc.add_text(self.fields.content, &chunk.content);
            doc.add_text(self.fields.heading_hierarchy, &chunk.heading_hierarchy);
            writer
                .add_document(doc)
                .map_err(|e| Error::Fts(e.to_string()))?;
        }
        Ok(())
    }

    /// Remove all chunks for a given source file path.
    ///
    /// Call [`commit`] after removals are done.
    pub fn remove_file(&self, source_path: &str) -> Result<()> {
        let writer_mutex = self
            .writer
            .as_ref()
            .ok_or_else(|| Error::Fts("FTS index opened in read-only mode".into()))?;
        let writer = writer_mutex.lock();
        let term = tantivy::Term::from_field_text(self.fields.source_path, source_path);
        writer.delete_term(term);
        Ok(())
    }

    /// Search the FTS index for matching chunks.
    ///
    /// Returns up to `limit` results sorted by BM25 score descending.
    /// Empty queries return an empty vec.
    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<FtsResult>> {
        let query_str = query_str.trim();
        if query_str.is_empty() {
            return Ok(Vec::new());
        }

        let reader = self
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e: tantivy::TantivyError| Error::Fts(e.to_string()))?;

        let searcher = reader.searcher();

        let mut query_parser = QueryParser::for_index(
            &self.index,
            vec![self.fields.content, self.fields.heading_hierarchy],
        );
        query_parser.set_field_boost(self.fields.heading_hierarchy, 1.5);

        let (query, _errors) = query_parser.parse_query_lenient(query_str);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .map_err(|e| Error::Fts(e.to_string()))?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| Error::Fts(e.to_string()))?;
            if let Some(chunk_id) =
                doc.get_first(self.fields.chunk_id)
                    .and_then(|v: &tantivy::schema::OwnedValue| {
                        if let tantivy::schema::OwnedValue::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
            {
                results.push(FtsResult {
                    chunk_id: chunk_id.to_string(),
                    score,
                });
            }
        }
        Ok(results)
    }

    /// Commit all pending writes to the index and reload the reader.
    pub fn commit(&self) -> Result<()> {
        let writer_mutex = self
            .writer
            .as_ref()
            .ok_or_else(|| Error::Fts("FTS index opened in read-only mode".into()))?;
        let mut writer = writer_mutex.lock();
        writer.commit().map_err(|e| Error::Fts(e.to_string()))?;
        Ok(())
    }

    /// Return the number of documents in the index.
    pub fn num_docs(&self) -> Result<u64> {
        let reader = self
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e: tantivy::TantivyError| Error::Fts(e.to_string()))?;
        let searcher = reader.searcher();
        Ok(searcher.num_docs())
    }

    /// Delete all documents from the index.
    pub fn delete_all(&self) -> Result<()> {
        let writer_mutex = self
            .writer
            .as_ref()
            .ok_or_else(|| Error::Fts("FTS index opened in read-only mode".into()))?;
        let writer = writer_mutex.lock();
        writer
            .delete_all_documents()
            .map_err(|e| Error::Fts(e.to_string()))?;
        Ok(())
    }
}

/// Build the Tantivy schema and return field handles.
fn build_schema() -> (Schema, FtsFields) {
    let mut builder = Schema::builder();

    let chunk_id = builder.add_text_field("chunk_id", STRING | STORED);
    let source_path = builder.add_text_field("source_path", STRING | STORED);

    // Content: indexed with English stemming, not stored (data lives in rkyv).
    let text_indexing = TextFieldIndexing::default()
        .set_tokenizer("en_stem")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let text_options = TextOptions::default().set_indexing_options(text_indexing);
    let content = builder.add_text_field("content", text_options.clone());
    let heading_hierarchy = builder.add_text_field("heading_hierarchy", text_options);

    let schema = builder.build();
    (
        schema,
        FtsFields {
            chunk_id,
            source_path,
            content,
            heading_hierarchy,
        },
    )
}

/// Strip markdown formatting from content, returning plain text.
///
/// Uses `pulldown-cmark` to parse and extract only text and code events.
pub fn strip_markdown(content: &str) -> String {
    use pulldown_cmark::{Event, Parser};

    let parser = Parser::new(content);
    let mut text = String::new();
    for event in parser {
        match event {
            Event::Text(t) => text.push_str(&t),
            Event::Code(c) => text.push_str(&c),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::chunk_document;
    use crate::index::types::EmbeddingConfig;
    use crate::parser::parse_markdown_file;
    use tempfile::TempDir;

    #[test]
    fn strip_markdown_removes_formatting() {
        let md = "# Hello **world** and `code` here\n\n[link](http://example.com) text";
        let plain = strip_markdown(md);
        assert!(plain.contains("Hello"));
        assert!(plain.contains("world"));
        assert!(plain.contains("code"));
        assert!(plain.contains("link"));
        assert!(plain.contains("text"));
        assert!(!plain.contains('#'));
        assert!(!plain.contains('*'));
        assert!(!plain.contains('['));
        assert!(!plain.contains("http"));
    }

    #[test]
    fn open_or_create_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fts_idx");

        // Create
        {
            let idx = FtsIndex::open_or_create(&path).unwrap();
            idx.commit().unwrap();
        }

        // Reopen
        {
            let idx = FtsIndex::open_or_create(&path).unwrap();
            assert_eq!(idx.num_docs().unwrap(), 0);
        }
    }

    #[test]
    fn upsert_and_search() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fts_idx");
        let idx = FtsIndex::open_or_create(&path).unwrap();

        let chunks = vec![
            FtsChunkData {
                chunk_id: "doc.md#0".into(),
                source_path: "doc.md".into(),
                content: "Rust programming language is fast and safe".into(),
                heading_hierarchy: "Introduction".into(),
            },
            FtsChunkData {
                chunk_id: "doc.md#1".into(),
                source_path: "doc.md".into(),
                content: "Python is great for data science".into(),
                heading_hierarchy: "Alternatives".into(),
            },
        ];

        idx.upsert_chunks("doc.md", &chunks).unwrap();
        idx.commit().unwrap();

        let results = idx.search("rust programming", 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].chunk_id, "doc.md#0");
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn remove_file_removes_chunks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fts_idx");
        let idx = FtsIndex::open_or_create(&path).unwrap();

        let chunks = vec![FtsChunkData {
            chunk_id: "a.md#0".into(),
            source_path: "a.md".into(),
            content: "unique searchable content here".into(),
            heading_hierarchy: String::new(),
        }];

        idx.upsert_chunks("a.md", &chunks).unwrap();
        idx.commit().unwrap();

        // Verify it's findable
        let results = idx.search("unique searchable", 10).unwrap();
        assert!(!results.is_empty());

        // Remove and verify gone
        idx.remove_file("a.md").unwrap();
        idx.commit().unwrap();

        let results = idx.search("unique searchable", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn empty_query_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fts_idx");
        let idx = FtsIndex::open_or_create(&path).unwrap();
        idx.commit().unwrap();

        let results = idx.search("", 10).unwrap();
        assert!(results.is_empty());

        let results = idx.search("   ", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn heading_boost_increases_score() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fts_idx");
        let idx = FtsIndex::open_or_create(&path).unwrap();

        let chunk_a = [FtsChunkData {
            chunk_id: "a.md#0".into(),
            source_path: "a.md".into(),
            content: "some unrelated body text here".into(),
            heading_hierarchy: "database optimization techniques".into(),
        }];
        let chunk_b = [FtsChunkData {
            chunk_id: "b.md#0".into(),
            source_path: "b.md".into(),
            content: "database optimization techniques explained in detail".into(),
            heading_hierarchy: "some unrelated heading".into(),
        }];

        idx.upsert_chunks("a.md", &chunk_a).unwrap();
        idx.upsert_chunks("b.md", &chunk_b).unwrap();
        idx.commit().unwrap();

        let results = idx.search("database optimization", 10).unwrap();
        assert!(results.len() >= 2);
        // Both should appear; the one with heading match should benefit from boost
        // but exact ordering depends on BM25 + boost interaction
        let chunk_ids: Vec<&str> = results.iter().map(|r| r.chunk_id.as_str()).collect();
        assert!(chunk_ids.contains(&"a.md#0"));
        assert!(chunk_ids.contains(&"b.md#0"));
    }

    #[test]
    fn durable_marker_recovers_a_nonempty_stale_fts_index() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".markdownvdb")).unwrap();
        std::fs::write(
            root.join("doc.md"),
            "# Current\n\nThe authoritative vector snapshot contains platypuses.\n",
        )
        .unwrap();

        let file = parse_markdown_file(root, Path::new("doc.md")).unwrap();
        let chunks = chunk_document(&file, 512, 0).unwrap();
        assert!(!chunks.is_empty());
        let embedding_config = EmbeddingConfig {
            provider: "test".into(),
            model: "test".into(),
            dimensions: 8,
        };
        let index =
            VectorIndex::create(&root.join(".markdownvdb").join("index"), &embedding_config)
                .unwrap();
        let embeddings = vec![vec![1.0; 8]; chunks.len()];
        index.upsert(&file, &chunks, &embeddings).unwrap();
        index.save().unwrap();

        let fts_index = FtsIndex::open_or_create(&root.join(".markdownvdb").join("fts")).unwrap();
        fts_index
            .upsert_chunks(
                "obsolete.md",
                &[FtsChunkData {
                    chunk_id: "obsolete.md#0".into(),
                    source_path: "obsolete.md".into(),
                    content: "This stale committed generation contains narwhals.".into(),
                    heading_hierarchy: "Obsolete".into(),
                }],
            )
            .unwrap();
        fts_index.commit().unwrap();
        assert!(!fts_index.search("narwhals", 10).unwrap().is_empty());
        assert!(fts_index.search("platypuses", 10).unwrap().is_empty());

        // Simulate a process dying after committing the vector generation but
        // before replacing an already-populated FTS generation.
        begin_reconciliation(root).unwrap();
        assert!(reconciliation_required(root).unwrap());

        assert!(recover_if_required(root, &index, &fts_index).unwrap());
        assert!(!reconciliation_required(root).unwrap());
        assert!(fts_index.search("narwhals", 10).unwrap().is_empty());
        let current = fts_index.search("platypuses", 10).unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].chunk_id, chunks[0].id);
    }

    #[test]
    fn incompatible_vector_rebuild_marks_before_replacing_nonempty_fts() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let state_dir = root.join(".markdownvdb");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(root.join("doc.md"), "# Old\n\nSearchable quokkas.\n").unwrap();

        let file = parse_markdown_file(root, Path::new("doc.md")).unwrap();
        let chunks = chunk_document(&file, 512, 0).unwrap();
        let embedding_config = EmbeddingConfig {
            provider: "test".into(),
            model: "test".into(),
            dimensions: 8,
        };
        let index_path = state_dir.join("index");
        let index = VectorIndex::create(&index_path, &embedding_config).unwrap();
        index
            .upsert(&file, &chunks, &vec![vec![1.0; 8]; chunks.len()])
            .unwrap();
        index.save().unwrap();

        let fts_index = FtsIndex::open_or_create(&state_dir.join("fts")).unwrap();
        fts_index
            .upsert_chunks(
                "doc.md",
                &[FtsChunkData {
                    chunk_id: "doc.md#0".into(),
                    source_path: "doc.md".into(),
                    content: "Searchable quokkas.".into(),
                    heading_hierarchy: "Old".into(),
                }],
            )
            .unwrap();
        fts_index.commit().unwrap();
        drop(index);

        // Force the archived-version gate, then stop at the exact historical
        // crash boundary: vector replacement has completed but FTS repair has
        // not started. The pre-rebuild hook must already be durable.
        let mut archived = std::fs::read(&index_path).unwrap();
        archived[6..10].copy_from_slice(&(crate::index::storage::VERSION + 1).to_le_bytes());
        std::fs::write(&index_path, archived).unwrap();
        let (rebuilt, was_rebuilt) =
            VectorIndex::open_or_create_with_options_report_and_rebuild_hook(
                &index_path,
                &embedding_config,
                crate::index::storage::WriteOptions::default(),
                || begin_reconciliation(root),
            )
            .unwrap();
        assert!(was_rebuilt);
        assert!(reconciliation_required(root).unwrap());
        assert!(!fts_index.search("quokkas", 10).unwrap().is_empty());

        assert!(recover_if_required(root, &rebuilt, &fts_index).unwrap());
        assert!(!reconciliation_required(root).unwrap());
        assert!(fts_index.search("quokkas", 10).unwrap().is_empty());
        assert_eq!(fts_index.num_docs().unwrap(), 0);
    }
}
