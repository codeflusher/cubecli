//! `cubecli doctor`: environment and project diagnostics. Read-only — every
//! check carries an actionable hint, and the exit code is 1 when any check
//! fails.

use std::time::Duration;

use cubecli_core::model::Backend;
use cubecli_core::paths::ProjectLayout;
use cubecli_core::project::ProjectHandle;
use cubecli_core::traits::java::{JavaLocator, JavaOrigin};
use cubecli_core::traits::runtime::{RuntimeBackend, RuntimeStatus};

use crate::context::AppContext;
use crate::exit::UserError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => " ok ",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Debug)]
pub struct Check {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    pub hint: Option<String>,
}

pub async fn doctor(ctx: &AppContext) -> Result<(), UserError> {
    let project = ProjectHandle::discover(".").ok();
    let checks = build_checks(ctx, project.as_ref()).await;
    for line in render(&checks).lines() {
        println!("{line}");
    }
    let failed = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();
    if failed > 0 {
        Err(UserError::new(format!(
            "{failed} check(s) failed — see above"
        )))
    } else {
        Ok(())
    }
}

async fn build_checks(ctx: &AppContext, project: Option<&ProjectHandle>) -> Vec<Check> {
    let mut checks = Vec::new();
    checks.push(docker_check().await);
    checks.push(wsl_check());
    checks.push(java_check(ctx));
    checks.push(daemon_check().await);
    if let Some(handle) = project {
        checks.extend(project_checks(ctx, handle).await);
    } else {
        checks.push(Check {
            name: "project",
            status: CheckStatus::Ok,
            detail: "no project in this directory (run inside a server dir for project checks)"
                .into(),
            hint: None,
        });
    }
    checks
}

async fn docker_check() -> Check {
    let probe = cubecli_runtime::env::docker_probe().await;
    let wsl_suffix = if cubecli_runtime::env::is_wsl() {
        " — inside WSL, enable Docker Desktop's WSL integration"
    } else {
        ""
    };
    if !probe.cli_found {
        return Check {
            name: "docker",
            status: CheckStatus::Fail,
            detail: "docker CLI not found on PATH".into(),
            hint: Some(format!(
                "install Docker (Desktop on macOS/Windows, Docker Engine on Linux){wsl_suffix}"
            )),
        };
    }
    let context = probe
        .context
        .map(|c| format!(" (context {c})"))
        .unwrap_or_default();
    if !probe.daemon_ok {
        return Check {
            name: "docker",
            status: CheckStatus::Fail,
            detail: format!("CLI found but daemon unreachable{context}"),
            hint: Some(format!(
                "start Docker Desktop (or `systemctl start docker`) and check DOCKER_HOST{wsl_suffix}"
            )),
        };
    }
    Check {
        name: "docker",
        status: CheckStatus::Ok,
        detail: format!(
            "daemon up{}{}",
            probe
                .server_version
                .map(|v| format!(" {v}"))
                .unwrap_or_default(),
            context
        ),
        hint: None,
    }
}

fn wsl_check() -> Check {
    match cubecli_runtime::env::wsl_version() {
        Some(2) => Check {
            name: "wsl",
            status: CheckStatus::Ok,
            detail: "WSL2 detected".into(),
            hint: None,
        },
        Some(1) => Check {
            name: "wsl",
            status: CheckStatus::Warn,
            detail: "WSL1 detected — Docker Desktop integration requires WSL2".into(),
            hint: Some("convert the distro: `wsl --set-version <distro> 2`".into()),
        },
        _ => Check {
            name: "wsl",
            status: CheckStatus::Ok,
            detail: "not WSL (native platform)".into(),
            hint: None,
        },
    }
}

fn java_check(ctx: &AppContext) -> Check {
    let installs = ctx.java.all();
    if installs.is_empty() {
        return Check {
            name: "java",
            status: CheckStatus::Ok,
            detail: "no JDKs found — the right one is auto-installed on first start".to_string(),
            hint: Some("pre-download one: `cubecli setup java install 21`".into()),
        };
    }
    let owned = installs
        .iter()
        .filter(|j| j.origin == JavaOrigin::Owned)
        .count();
    Check {
        name: "java",
        status: CheckStatus::Ok,
        detail: format!(
            "{} JDK(s) found ({} owned in ~/.cubecli/jdks)",
            installs.len(),
            owned
        ),
        hint: None,
    }
}

async fn daemon_check() -> Check {
    let kind = cubecli_runtime::service::detect();
    if kind == "unsupported" {
        return Check {
            name: "daemon",
            status: CheckStatus::Warn,
            detail: "no service mechanism on this platform yet".into(),
            hint: Some("run `cubecli daemon run` in a terminal/screen instead".into()),
        };
    }
    if kind == "systemd" && !cubecli_runtime::env::systemd_available() {
        return Check {
            name: "daemon",
            status: CheckStatus::Warn,
            detail: "systemd user session not available".into(),
            hint: Some(
                "inside WSL: add `[boot] systemd=true` to /etc/wsl.conf, then `wsl --shutdown`"
                    .into(),
            ),
        };
    }
    let path = cubecli_runtime::service::installed_path();
    if cubecli_runtime::service::is_installed() {
        Check {
            name: "daemon",
            status: CheckStatus::Ok,
            detail: format!("installed as {kind} service: {}", path.display()),
            hint: None,
        }
    } else {
        Check {
            name: "daemon",
            status: CheckStatus::Warn,
            detail: "not installed — crash restarts and scheduled backups are off".into(),
            hint: Some("run `cubecli daemon install`".into()),
        }
    }
}

async fn project_checks(ctx: &AppContext, handle: &ProjectHandle) -> Vec<Check> {
    let meta = handle.meta();
    let mut checks = Vec::new();

    if meta.server.eula_accepted {
        checks.push(Check {
            name: "project/eula",
            status: CheckStatus::Ok,
            detail: "accepted".into(),
            hint: None,
        });
    } else {
        checks.push(Check {
            name: "project/eula",
            status: CheckStatus::Fail,
            detail: "NOT accepted — `project start` will refuse".into(),
            hint: Some("run `cubecli project config eula=true`".into()),
        });
    }

    if meta.runtime.port == meta.runtime.rcon_port {
        checks.push(Check {
            name: "project/ports",
            status: CheckStatus::Fail,
            detail: format!("game port {} equals rcon port", meta.runtime.port),
            hint: Some("run `cubecli project config port=<other port>`".into()),
        });
    } else {
        checks.push(Check {
            name: "project/ports",
            status: CheckStatus::Ok,
            detail: format!(
                "game {}, rcon {}",
                meta.runtime.port, meta.runtime.rcon_port
            ),
            hint: None,
        });
    }

    // WSL bind-mount performance: the Linux filesystem is dramatically
    // faster for project dirs than /mnt/c-style Windows mounts.
    if cubecli_runtime::env::is_wsl() {
        let cwd = std::env::current_dir().unwrap_or_default();
        if cwd.starts_with("/mnt/") {
            checks.push(Check {
                name: "project/path",
                status: CheckStatus::Warn,
                detail: "project lives on a Windows drive (/mnt/…) — slow I/O and slow world saves"
                    .into(),
                hint: Some("keep server projects in the WSL filesystem (e.g. ~/servers)".into()),
            });
        }
    }

    // Backend liveness + RCON reachability (the mapping compose renders must
    // actually answer on 127.0.0.1:rcon_port).
    let backend = crate::commands::project::backend(ctx, handle);
    match backend.status(handle).await {
        Ok(RuntimeStatus::Running { .. }) => {
            let rcon = rcon_probe(handle).await;
            checks.push(Check {
                name: "project/state",
                status: if rcon {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Fail
                },
                detail: if rcon {
                    "server running, RCON reachable".into()
                } else {
                    format!(
                        "server running but RCON unreachable on 127.0.0.1:{}",
                        meta.runtime.rcon_port
                    )
                },
                hint: (!rcon).then(|| match meta.runtime.backend {
                    Backend::Compose => {
                        "compose projects need `project compose` re-run after a port change".into()
                    }
                    Backend::Native => {
                        "check the server actually finished booting (project logs)".into()
                    }
                }),
            });
        }
        Ok(status) => {
            checks.push(Check {
                name: "project/state",
                status: CheckStatus::Ok,
                detail: match status {
                    RuntimeStatus::Stopped { clean: false } => {
                        "stopped (crashed — the daemon will restart it if the policy allows)".into()
                    }
                    _ => "stopped".into(),
                },
                hint: None,
            });
            if meta.runtime.backend == Backend::Compose
                && cubecli_runtime::compose::is_hand_edited(handle)
            {
                checks.push(Check {
                    name: "project/compose",
                    status: CheckStatus::Warn,
                    detail: "hand-edited compose.yaml — start keeps your edits".into(),
                    hint: Some("`cubecli project compose` regenerates it".into()),
                });
            }
        }
        Err(e) => {
            checks.push(Check {
                name: "project/state",
                status: CheckStatus::Warn,
                detail: format!("status unavailable: {e}"),
                hint: None,
            });
        }
    }
    checks
}

/// 1s RCON reachability probe against the project's configured port.
async fn rcon_probe(handle: &ProjectHandle) -> bool {
    let layout: &ProjectLayout = handle.layout();
    let Ok(secret) = std::fs::read_to_string(layout.rcon_secret_file()) else {
        return false;
    };
    let port = cubecli_runtime::native::properties_port(&layout.root().join("server.properties"))
        .unwrap_or(25575);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    matches!(
        tokio::time::timeout(
            Duration::from_secs(1),
            cubecli_runtime::rcon::connect(addr, secret.trim())
        )
        .await,
        Ok(Ok(_))
    )
}

/// Pure renderer for testability.
pub fn render(checks: &[Check]) -> String {
    let mut out = String::new();
    out.push_str("cubecli doctor\n");
    for check in checks {
        out.push_str(&format!(
            "  [{}] {:<16} {}\n",
            check.status.label(),
            check.name,
            check.detail
        ));
        if let Some(hint) = &check.hint {
            out.push_str(&format!("         {:<16} hint: {hint}\n", ""));
        }
    }
    let failed = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let warned = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Warn)
        .count();
    if failed == 0 && warned == 0 {
        out.push_str("all checks passed\n");
    } else {
        out.push_str(&format!("{failed} failed, {warned} warning(s)\n"));
    }
    out
}
