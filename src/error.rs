use std::path::PathBuf;

/// All errors that can occur in mdvdb.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("index not found: {}", path.display())]
    IndexNotFound { path: PathBuf },

    #[error("index corrupted: {0}")]
    IndexCorrupted(String),

    #[error("embedding provider error: {0}")]
    EmbeddingProvider(String),

    #[error("markdown parse error in {}: {message}", path.display())]
    MarkdownParse { path: PathBuf, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("watch error: {0}")]
    Watch(String),

    #[error("lock acquisition timed out")]
    LockTimeout,

    #[error("logging initialization failed: {0}")]
    Logging(String),

    #[error("file not in index: {}", path.display())]
    FileNotInIndex { path: PathBuf },

    #[error("index already exists: {}", path.display())]
    IndexAlreadyExists { path: PathBuf },

    #[error("config already exists: {}", path.display())]
    ConfigAlreadyExists { path: PathBuf },

    #[error("clustering error: {0}")]
    Clustering(String),

    #[error("link graph not built: run `mdvdb links` first")]
    LinkGraphNotBuilt,

    #[error("full-text search error: {0}")]
    Fts(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn config_variant_formats() {
        let err = Error::Config("bad key".into());
        assert_eq!(err.to_string(), "configuration error: bad key");
    }

    #[test]
    fn index_not_found_variant_formats() {
        let err = Error::IndexNotFound {
            path: PathBuf::from("/tmp/idx"),
        };
        assert!(err.to_string().contains("/tmp/idx"));
    }

    #[test]
    fn index_corrupted_variant_formats() {
        let err = Error::IndexCorrupted("crc mismatch".into());
        assert_eq!(err.to_string(), "index corrupted: crc mismatch");
    }

    #[test]
    fn embedding_provider_variant_formats() {
        let err = Error::EmbeddingProvider("timeout".into());
        assert_eq!(err.to_string(), "embedding provider error: timeout");
    }

    #[test]
    fn markdown_parse_variant_formats() {
        let err = Error::MarkdownParse {
            path: PathBuf::from("doc.md"),
            message: "unexpected token".into(),
        };
        let s = err.to_string();
        assert!(s.contains("doc.md"));
        assert!(s.contains("unexpected token"));
    }

    #[test]
    fn io_from_conversion() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "gone");
        let err: Error = Error::from(io_err);
        assert!(matches!(err, Error::Io(_)));
        assert!(err.to_string().contains("gone"));
    }

    #[test]
    fn serialization_variant_formats() {
        let err = Error::Serialization("invalid json".into());
        assert_eq!(err.to_string(), "serialization error: invalid json");
    }

    #[test]
    fn watch_variant_formats() {
        let err = Error::Watch("inotify limit".into());
        assert_eq!(err.to_string(), "watch error: inotify limit");
    }

    #[test]
    fn lock_timeout_variant_formats() {
        let err = Error::LockTimeout;
        assert_eq!(err.to_string(), "lock acquisition timed out");
    }

    #[test]
    fn file_not_in_index_variant_formats() {
        let err = Error::FileNotInIndex {
            path: PathBuf::from("missing.md"),
        };
        assert_eq!(err.to_string(), "file not in index: missing.md");
    }

    #[test]
    fn index_already_exists_variant_formats() {
        let err = Error::IndexAlreadyExists {
            path: PathBuf::from("/tmp/index.bin"),
        };
        assert_eq!(err.to_string(), "index already exists: /tmp/index.bin");
    }

    #[test]
    fn config_already_exists_variant_formats() {
        let err = Error::ConfigAlreadyExists {
            path: PathBuf::from(".markdownvdb"),
        };
        assert_eq!(err.to_string(), "config already exists: .markdownvdb");
    }

    #[test]
    fn clustering_variant_formats() {
        let err = Error::Clustering("too few points".into());
        assert_eq!(err.to_string(), "clustering error: too few points");
    }

    #[test]
    fn link_graph_not_built_variant_formats() {
        let err = Error::LinkGraphNotBuilt;
        assert_eq!(
            err.to_string(),
            "link graph not built: run `mdvdb links` first"
        );
    }

    #[test]
    fn fts_variant_formats() {
        let err = Error::Fts("tokenization failed".into());
        assert_eq!(err.to_string(), "full-text search error: tokenization failed");
    }

    #[test]
    fn error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Error>();
    }
}
