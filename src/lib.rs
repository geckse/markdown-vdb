pub mod chunker;
pub mod config;
pub mod discovery;
pub mod error;
pub mod logging;
pub mod parser;

pub use error::Error;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
