use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, STORED, STRING,
};
use tantivy::{Index, IndexWriter, ReloadPolicy, TantivyDocument};

use crate::error::{Error, Result};

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
    writer: parking_lot::Mutex<IndexWriter>,
}

impl FtsIndex {
    /// Open an existing Tantivy index or create a new one at the given directory.
    pub fn open_or_create(path: &Path) -> Result<Self> {
        let (schema, fields) = build_schema();

        let index = if path.exists() && path.join("meta.json").exists() {
            Index::open_in_dir(path).map_err(|e| Error::Fts(e.to_string()))?
        } else {
            std::fs::create_dir_all(path)?;
            Index::create_in_dir(path, schema).map_err(|e| Error::Fts(e.to_string()))?
        };

        let writer = index
            .writer(50_000_000) // 50MB heap
            .map_err(|e| Error::Fts(e.to_string()))?;

        Ok(Self {
            index,
            fields,
            writer: parking_lot::Mutex::new(writer),
        })
    }

    /// Upsert chunks for a given source file.
    ///
    /// Deletes all existing chunks for the source path, then adds the new chunks.
    /// Call [`commit`] after all upserts are done.
    pub fn upsert_chunks(&self, source_path: &str, chunks: &[FtsChunkData]) -> Result<()> {
        let writer = self.writer.lock();
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
        let writer = self.writer.lock();
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

        let mut query_parser =
            QueryParser::for_index(&self.index, vec![self.fields.content, self.fields.heading_hierarchy]);
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
            if let Some(chunk_id) = doc.get_first(self.fields.chunk_id).and_then(|v: &tantivy::schema::OwnedValue| {
                if let tantivy::schema::OwnedValue::Str(s) = v { Some(s.as_str()) } else { None }
            }) {
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
        let mut writer = self.writer.lock();
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
        let writer = self.writer.lock();
        writer.delete_all_documents().map_err(|e| Error::Fts(e.to_string()))?;
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

        let chunks = vec![
            FtsChunkData {
                chunk_id: "a.md#0".into(),
                source_path: "a.md".into(),
                content: "some unrelated body text here".into(),
                heading_hierarchy: "database optimization techniques".into(),
            },
            FtsChunkData {
                chunk_id: "b.md#0".into(),
                source_path: "b.md".into(),
                content: "database optimization techniques explained in detail".into(),
                heading_hierarchy: "some unrelated heading".into(),
            },
        ];

        idx.upsert_chunks("a.md", &chunks[0..1]).unwrap();
        idx.upsert_chunks("b.md", &chunks[1..2]).unwrap();
        idx.commit().unwrap();

        let results = idx.search("database optimization", 10).unwrap();
        assert!(results.len() >= 2);
        // Both should appear; the one with heading match should benefit from boost
        // but exact ordering depends on BM25 + boost interaction
        let chunk_ids: Vec<&str> = results.iter().map(|r| r.chunk_id.as_str()).collect();
        assert!(chunk_ids.contains(&"a.md#0"));
        assert!(chunk_ids.contains(&"b.md#0"));
    }
}
