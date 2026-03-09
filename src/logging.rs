use tracing::Level;
use tracing_subscriber::EnvFilter;

use crate::error::Error;

/// Convert a verbosity count to a tracing [`Level`].
fn verbosity_to_level(verbosity: u8) -> Level {
    match verbosity {
        0 => Level::WARN,
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    }
}

/// Initialise a silent tracing subscriber that discards all output.
///
/// Used in JSON mode so that log lines never leak into stdout/stderr
/// and break structured output parsing.
pub fn init_silent() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("off"))
        .try_init()
        .map_err(|e| Error::Logging(e.to_string()))
}

/// Initialise the global tracing subscriber.
///
/// `verbosity` controls the default log level (0 = warn … 3+ = trace).
/// The `RUST_LOG` environment variable, when set, overrides the verbosity
/// flag entirely.
/// Logs always go to stderr so they never contaminate stdout (important for JSON mode).
pub fn init(verbosity: u8) -> Result<(), Error> {
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        let level = verbosity_to_level(verbosity);
        EnvFilter::new(level.to_string())
    };

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .try_init()
        .map_err(|e| Error::Logging(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbosity_0_is_warn() {
        assert_eq!(verbosity_to_level(0), Level::WARN);
    }

    #[test]
    fn verbosity_1_is_info() {
        assert_eq!(verbosity_to_level(1), Level::INFO);
    }

    #[test]
    fn verbosity_2_is_debug() {
        assert_eq!(verbosity_to_level(2), Level::DEBUG);
    }

    #[test]
    fn verbosity_3_is_trace() {
        assert_eq!(verbosity_to_level(3), Level::TRACE);
    }

    #[test]
    fn verbosity_high_is_trace() {
        assert_eq!(verbosity_to_level(255), Level::TRACE);
    }
}
