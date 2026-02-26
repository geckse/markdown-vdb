pub mod state;
pub mod storage;
pub mod types;

// Re-export key types for convenient access via `crate::index::*`
pub use state::Index;
pub use types::{EmbeddingConfig, IndexMetadata, IndexStatus, StoredChunk, StoredFile};

// Re-export clustering types for convenient access
pub use crate::clustering::{ClusterInfo, ClusterState};

// Re-export Schema for convenient access
pub use crate::schema::Schema;
