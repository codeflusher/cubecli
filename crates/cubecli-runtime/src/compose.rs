//! Docker compose backend: render `compose.yaml` from project metadata and
//! drive `docker compose` through the same [`RuntimeBackend`] trait.
//!
//! The plain JDK image mounts the project dir — identical layout to the
//! native backend, so addon installs and saves work unchanged. RCON reaches
//! the server over the loopback mapping `127.0.0.1:{rcon}:{rcon}` (the
//! in-container server listens on the same `rcon.port` the host writes into
//! the bind-mounted `server.properties`).

use std::path::Path;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use cubecli_core::error::{CoreError, Result};
use cubecli_core::model::{LaunchSpec, RestartPolicy};
use cubecli_core::project::ProjectHandle;
use cubecli_core::traits::runtime::{
    RuntimeBackend, RuntimeId, RuntimeStatus, StopGrace, StopOutcome, StopStage,
};

use crate::error::{
    Result as RtResult, RuntimeError, DOCKER_CLI_HINT, DOCKER_DAEMON_HINT, DOCKER_PLUGIN_HINT,
};
use crate::stop::{clear_manual_stop, write_manual_stop};

pub const COMPOSE_FILE: &str = "compose.yaml";

const SIG_PREFIX: &str = "# cubecli: generated (sig ";

// ------------------------------------------------------------------------------------------------
// rendering

/// Render compose.yaml with a freshness signature on the first line: start
/// and restart regenerate the file when the signature drifts from the
/// current metadata, and leave it untouched when the line is gone
/// (hand-edited). `uid`/`gid` keep world files owned by the host user
/// (otherwise the container's root chowns everything).
pub fn render(meta: &cubecli_core::ProjectMeta, root: &Path) -> Result<String> {
    let body = render_body(meta, root)?;
    let sig = format!("{:016x}", fnv1a(&body));
    Ok(format!(
        "{SIG_PREFIX}{sig}) — auto-refreshed on start; delete this line to pin hand edits\n{body}"
    ))
}

fn render_body(meta: &cubecli_core::ProjectMeta, root: &Path) -> Result<String> {
    let java = if meta.server.java_version > 0 {
        meta.server.java_version.to_string()
    } else {
        return Err(CoreError::Runtime(
            "java version unknown — provision the project first".into(),
        ));
    };
    let launch = meta
        .server
        .launch
        .clone()
        .ok_or_else(|| CoreError::Runtime("server is not provisioned yet".into()))?;
    let command = match &launch {
        LaunchSpec::Jar {
            jar,
            jvm_args,
            args,
        } => {
            // Explicit max-heap config wins over provider-authored flags;
            // without it, the provider's defaults stand. (ArgFile JVM flags
            // live in user_jvm_args.txt, rewritten by
            // managed_files::apply_max_heap — not here.)
            let parts =
                cubecli_core::jvm::merge_jvm_args(jvm_args, meta.runtime.max_heap.as_deref());
            format!(
                "{} -jar {} {}",
                parts.join(" "),
                shell_quote(jar),
                args.join(" ")
            )
            .trim()
            .to_string()
        }
        LaunchSpec::ArgFile {
            user_jvm_args,
            argfile,
            args,
        } => format!(
            "@{} @{} {}",
            shell_quote(user_jvm_args),
            shell_quote(&container_path(argfile)),
            args.join(" ")
        )
        .trim()
        .to_string(),
        LaunchSpec::Script { script, args } => {
            // The container image is linux: a project provisioned on a
            // Windows host persists run.bat / windows_args.txt in its launch
            // spec — the installers emit both variants, so swap to the unix
            // pair for the container.
            format!("sh {} {}", shell_quote(&container_path(script)), args.join(" "))
        }
    };
    let command = format!("java {command}").replace('"', "'");

    let restart = match meta.runtime.restart_policy {
        RestartPolicy::ManualOnly => "no",
        RestartPolicy::UnlessStopped => "unless-stopped",
        // The daemon drives cron restarts; compose stays off.
        RestartPolicy::Cron => "no",
    };

    let uid = current_uid();
    let gid = current_gid();

    Ok(format!(
        r#"services:
  {slug}:
    image: eclipse-temurin:{java}-jre
    container_name: cubecli-{slug}
    working_dir: /data
    user: "{uid}:{gid}"
    volumes:
      - {root}:/data
    command: "{command}"
    ports:
      - "{port}:{port}"
      - "127.0.0.1:{rcon}:{rcon}"
    stop_grace_period: 90s
    restart: {restart}
"#,
        slug = meta.project.slug,
        root = root.display(),
        port = meta.runtime.port,
        rcon = meta.runtime.rcon_port,
    ))
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./=@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{s}'")
    }
}

/// Map a Windows-only launch artifact to the unix twin the container needs
/// (Forge/NeoForge installers emit both variants on every host).
fn container_path(path: &str) -> String {
    if let Some(stripped) = path.strip_suffix(".bat") {
        return format!("{stripped}.sh");
    }
    if let Some(stripped) = path.strip_suffix("windows_args.txt") {
        return format!("{stripped}unix_args.txt");
    }
    path.to_string()
}

/// FNV-1a — stable across Rust releases (std's DefaultHasher is not), which
/// a signature persisted in a user-visible file requires. Change detection
/// only; not cryptographic.
fn fnv1a(data: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The signature stamped on an existing compose.yaml, if it has one.
pub fn on_disk_sig(text: &str) -> Option<String> {
    let first = text.lines().next()?;
    let rest = first.strip_prefix(SIG_PREFIX)?;
    let sig = rest.split(')').next()?.trim();
    (!sig.is_empty()).then(|| sig.to_string())
}

/// Is the on-disk compose.yaml hand-edited (signature line removed)?
pub fn is_hand_edited(project: &ProjectHandle) -> bool {
    std::fs::read_to_string(project.root().join(COMPOSE_FILE))
        .map(|text| on_disk_sig(&text).is_none())
        .unwrap_or(false)
}

/// What [`ensure_fresh`] did about the on-disk compose.yaml.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// File was missing — written.
    Written,
    /// Signature matches current metadata — untouched.
    Current,
    /// Signature drifted (upgrade, config change) — regenerated.
    Regenerated,
    /// No signature line (hand-edited) — kept as-is.
    KeptManual,
}

/// Bring compose.yaml in line with the project metadata before an up.
pub fn ensure_fresh(project: &ProjectHandle) -> Result<Freshness> {
    let path = project.root().join(COMPOSE_FILE);
    let rendered = render(project.meta(), project.root())?;
    let fresh_sig = on_disk_sig(&rendered)
        .expect("render() always stamps a signature");

    let outcome = match std::fs::read_to_string(&path) {
        Err(_) => {
            write_compose(&path, &rendered)?;
            Freshness::Written
        }
        Ok(existing) => match on_disk_sig(&existing) {
            None => Freshness::KeptManual,
            Some(sig) if sig == fresh_sig => Freshness::Current,
            Some(_) => {
                write_compose(&path, &rendered)?;
                Freshness::Regenerated
            }
        },
    };
    Ok(outcome)
}

fn write_compose(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|e| CoreError::io(path, e))
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: plain syscall wrapper.
    unsafe { libc::getuid() }
}

#[cfg(unix)]
fn current_gid() -> u32 {
    // SAFETY: plain syscall wrapper.
    unsafe { libc::getgid() }
}

// Windows host support lands later; the linux container does not care about
// host uid/gid (Docker Desktop bind mounts ignore them).
#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

#[cfg(not(unix))]
fn current_gid() -> u32 {
    0
}

// ------------------------------------------------------------------------------------------------
// `docker compose ps` parsing

/// One entry of `docker compose ps --format json --all` (tolerant of the
/// NDJSON-vs-array split across compose versions and of missing fields).
#[derive(Debug, Clone, PartialEq)]
pub struct ComposeContainer {
    /// Lowercased docker state: running, exited, paused, dead, …
    pub state: String,
    pub exit_code: Option<i64>,
    pub started_at: Option<DateTime<Utc>>,
}

pub fn parse_ps(stdout: &str) -> Vec<ComposeContainer> {
    // One JSON object per line (newer compose), or a single JSON array
    // (older). Pretty-printed arrays fail the per-line parse and fall
    // through to the array branch.
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .chain(
            serde_json::from_str::<serde_json::Value>(stdout)
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .into_iter(),
        )
        .filter_map(|entry| {
            Some(ComposeContainer {
                state: entry
                    .get("State")
                    .and_then(|s| s.as_str())?
                    .to_ascii_lowercase(),
                exit_code: entry.get("ExitCode").and_then(|c| c.as_i64()),
                started_at: entry
                    .get("StartedAt")
                    .and_then(|s| s.as_str())
                    .and_then(|s| {
                        DateTime::parse_from_rfc3339(s)
                            .ok()
                            .map(|d| d.with_timezone(&Utc))
                    }),
            })
        })
        .collect()
}

/// The trait-level status of a ps listing. Crash truth lives in `ExitCode`:
/// `docker compose stop` exits 0 regardless of how the server died, so an
/// exited-nonzero (or dead) container is the only unclean evidence there is.
fn status_of(containers: &[ComposeContainer]) -> RuntimeStatus {
    if containers
        .iter()
        .any(|c| matches!(c.state.as_str(), "running" | "paused" | "restarting"))
    {
        return RuntimeStatus::Running {
            pid: None,
            started_at: containers.iter().filter_map(|c| c.started_at).max(),
        };
    }
    let crashed = containers.iter().any(|c| match c.state.as_str() {
        "dead" => true,
        "exited" => c.exit_code.map(|code| code != 0).unwrap_or(false),
        _ => false,
    });
    RuntimeStatus::Stopped { clean: !crashed }
}

/// Status straight from raw `docker compose ps --format json --all` output —
/// the pure seam `ComposeBackend::status` is built on.
pub fn status_from_ps(stdout: &str) -> RuntimeStatus {
    status_of(&parse_ps(stdout))
}

// ------------------------------------------------------------------------------------------------
// the backend

/// Drive `docker compose -p <slug> -f compose.yaml` in the project dir.
pub struct ComposeBackend;

impl ComposeBackend {
    fn docker(project: &ProjectHandle, args: &[&str]) -> tokio::process::Command {
        let exe = std::env::var_os("CUBECLI_DOCKER_BIN").unwrap_or_else(|| "docker".into());
        let mut cmd = tokio::process::Command::new(exe);
        cmd.args(["compose", "-p", &project.meta().project.slug])
            .args(args)
            .current_dir(project.root());
        cmd
    }

    /// Bare docker CLI (no compose subcommand) — used by the preflight probe.
    fn docker_cli(project: &ProjectHandle, args: &[&str]) -> tokio::process::Command {
        let exe = std::env::var_os("CUBECLI_DOCKER_BIN").unwrap_or_else(|| "docker".into());
        let mut cmd = tokio::process::Command::new(exe);
        cmd.args(args).current_dir(project.root());
        cmd
    }

    fn spawn_error(err: std::io::Error) -> RuntimeError {
        if err.kind() == std::io::ErrorKind::NotFound {
            RuntimeError::Docker {
                message: "docker CLI not found on PATH".into(),
                hint: DOCKER_CLI_HINT,
            }
        } else {
            RuntimeError::Other(format!("cannot run docker: {err}"))
        }
    }

    fn stderr_error(args: &[&str], stderr: &str) -> RuntimeError {
        // Docker CLI / Desktop phrasings for "the daemon is not up".
        let lowered = stderr.to_ascii_lowercase();
        if lowered.contains("cannot connect to the docker daemon")
            || lowered.contains("daemon is not running")
            || lowered.contains("failed to connect to the docker api")
            || lowered.contains("is the docker daemon running")
        {
            RuntimeError::Docker {
                message: "docker daemon unreachable".into(),
                hint: DOCKER_DAEMON_HINT,
            }
        } else if lowered.contains("unknown shorthand flag: 'p'") {
            // A docker CLI without the compose v2 plugin parses the global
            // `-p` (project name) flag and dies with this cryptic message.
            RuntimeError::Docker {
                message: "the docker compose plugin is not available".into(),
                hint: DOCKER_PLUGIN_HINT,
            }
        } else {
            RuntimeError::Other(format!(
                "docker {} failed: {}",
                args.join(" "),
                stderr.trim()
            ))
        }
    }

    async fn run(project: &ProjectHandle, args: &[&str]) -> RtResult<String> {
        let output = Self::docker(project, args)
            .output()
            .await
            .map_err(Self::spawn_error)?;
        if !output.status.success() {
            return Err(Self::stderr_error(
                args,
                &String::from_utf8_lossy(&output.stderr),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Cheap daemon + compose-plugin reachability check before mutating
    /// operations, so the user gets an actionable error instead of a
    /// half-applied change or a cryptic CLI parse error.
    async fn preflight(project: &ProjectHandle) -> RtResult<()> {
        let output = Self::docker_cli(project, &["info", "--format", "{{.ServerVersion}}"])
            .output()
            .await
            .map_err(Self::spawn_error)?;
        if !output.status.success() {
            return Err(Self::stderr_error(
                &["info"],
                &String::from_utf8_lossy(&output.stderr),
            ));
        }
        Self::require_compose_plugin(project).await
    }

    /// `docker compose` must be a working subcommand — a bare docker CLI
    /// without the compose v2 plugin fails later with "unknown shorthand
    /// flag: 'p'", which helps nobody.
    async fn require_compose_plugin(project: &ProjectHandle) -> RtResult<()> {
        let output = Self::docker_cli(project, &["compose", "version"])
            .output()
            .await
            .map_err(Self::spawn_error)?;
        if !output.status.success() {
            return Err(RuntimeError::Docker {
                message: "the docker compose plugin is not available".into(),
                hint: DOCKER_PLUGIN_HINT,
            });
        }
        Ok(())
    }

    fn note_freshness(fresh: Freshness) {
        match fresh {
            Freshness::Regenerated => {
                tracing::info!("compose.yaml regenerated (project metadata changed)");
            }
            Freshness::KeptManual => {
                tracing::warn!(
                    "hand-edited compose.yaml kept — `cubecli project compose` regenerates it"
                );
            }
            Freshness::Written | Freshness::Current => {}
        }
    }

    fn stop_timeout(grace: StopGrace) -> String {
        format!("-t{}", grace.soft.as_secs() + grace.term.as_secs() + 5)
    }

    /// Client-side validation of the exported file (`docker compose config`
    /// needs no daemon). Public because `project compose` surfaces it.
    pub async fn validate(project: &ProjectHandle) -> RtResult<()> {
        Self::require_compose_plugin(project).await?;
        let output = Self::docker(project, &["config", "--quiet"])
            .output()
            .await
            .map_err(Self::spawn_error)?;
        if !output.status.success() {
            return Err(RuntimeError::Other(format!(
                "compose.yaml failed validation: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl RuntimeBackend for ComposeBackend {
    fn id(&self) -> RuntimeId {
        RuntimeId::Compose
    }

    async fn start(&self, project: &ProjectHandle) -> Result<RuntimeStatus> {
        if !project.meta().server.eula_accepted {
            return Err(CoreError::EulaNotAccepted);
        }
        cubecli_core::managed_files::write_eula(project.layout(), true)?;
        cubecli_core::managed_files::sync_server_properties(project.meta(), project.layout())?;
        Self::preflight(project).await?;
        Self::note_freshness(ensure_fresh(project)?);
        Self::run(project, &["up", "-d"]).await?;
        clear_manual_stop(project.layout());
        self.status(project).await
    }

    async fn stop(&self, project: &ProjectHandle, grace: StopGrace) -> Result<StopOutcome> {
        Self::preflight(project).await?;
        let was_running = self.status(project).await?.is_running();
        if !was_running {
            write_manual_stop(project.layout());
            return Ok(StopOutcome {
                stage: StopStage::AlreadyStopped,
                waited: Duration::ZERO,
            });
        }
        // The marker goes down FIRST: the daemon ticks while this stop runs
        // and must not "crash-restart" the server we are deliberately
        // stopping (same invariant as the native stop ladder).
        write_manual_stop(project.layout());
        let started = Instant::now();
        Self::run(project, &["stop", &Self::stop_timeout(grace)]).await?;
        // docker SIGTERMs the container's main process; the JVM's shutdown
        // hooks save the world.
        Ok(StopOutcome {
            stage: StopStage::Signal,
            waited: started.elapsed(),
        })
    }

    async fn restart(&self, project: &ProjectHandle) -> Result<RuntimeStatus> {
        Self::preflight(project).await?;
        Self::note_freshness(ensure_fresh(project)?);
        // stop + up (not `docker compose restart`): up recreates containers
        // whose rendered config changed — the native stop+start semantic.
        if self.status(project).await?.is_running() {
            Self::run(project, &["stop", &Self::stop_timeout(StopGrace::default())]).await?;
        }
        Self::run(project, &["up", "-d"]).await?;
        clear_manual_stop(project.layout());
        self.status(project).await
    }

    async fn status(&self, project: &ProjectHandle) -> Result<RuntimeStatus> {
        let out = Self::run(project, &["ps", "--format", "json", "--all"]).await?;
        Ok(status_of(&parse_ps(&out)))
    }

    async fn tail_logs(
        &self,
        project: &ProjectHandle,
        tx: mpsc::Sender<String>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let mut child = Self::docker(project, &["logs", "-f", "--tail", "0"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| RuntimeError::Other(format!("docker compose logs: {e}")))?;
        let Some(stdout) = child.stdout.take() else {
            return Ok(());
        };
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => { let _ = child.kill().await; return Ok(()); }
                line = lines.next_line() => match line {
                    Ok(Some(l)) => { if tx.send(l).await.is_err() { let _ = child.kill().await; return Ok(()); } }
                    _ => return Ok(()),
                }
            }
        }
    }

    async fn read_logs(&self, project: &ProjectHandle, lines: usize) -> Result<Vec<String>> {
        let out = Self::run(project, &["logs", "--tail", &lines.to_string()]).await?;
        Ok(out.lines().map(str::to_string).collect())
    }
}

/// Compose export writes (or refreshes) `compose.yaml`; also flips the
/// backend marker so the daemon uses the right driver.
pub fn export(project: &ProjectHandle) -> Result<std::path::PathBuf> {
    let yaml = render(project.meta(), project.root())?;
    let path = project.root().join(COMPOSE_FILE);
    std::fs::write(&path, yaml).map_err(|e| CoreError::io(&path, e))?;
    Ok(path)
}
