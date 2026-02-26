pub mod chunker;
pub mod clustering;
pub mod config;
pub mod discovery;
pub mod embedding;
pub mod error;
pub mod index;
pub mod logging;
pub mod parser;
pub mod schema;
pub mod search;

pub use error::Error;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
