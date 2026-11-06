//! The native process-manager backend.

pub mod pidfile;
pub mod signals;
pub mod spawn;
pub mod tail;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use cubecli_core::error::{CoreError, Result};
use cubecli_core::model::JavaRequirement;
use cubecli_core::project::ProjectHandle;
use cubecli_core::traits::java::JavaLocator;
use cubecli_core::traits::runtime::{
    RuntimeBackend, RuntimeId, RuntimeStatus, StopGrace, StopOutcome,
};

use crate::error::{Result as RtResult, RuntimeError};
use crate::stop;

pub struct NativeBackend {
    pub locator: Arc<dyn JavaLocator>,
}

impl NativeBackend {
    pub fn new(locator: Arc<dyn JavaLocator>) -> Self {
        Self { locator }
    }

    fn rcon_addr(layout: &Path) -> RtResult<std::net::SocketAddr> {
        let port = properties_port(&layout.join("server.properties")).unwrap_or(25575);
        Ok(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
    }
}

/// Connect to a project's RCON console (loopback, per server.properties).
pub async fn rcon_client(project: &ProjectHandle) -> RtResult<crate::rcon::RconClient> {
    let layout = project.layout();
    let secret = std::fs::read_to_string(layout.rcon_secret_file())
        .map_err(|e| RuntimeError::Io {
            path: layout.rcon_secret_file(),
            source: e,
        })?
        .trim()
        .to_string();
    let port = properties_port(&layout.root().join("server.properties")).unwrap_or(25575);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    crate::rcon::connect(addr, &secret).await
}

/// Read `rcon.port` out of server.properties (the running server's truth).
pub fn properties_port(properties_path: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(properties_path).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("rcon.port=") {
            return rest.trim().parse().ok();
        }
    }
    None
}

#[async_trait]
impl RuntimeBackend for NativeBackend {
    fn id(&self) -> RuntimeId {
        RuntimeId::Native
    }

    async fn start(&self, project: &ProjectHandle) -> Result<RuntimeStatus> {
        let status = self.status(project).await?;
        if status.is_running() {
            return Ok(status);
        }
        let meta = project.meta();
        if !meta.server.eula_accepted {
            return Err(CoreError::EulaNotAccepted);
        }
        let launch = meta
            .server
            .launch
            .clone()
            .ok_or_else(|| CoreError::Runtime("server is not provisioned yet".into()))?;
        if meta.server.java_version == 0 {
            return Err(CoreError::Runtime(
                "java version unknown — provisioning is incomplete".into(),
            ));
        }

        // Keep managed files consistent before boot (idempotent).
        cubecli_core::managed_files::write_eula(project.layout(), true)?;
        cubecli_core::managed_files::sync_server_properties(meta, project.layout())?;

        let java = self
            .locator
            .locate(JavaRequirement {
                major: meta.server.java_version,
            })
            .map_err(|e| {
                CoreError::Runtime(format!(
                    "cannot start: {e} — hint: {}",
                    e.hint().unwrap_or_default()
                ))
            })?;

        let argv = launch.argv(&java.java_bin);
        // Explicit max-heap wins over provider-authored flags. Jar launches
        // only — ArgFile JVM flags live in user_jvm_args.txt (rewritten by
        // managed_files::apply_max_heap), and Script launches are opaque.
        let argv = if matches!(launch, cubecli_core::model::LaunchSpec::Jar { .. }) {
            let merged =
                cubecli_core::jvm::merge_jvm_args(&argv[1..], meta.runtime.max_heap.as_deref());
            std::iter::once(argv[0].clone())
                .chain(merged)
                .collect::<Vec<String>>()
        } else {
            argv
        };
        let pid = spawn::spawn_detached(
            &java.java_bin,
            &argv,
            project.root(),
            &project.layout().server_log(),
        )?;

        let started_at = chrono::Utc::now();
        pidfile::write(
            &project.layout().pidfile(),
            &pidfile::PidFile {
                pid,
                started_at,
                argv,
                java_feature: java.feature,
            },
        )?;
        stop::clear_manual_stop(project.layout());

        Ok(RuntimeStatus::Running {
            pid: Some(pid),
            started_at: Some(started_at),
        })
    }

    async fn stop(&self, project: &ProjectHandle, grace: StopGrace) -> Result<StopOutcome> {
        stop::graceful(project.layout(), grace, true)
            .await
            .map_err(CoreError::from)
    }

    async fn restart(&self, project: &ProjectHandle) -> Result<RuntimeStatus> {
        // Not a manual stop: restart is daemon-initiated (cron) or a user
        // convenience — it must not leave the manually-stopped marker down,
        // or a failed start would read as "user stopped it" and the daemon
        // would never recover the server.
        stop::graceful(project.layout(), StopGrace::default(), false)
            .await
            .map_err(CoreError::from)?;
        self.start(project).await
    }

    /// RCON probe is the authority; the pidfile is a hint (boot detection,
    /// signal target) and a dead pid under an existing pidfile means a crash.
    async fn status(&self, project: &ProjectHandle) -> Result<RuntimeStatus> {
        let layout = project.layout();
        let pf = pidfile::read(&layout.pidfile())?;

        let Some(pf) = pf else {
            return Ok(RuntimeStatus::Stopped { clean: true });
        };

        if signals::pid_alive(pf.pid) {
            // Probe RCON (short timeout) — works whether booting or up.
            if let Ok(secret) = std::fs::read_to_string(layout.rcon_secret_file()) {
                let addr = Self::rcon_addr(layout.root())?;
                let probe = tokio::time::timeout(
                    Duration::from_secs(1),
                    crate::rcon::connect(addr, secret.trim()),
                )
                .await;
                let _ = probe; // result irrelevant: alive is alive
            }
            return Ok(RuntimeStatus::Running {
                pid: Some(pf.pid),
                started_at: Some(pf.started_at),
            });
        }

        Ok(RuntimeStatus::Stopped { clean: false })
    }

    async fn tail_logs(
        &self,
        project: &ProjectHandle,
        tx: mpsc::Sender<String>,
        cancel: CancellationToken,
    ) -> Result<()> {
        tail::follow(project.layout().server_log(), tx, cancel)
            .await
            .map_err(CoreError::from)
    }

    async fn read_logs(&self, project: &ProjectHandle, lines: usize) -> Result<Vec<String>> {
        tail::last_lines(&project.layout().server_log(), lines).map_err(CoreError::from)
    }
}
