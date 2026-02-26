use std::fs;
use std::io::Write;
use std::path::Path;

use memmap2::Mmap;
use usearch::Index;

use crate::error::{Error, Result};
use crate::index::types::IndexMetadata;

/// Magic bytes identifying an mdvdb index file.
pub const MAGIC: &[u8; 6] = b"MDVDB\x00";

/// Current index format version.
pub const VERSION: u32 = 1;

/// Fixed header size in bytes.
pub const HEADER_SIZE: usize = 64;

/// Create a new HNSW index with default options for the given dimensionality.
pub fn create_hnsw(dimensions: usize) -> Result<Index> {
    let opts = usearch::IndexOptions {
        dimensions,
        metric: usearch::MetricKind::Cos,
        quantization: usearch::ScalarKind::F32,
        connectivity: 16,
        expansion_add: 128,
        expansion_search: 64,
        multi: false,
    };
    Index::new(&opts).map_err(|e| Error::Serialization(format!("failed to create HNSW index: {e}")))
}

/// Write an index file atomically: serialize to `.tmp`, fsync, then rename.
pub fn write_index(path: &Path, metadata: &IndexMetadata, hnsw: &Index) -> Result<()> {
    // Serialize metadata via rkyv
    let meta_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(metadata)
        .map_err(|e| Error::Serialization(e.to_string()))?;

    // Serialize HNSW to buffer
    let hnsw_len = hnsw.serialized_length();
    let mut hnsw_bytes = vec![0u8; hnsw_len];
    hnsw.save_to_buffer(&mut hnsw_bytes)
        .map_err(|e| Error::Serialization(format!("usearch save_to_buffer: {e}")))?;

    // Compute offsets
    let meta_offset: u64 = HEADER_SIZE as u64;
    let meta_size: u64 = meta_bytes.len() as u64;
    let hnsw_offset: u64 = meta_offset + meta_size;
    let hnsw_size: u64 = hnsw_bytes.len() as u64;

    // Build header (64 bytes)
    let mut header = [0u8; HEADER_SIZE];
    header[..6].copy_from_slice(MAGIC);
    header[6..10].copy_from_slice(&VERSION.to_le_bytes());
    header[10..18].copy_from_slice(&meta_offset.to_le_bytes());
    header[18..26].copy_from_slice(&meta_size.to_le_bytes());
    header[26..34].copy_from_slice(&hnsw_offset.to_le_bytes());
    header[34..42].copy_from_slice(&hnsw_size.to_le_bytes());
    // bytes 42..64 reserved

    // Write to tmp file, fsync, rename
    let tmp_path = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp_path)?;
    file.write_all(&header)?;
    file.write_all(&meta_bytes)?;
    file.write_all(&hnsw_bytes)?;
    file.sync_all()?;

    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Load an index file via memory-mapping. Returns deserialized metadata and HNSW index.
pub fn load_index(path: &Path) -> Result<(IndexMetadata, Index)> {
    if !path.exists() {
        return Err(Error::IndexNotFound {
            path: path.to_path_buf(),
        });
    }

    let file = fs::File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };

    if mmap.len() < HEADER_SIZE {
        return Err(Error::IndexCorrupted("file too small for header".into()));
    }

    // Validate magic
    if &mmap[..6] != MAGIC {
        return Err(Error::IndexCorrupted("invalid magic bytes".into()));
    }

    // Validate version
    let version = u32::from_le_bytes(mmap[6..10].try_into().unwrap());
    if version != VERSION {
        return Err(Error::IndexCorrupted(format!(
            "unsupported version: {version}"
        )));
    }

    // Read offsets
    let meta_offset = u64::from_le_bytes(mmap[10..18].try_into().unwrap()) as usize;
    let meta_size = u64::from_le_bytes(mmap[18..26].try_into().unwrap()) as usize;
    let hnsw_offset = u64::from_le_bytes(mmap[26..34].try_into().unwrap()) as usize;
    let hnsw_size = u64::from_le_bytes(mmap[34..42].try_into().unwrap()) as usize;

    // Validate regions fit in file
    if meta_offset + meta_size > mmap.len() || hnsw_offset + hnsw_size > mmap.len() {
        return Err(Error::IndexCorrupted("truncated file".into()));
    }

    // Deserialize metadata
    let meta_bytes = &mmap[meta_offset..meta_offset + meta_size];
    let metadata: IndexMetadata =
        rkyv::from_bytes::<IndexMetadata, rkyv::rancor::Error>(meta_bytes)
            .map_err(|e| Error::Serialization(format!("rkyv deserialize: {e}")))?;

    // Load HNSW — create an empty index then load state from the buffer
    let hnsw_bytes = &mmap[hnsw_offset..hnsw_offset + hnsw_size];
    let hnsw = Index::new(&usearch::IndexOptions {
        dimensions: metadata.embedding_config.dimensions,
        metric: usearch::MetricKind::Cos,
        quantization: usearch::ScalarKind::F32,
        connectivity: 16,
        expansion_add: 128,
        expansion_search: 64,
        multi: false,
    })
    .map_err(|e| Error::Serialization(format!("usearch create: {e}")))?;
    hnsw.load_from_buffer(hnsw_bytes)
        .map_err(|e| Error::Serialization(format!("usearch load: {e}")))?;

    Ok((metadata, hnsw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::types::EmbeddingConfig;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn test_metadata() -> IndexMetadata {
        IndexMetadata {
            chunks: HashMap::new(),
            files: HashMap::new(),
            embedding_config: EmbeddingConfig {
                provider: "Mock".to_string(),
                model: "test".to_string(),
                dimensions: 128,
            },
            last_updated: 1234567890,
            schema: None,
            cluster_state: None,
        }
    }

    #[test]
    fn create_hnsw_returns_index() {
        let idx = create_hnsw(128).unwrap();
        assert_eq!(idx.dimensions(), 128);
    }

    #[test]
    fn roundtrip_write_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let meta = test_metadata();
        let hnsw = create_hnsw(128).unwrap();
        hnsw.reserve(10).unwrap();

        write_index(&path, &meta, &hnsw).unwrap();
        assert!(path.exists());

        let (loaded_meta, _loaded_hnsw) = load_index(&path).unwrap();
        assert_eq!(loaded_meta.last_updated, 1234567890);
        assert_eq!(loaded_meta.embedding_config.provider, "Mock");
    }

    #[test]
    fn load_missing_file() {
        let result = load_index(Path::new("/nonexistent/index.bin"));
        assert!(matches!(result, Err(Error::IndexNotFound { .. })));
    }

    #[test]
    fn load_corrupted_magic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.idx");
        fs::write(&path, &[0u8; 64]).unwrap();
        let result = load_index(&path);
        assert!(matches!(result, Err(Error::IndexCorrupted(_))));
    }

    #[test]
    fn load_too_small() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tiny.idx");
        fs::write(&path, &[0u8; 10]).unwrap();
        let result = load_index(&path);
        assert!(matches!(result, Err(Error::IndexCorrupted(_))));
    }
}
