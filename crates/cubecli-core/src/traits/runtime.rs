//! Runtime backends — native process manager and docker compose.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::project::ProjectHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeId {
    Native,
    Compose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    Running {
        pid: Option<u32>,
        started_at: Option<DateTime<Utc>>,
    },
    Stopped {
        /// false when the process died without a clean stop (crash).
        clean: bool,
    },
    Unknown,
}

impl RuntimeStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

/// Grace periods for the stop sequence.
#[derive(Debug, Clone, Copy)]
pub struct StopGrace {
    /// How long to wait for the RCON `stop` to take effect.
    pub soft: Duration,
    /// How long SIGTERM gets before SIGKILL.
    pub term: Duration,
}

impl Default for StopGrace {
    fn default() -> Self {
        Self {
            soft: Duration::from_secs(30),
            term: Duration::from_secs(10),
        }
    }
}

/// Which stage of the stop sequence did the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopStage {
    AlreadyStopped,
    /// RCON `stop` worked (the clean path).
    Rcon,
    /// Needed SIGTERM.
    Signal,
    /// Needed SIGKILL.
    Kill,
}

#[derive(Debug, Clone)]
pub struct StopOutcome {
    pub stage: StopStage,
    pub waited: Duration,
}

#[async_trait]
pub trait RuntimeBackend: Send + Sync {
    fn id(&self) -> RuntimeId;

    /// Idempotent — starting a running server reports its status.
    async fn start(&self, project: &ProjectHandle) -> Result<RuntimeStatus>;

    /// RCON `stop` → grace → SIGTERM process group → SIGKILL.
    async fn stop(&self, project: &ProjectHandle, grace: StopGrace) -> Result<StopOutcome>;

    async fn restart(&self, project: &ProjectHandle) -> Result<RuntimeStatus>;

    /// RCON auth probe is the authority; the pidfile is only a hint.
    async fn status(&self, project: &ProjectHandle) -> Result<RuntimeStatus>;

    /// Streams log lines until `cancel` fires.
    async fn tail_logs(
        &self,
        project: &ProjectHandle,
        tx: mpsc::Sender<String>,
        cancel: CancellationToken,
    ) -> Result<()>;
}
