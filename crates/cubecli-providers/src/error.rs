use std::time::Duration;

use thiserror::Error;

/// Provider-layer error, mapped into [`cubecli_core::CoreError`] at the
/// trait boundary (hints are attached there).
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("http error: {0}")]
    Http(String),

    #[error("{service}: not found: {what}")]
    NotFound { service: &'static str, what: String },

    #[error("{service}: authentication required or invalid")]
    Auth { service: &'static str },

    #[error("{service}: rate limited")]
    RateLimited {
        service: &'static str,
        retry_after: Option<Duration>,
    },

    #[error("checksum mismatch for {file} ({algorithm}: expected {expected}, got {actual})")]
    ChecksumMismatch {
        file: String,
        algorithm: &'static str,
        expected: String,
        actual: String,
    },

    #[error("{service}: unsupported: {reason}")]
    Unsupported {
        service: &'static str,
        reason: String,
    },

    #[error("io error at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("{0}")]
    Other(String),
}

impl ProviderError {
    pub fn not_found(service: &'static str, what: impl Into<String>) -> Self {
        Self::NotFound {
            service,
            what: what.into(),
        }
    }

    pub fn unsupported(service: &'static str, reason: impl Into<String>) -> Self {
        Self::Unsupported {
            service,
            reason: reason.into(),
        }
    }

    pub fn io(path: impl Into<std::path::PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<ProviderError> for cubecli_core::CoreError {
    fn from(err: ProviderError) -> Self {
        match err {
            ProviderError::Http(m) => Self::External {
                service: "network",
                message: m,
            },
            ProviderError::NotFound { service, what } => Self::NotFound { service, what },
            ProviderError::Auth { service } => Self::Auth { service },
            ProviderError::RateLimited {
                service,
                retry_after,
            } => Self::RateLimited {
                service,
                retry_after,
            },
            ProviderError::ChecksumMismatch {
                file,
                algorithm,
                expected,
                actual,
            } => Self::ChecksumMismatch {
                file,
                algorithm,
                expected,
                actual,
            },
            ProviderError::Unsupported { service, reason } => Self::Unsupported { service, reason },
            ProviderError::Io { path, source } => Self::Io { path, source },
            ProviderError::Other(m) => Self::External {
                service: "network",
                message: m,
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, ProviderError>;
