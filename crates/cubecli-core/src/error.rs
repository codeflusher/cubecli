use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

/// Domain error for cubecli. All trait contracts in [`crate::traits`] return this,
/// so providers/runtime map their richer errors into these variants.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error(
        "not a cubecli project (no .cubecli.meta found in {searched:?} or any parent directory)"
    )]
    NotAProject { searched: Vec<PathBuf> },

    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid project metadata: {0}")]
    Meta(String),

    #[error("invalid value for `{key}`: {message}")]
    InvalidConfig { key: String, message: String },

    #[error("directory is not empty: {path}")]
    DirNotEmpty { path: PathBuf },

    #[error("invalid schedule rule `{value}`: {message}")]
    InvalidSchedule { value: String, message: String },

    #[error("{kind} addons are not supported on {core} servers")]
    IncompatibleAddon { kind: String, core: String },

    #[error("project schema migration failed: {0}")]
    Migration(String),

    #[error("no Java {feature} runtime found")]
    JavaMissing { feature: u32 },

    #[error("{service}: not found: {what}")]
    NotFound { service: &'static str, what: String },

    #[error("{service}: authentication required or invalid")]
    Auth { service: &'static str },

    #[error("{service}: rate limited")]
    RateLimited {
        service: &'static str,
        retry_after: Option<Duration>,
    },

    #[error("checksum mismatch for {file} (expected {algorithm} {expected}, got {actual})")]
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

    #[error("{service}: {message}")]
    External {
        service: &'static str,
        message: String,
    },

    #[error("runtime error: {0}")]
    Runtime(String),

    #[error("the Minecraft EULA has not been accepted yet")]
    EulaNotAccepted,
}

impl CoreError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn external(service: &'static str, message: impl Into<String>) -> Self {
        Self::External {
            service,
            message: message.into(),
        }
    }

    pub fn invalid_config(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidConfig {
            key: key.into(),
            message: message.into(),
        }
    }

    pub fn not_found(service: &'static str, what: impl Into<String>) -> Self {
        Self::NotFound {
            service,
            what: what.into(),
        }
    }

    /// Actionable next step for this error, shown to the user after the message.
    pub fn hint(&self) -> Option<String> {
        match self {
            Self::NotAProject { .. } => {
                Some("run `cubecli create` to start a project in an empty directory".into())
            }
            Self::EulaNotAccepted => Some(
                "read https://aka.ms/MinecraftEULA then run `cubecli project config eula=true`"
                    .into(),
            ),
            Self::JavaMissing { feature } => Some(format!(
                "run `cubecli setup java install {feature}` to download a JDK into ~/.cubecli/jdks"
            )),
            Self::Auth {
                service: "curseforge",
            } => Some("run `cubecli setup curseforge` to configure your CurseForge API key".into()),
            Self::RateLimited {
                service: "modrinth",
                ..
            } => Some("Modrinth allows 300 requests/min — retry in a moment".into()),
            Self::RateLimited { .. } => Some("rate limited — retry in a moment".into()),
            Self::ChecksumMismatch { .. } => {
                Some("the download is corrupted — retry; if it persists, check disk space".into())
            }
            Self::DirNotEmpty { .. } => {
                Some("choose an empty directory, or remove the files listed above".into())
            }
            Self::Meta(_) => Some("fix or restore the .cubecli.meta file".into()),
            Self::Migration(_) => Some("upgrade cubecli to work with this project".into()),
            Self::InvalidSchedule { .. } => {
                Some("examples: `6h`, `1d6h`, or a cron expression like `0 4 * * *`".into())
            }
            Self::InvalidConfig { .. } => {
                Some("run `cubecli project config` to list valid keys and formats".into())
            }
            Self::IncompatibleAddon { .. } => Some(
                "pick an addon kind that matches this server core (e.g. plugins for paper)".into(),
            ),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
