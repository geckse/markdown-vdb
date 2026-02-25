pub mod config;
pub mod error;
pub mod logging;

pub use error::Error;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
