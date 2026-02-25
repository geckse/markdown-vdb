pub mod state;
pub mod storage;
pub mod types;

// Re-export key types for convenient access via `crate::index::*`
pub use state::Index;
pub use types::{EmbeddingConfig, IndexMetadata, IndexStatus, StoredChunk, StoredFile};
