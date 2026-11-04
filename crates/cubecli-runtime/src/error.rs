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

    /// Docker is unusable in a way the user can fix; `hint` says how.
    #[error("docker is unavailable: {message} — {hint}")]
    Docker { message: String, hint: &'static str },
}

/// Hint for a machine where the `docker` CLI itself is missing.
pub const DOCKER_CLI_HINT: &str =
    "install Docker (Desktop on macOS/Windows with WSL integration, or Docker Engine on Linux)";
/// Hint for a machine with the CLI but an unreachable daemon.
pub const DOCKER_DAEMON_HINT: &str =
    "start Docker Desktop (or `systemctl start docker`) and check DOCKER_HOST / `docker context`";
/// Hint for a machine with a docker CLI too old for the compose v2 plugin.
pub const DOCKER_PLUGIN_HINT: &str =
    "install Docker Desktop or the compose plugin — `docker compose version` must succeed";

impl From<RuntimeError> for cubecli_core::CoreError {
    fn from(err: RuntimeError) -> Self {
        match err {
            RuntimeError::Rcon(m) => Self::Runtime(format!("rcon: {m}")),
            RuntimeError::Io { path, source } => Self::Io { path, source },
            RuntimeError::Docker { message, hint } => {
                Self::Runtime(format!("docker is unavailable: {message} — {hint}"))
            }
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
