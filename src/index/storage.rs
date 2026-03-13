use std::fs;
use std::io::Write;
use std::path::Path;

use memmap2::Mmap;
use usearch::Index;

use crate::config::VectorQuantization;
use crate::error::{Error, Result};
use crate::index::types::IndexMetadata;

/// Magic bytes identifying an mdvdb index file.
pub const MAGIC: &[u8; 6] = b"MDVDB\x00";

/// Current index format version.
pub const VERSION: u32 = 2;

/// Fixed header size in bytes.
pub const HEADER_SIZE: usize = 64;

/// Quantization byte value: F32.
const QUANT_F32: u8 = 0;

/// Quantization byte value: F16.
const QUANT_F16: u8 = 1;

/// Compression flag: zstd.
const COMPRESS_ZSTD: u8 = 0x01;

/// Default zstd compression level.
const ZSTD_LEVEL: i32 = 3;

/// Options controlling how the index is written to disk.
#[derive(Debug, Clone)]
pub struct WriteOptions {
    pub quantization: VectorQuantization,
    pub compress_metadata: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            quantization: VectorQuantization::F16,
            compress_metadata: true,
        }
    }
}

/// Convert a `VectorQuantization` config value to the corresponding usearch `ScalarKind`.
pub fn scalar_kind_for(q: &VectorQuantization) -> usearch::ScalarKind {
    match q {
        VectorQuantization::F16 => usearch::ScalarKind::F16,
        VectorQuantization::F32 => usearch::ScalarKind::F32,
    }
}

/// Create a new HNSW index with the given dimensionality and scalar kind.
pub fn create_hnsw(dimensions: usize, quantization: usearch::ScalarKind) -> Result<Index> {
    let opts = usearch::IndexOptions {
        dimensions,
        metric: usearch::MetricKind::Cos,
        quantization,
        connectivity: 16,
        expansion_add: 128,
        expansion_search: 64,
        multi: false,
    };
    Index::new(&opts).map_err(|e| Error::Serialization(format!("failed to create HNSW index: {e}")))
}

/// Write an index file atomically: serialize to `.tmp`, fsync, then rename.
///
/// Writes a V2 format header with quantization type and optional zstd compression
/// of the rkyv metadata region.
pub fn write_index(
    path: &Path,
    metadata: &IndexMetadata,
    hnsw: &Index,
    options: &WriteOptions,
) -> Result<()> {
    // Serialize metadata via rkyv
    let meta_bytes_raw = rkyv::to_bytes::<rkyv::rancor::Error>(metadata)
        .map_err(|e| Error::Serialization(e.to_string()))?;

    let uncompressed_meta_size = meta_bytes_raw.len() as u32;

    // Optionally compress metadata with zstd
    let meta_bytes: Vec<u8> = if options.compress_metadata {
        zstd::bulk::compress(&meta_bytes_raw, ZSTD_LEVEL)
            .map_err(|e| Error::Serialization(format!("zstd compress: {e}")))?
    } else {
        meta_bytes_raw.to_vec()
    };

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

    // Build V2 header (64 bytes)
    let mut header = [0u8; HEADER_SIZE];
    header[..6].copy_from_slice(MAGIC);
    header[6..10].copy_from_slice(&VERSION.to_le_bytes());
    header[10..18].copy_from_slice(&meta_offset.to_le_bytes());
    header[18..26].copy_from_slice(&meta_size.to_le_bytes());
    header[26..34].copy_from_slice(&hnsw_offset.to_le_bytes());
    header[34..42].copy_from_slice(&hnsw_size.to_le_bytes());
    // V2 extension fields (bytes 42..48)
    header[42] = match options.quantization {
        VectorQuantization::F16 => QUANT_F16,
        VectorQuantization::F32 => QUANT_F32,
    };
    header[43] = if options.compress_metadata {
        COMPRESS_ZSTD
    } else {
        0
    };
    header[44..48].copy_from_slice(&uncompressed_meta_size.to_le_bytes());
    // bytes 48..64 reserved

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
        if version < VERSION {
            return Err(Error::IndexVersionMismatch { version });
        }
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

    // Read extension fields
    let quantization_byte = mmap[42];
    let compression_flags = mmap[43];
    let uncompressed_meta_size = u32::from_le_bytes(mmap[44..48].try_into().unwrap()) as usize;

    // Determine HNSW ScalarKind from header
    let scalar_kind = match quantization_byte {
        QUANT_F32 => usearch::ScalarKind::F32,
        QUANT_F16 => usearch::ScalarKind::F16,
        other => {
            return Err(Error::IndexCorrupted(format!(
                "unknown quantization type: {other}"
            )))
        }
    };

    // Decompress metadata if zstd flag is set
    let raw_meta_bytes = &mmap[meta_offset..meta_offset + meta_size];
    let decompressed: Vec<u8>;
    let meta_bytes: &[u8] = if compression_flags & COMPRESS_ZSTD != 0 {
        decompressed = zstd::bulk::decompress(raw_meta_bytes, uncompressed_meta_size)
            .map_err(|e| Error::Serialization(format!("zstd decompress: {e}")))?;
        &decompressed
    } else {
        raw_meta_bytes
    };

    // Deserialize metadata
    let metadata: IndexMetadata =
        rkyv::from_bytes::<IndexMetadata, rkyv::rancor::Error>(meta_bytes)
            .map_err(|_| Error::IndexCorrupted(
                "index format is incompatible or corrupted — delete .markdownvdb/ and re-ingest".into()
            ))?;

    // Load HNSW with the correct ScalarKind
    let hnsw_bytes = &mmap[hnsw_offset..hnsw_offset + hnsw_size];
    let hnsw = Index::new(&usearch::IndexOptions {
        dimensions: metadata.embedding_config.dimensions,
        metric: usearch::MetricKind::Cos,
        quantization: scalar_kind,
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
            link_graph: None,
            file_mtimes: Some(HashMap::new()),
            scoped_schemas: None,
        }
    }

    #[test]
    fn create_hnsw_returns_index() {
        let idx = create_hnsw(128, usearch::ScalarKind::F32).unwrap();
        assert_eq!(idx.dimensions(), 128);
    }

    #[test]
    fn create_hnsw_f16() {
        let idx = create_hnsw(128, usearch::ScalarKind::F16).unwrap();
        assert_eq!(idx.dimensions(), 128);
    }

    #[test]
    fn roundtrip_write_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let meta = test_metadata();
        let hnsw = create_hnsw(128, usearch::ScalarKind::F16).unwrap();
        hnsw.reserve(10).unwrap();

        write_index(&path, &meta, &hnsw, &WriteOptions::default()).unwrap();
        assert!(path.exists());

        let (loaded_meta, _loaded_hnsw) = load_index(&path).unwrap();
        assert_eq!(loaded_meta.last_updated, 1234567890);
        assert_eq!(loaded_meta.embedding_config.provider, "Mock");
    }

    #[test]
    fn roundtrip_v2_f16_compressed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let meta = test_metadata();
        let hnsw = create_hnsw(128, usearch::ScalarKind::F16).unwrap();
        hnsw.reserve(10).unwrap();

        let options = WriteOptions {
            quantization: VectorQuantization::F16,
            compress_metadata: true,
        };
        write_index(&path, &meta, &hnsw, &options).unwrap();

        let (loaded_meta, _) = load_index(&path).unwrap();
        assert_eq!(loaded_meta.last_updated, 1234567890);
        assert_eq!(loaded_meta.embedding_config.provider, "Mock");
    }

    #[test]
    fn roundtrip_v2_f32_uncompressed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let meta = test_metadata();
        let hnsw = create_hnsw(128, usearch::ScalarKind::F32).unwrap();
        hnsw.reserve(10).unwrap();

        let options = WriteOptions {
            quantization: VectorQuantization::F32,
            compress_metadata: false,
        };
        write_index(&path, &meta, &hnsw, &options).unwrap();

        let (loaded_meta, _) = load_index(&path).unwrap();
        assert_eq!(loaded_meta.last_updated, 1234567890);
    }

    #[test]
    fn header_bytes_correct() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let meta = test_metadata();
        let hnsw = create_hnsw(128, usearch::ScalarKind::F16).unwrap();
        hnsw.reserve(10).unwrap();

        let options = WriteOptions {
            quantization: VectorQuantization::F16,
            compress_metadata: true,
        };
        write_index(&path, &meta, &hnsw, &options).unwrap();

        let raw = fs::read(&path).unwrap();
        assert_eq!(&raw[..6], b"MDVDB\x00");
        assert_eq!(u32::from_le_bytes(raw[6..10].try_into().unwrap()), VERSION);
        assert_eq!(raw[42], QUANT_F16);
        assert_eq!(raw[43], COMPRESS_ZSTD);
        let uncomp = u32::from_le_bytes(raw[44..48].try_into().unwrap());
        assert!(uncomp > 0);
    }

    #[test]
    fn unknown_future_version_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("future.idx");
        let mut data = vec![0u8; 128];
        data[..6].copy_from_slice(b"MDVDB\x00");
        data[6..10].copy_from_slice(&999u32.to_le_bytes());
        fs::write(&path, &data).unwrap();
        let result = load_index(&path);
        assert!(matches!(result, Err(Error::IndexCorrupted(_))));
    }

    #[test]
    fn old_version_returns_mismatch() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("old.idx");
        let mut data = vec![0u8; 128];
        data[..6].copy_from_slice(b"MDVDB\x00");
        data[6..10].copy_from_slice(&1u32.to_le_bytes());
        fs::write(&path, &data).unwrap();
        let result = load_index(&path);
        assert!(matches!(
            result,
            Err(Error::IndexVersionMismatch { version: 1 })
        ));
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
        fs::write(&path, [0u8; 64]).unwrap();
        let result = load_index(&path);
        assert!(matches!(result, Err(Error::IndexCorrupted(_))));
    }

    #[test]
    fn load_too_small() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tiny.idx");
        fs::write(&path, [0u8; 10]).unwrap();
        let result = load_index(&path);
        assert!(matches!(result, Err(Error::IndexCorrupted(_))));
    }
}
