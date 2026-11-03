use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("rcon protocol error: {0}")]
    Rcon(String),

    #[error("io error at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("{0}")]
    Other(String),
}

impl From<RuntimeError> for cubecli_core::CoreError {
    fn from(err: RuntimeError) -> Self {
        match err {
            RuntimeError::Rcon(m) => Self::Runtime(format!("rcon: {m}")),
            RuntimeError::Io { path, source } => Self::Io { path, source },
            RuntimeError::Other(m) => Self::Runtime(m),
        }
    }
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

impl RuntimeError {
    pub fn io(path: impl Into<std::path::PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
