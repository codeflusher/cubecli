//! The daemon: tick loop executing the engine's decisions for every
//! registered project, plus service install/uninstall/status.

use std::time::Duration;

use chrono::{DateTime, Utc};
use fd_lock::RwLock;

use cubecli_core::backup;
use cubecli_core::daemon::{
    Action, ProjectSnapshot, cron_due, decide, state_after_restart, update_state_after_tick,
};
use cubecli_core::pipeline::AddonRequest;
use cubecli_core::registry::Registry;
use cubecli_core::{ProjectHandle, ProjectLayout};

use crate::context::AppContext;
use crate::exit::UserError;

pub async fn run(interval_secs: u64) -> Result<(), UserError> {
    let ctx = AppContext::load()?;
    let interval = Duration::from_secs(if interval_secs > 0 {
        interval_secs
    } else {
        ctx.config.daemon_tick_secs
    });

    // Single instance.
    let lock_path = ctx.global.lock_file();
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| UserError::from(cubecli_core::CoreError::io(&lock_path, e)))?;
    let mut lock = RwLock::new(lock_file);
    let _guard = lock.try_write().map_err(|_| {
        UserError::new("another daemon is already running")
            .with_hint(format!("lock: {}", lock_path.display()))
    })?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cubecli=info".into()),
        )
        .init();

    println!(
        "daemon running (tick {interval:?}, {} projects registered)",
        {
            Registry::load(ctx.global.registry_file())
                .map(|r| r.projects.len())
                .unwrap_or(0)
        }
    );

    // Reaper: the daemon is the long-lived parent of servers it restarts —
    // without this they linger as zombies and liveness probes misfire.
    #[cfg(unix)]
    std::thread::spawn(|| {
        loop {
            // SAFETY: waitpid with WNOHANG only reaps our own children.
            let mut status = 0;
            loop {
                let rc = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                if rc <= 0 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });

    loop {
        // ctrl_c gives a clean shutdown on every platform (Windows services
        // need an explicit stop handler — the scheduled task just terminates
        // us, which is safe: every tick persists its state).
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("interrupted — daemon shutting down");
                break Ok(());
            }
            _ = tokio::time::sleep(interval) => {}
        }
        if let Err(e) = tick(&ctx).await {
            tracing::error!("tick failed: {e}");
        }
        let _ = std::fs::write(ctx.global.heartbeat_file(), Utc::now().to_rfc3339());
    }
}

async fn tick(ctx: &AppContext) -> Result<(), UserError> {
    let registry = Registry::load(ctx.global.registry_file())?;
    let now = Utc::now();
    let tick_len = chrono::Duration::from_std(ctx.config.daemon_tick())
        .unwrap_or_else(|_| chrono::Duration::seconds(30));

    for entry in &registry.projects {
        if !entry.path.is_dir() {
            continue; // tombstoned / moved away
        }
        let layout = ProjectLayout::new(&entry.path);
        let Ok(mut handle) = ProjectHandle::open(layout) else {
            continue;
        };
        let slug = entry.slug.clone();
        let state_path = ctx.global.daemon_state_file(&slug);

        let mut state = cubecli_core::daemon::state::load(&state_path).unwrap_or_default();
        let backend = crate::commands::project::backend(ctx, &handle);
        // A status error (docker daemon down, …) is not "stopped": treating
        // it as not-running restart-storms a dead docker, and running
        // decide()/update_state_after_tick anyway corrupts the backoff
        // bookkeeping. Skip the whole project for this tick.
        let status = match backend.status(&handle).await {
            Ok(status) => status,
            Err(e) => {
                tracing::warn!("{slug}: status failed ({e}) — skipping this tick");
                continue;
            }
        };
        let running = status.is_running();
        let manually_stopped = handle.layout().manually_stopped_marker().exists();
        // Unclean death (pidfile present, process gone) — crash evidence.
        let crash_evidence = matches!(
            status,
            cubecli_core::traits::runtime::RuntimeStatus::Stopped { clean: false }
        );

        let meta = handle.meta().clone();
        let autocheck_due = meta.addons.autocheck
            && state
                .last_addon_check
                .map(|t| {
                    now - t
                        >= chrono::Duration::from_std(ctx.config.addon_check_interval())
                            .unwrap_or_else(|_| chrono::Duration::hours(12))
                })
                .unwrap_or(true);

        let snapshot = ProjectSnapshot {
            running,
            manually_stopped,
            crash_evidence,
            restart_policy: meta.runtime.restart_policy,
            restart_cron: meta.runtime.restart_cron.clone(),
            backup_auto: meta.backup.auto,
            backup_rule: meta.backup.rule.clone(),
            autocheck_due,
            state: state.clone(),
            now,
            tick: tick_len,
        };

        for action in decide(&snapshot) {
            let result: std::result::Result<(), UserError> = match action {
                Action::RestartCrashed { backoff_secs } => {
                    tracing::warn!(
                        "{slug}: server crashed — restarting (backoff was {backoff_secs}s)"
                    );
                    let started = backend.start(&handle).await;
                    state_after_restart(&mut state);
                    match started {
                        Ok(_) => tracing::info!("{slug}: restarted"),
                        Err(e) => tracing::error!("{slug}: restart failed: {e}"),
                    }
                    Ok(())
                }
                Action::CronRestart => {
                    let boundary = cron_due(
                        &meta.runtime.restart_cron,
                        state.last_cron_fire,
                        now,
                        tick_len,
                    );
                    tracing::info!("{slug}: cron restart (boundary {boundary:?})");
                    let done = if running {
                        backend.restart(&handle).await.map(|_| ())
                    } else {
                        backend.start(&handle).await.map(|_| ())
                    };
                    state.last_cron_fire = boundary;
                    match done {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            tracing::error!("{slug}: cron restart failed: {e}");
                            Ok(())
                        }
                    }
                }
                Action::Backup => {
                    tracing::info!("{slug}: auto backup starting");
                    let flush = if running {
                        crate::commands::saves::flush_world(&handle).await.ok()
                    } else {
                        None
                    };
                    let layout = handle.layout().clone();
                    let meta = meta.clone();
                    let made = tokio::task::spawn_blocking(move || backup::create(&layout, &meta))
                        .await
                        .map_err(|e| UserError::new(e.to_string()))?;
                    if flush.is_some() {
                        crate::commands::saves::unflush_world(&handle).await;
                    }
                    match made {
                        Ok(path) => {
                            tracing::info!("{slug}: backup → {}", path.display());
                            state.last_backup_at = Some(now);
                            Ok(())
                        }
                        Err(e) => {
                            tracing::error!("{slug}: backup failed: {e}");
                            Ok(())
                        }
                    }
                }
                Action::CheckUpdates => {
                    state.last_addon_check = Some(now);
                    check_addon_updates(ctx, &mut handle).await;
                    Ok(())
                }
            };
            if let Err(e) = result {
                tracing::error!("{slug}: action {action:?} failed: {e}");
            }
        }

        update_state_after_tick(&mut state, &snapshot);
        cubecli_core::daemon::state::save(&state_path, &state)?;
    }
    Ok(())
}

/// autoupgrade slugs: reinstall latest; other findings go to the outdated file.
async fn check_addon_updates(ctx: &AppContext, handle: &mut ProjectHandle) {
    let meta = handle.meta().clone();
    if meta.addons.autoupgrade.is_empty() && meta.addons.installed.is_empty() {
        return;
    }
    let Ok(source) = ctx.addons.get(meta.addons.default_source) else {
        return;
    };
    let mut outdated = Vec::new();

    for installed in meta.addons.installed.clone() {
        let request = AddonRequest {
            kind: installed.kind,
            slug: installed.slug.clone(),
            version: None,
        };
        let latest = match latest_entry(&source, &request, &meta).await {
            Ok(Some(entry)) => entry,
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!(
                    "{}: update check for {} failed: {e}",
                    meta.project.slug,
                    installed.slug
                );
                continue;
            }
        };
        if latest.version == installed.version {
            continue;
        }
        if meta.addons.autoupgrade.iter().any(|s| s == &installed.slug) {
            match cubecli_core::pipeline::install_addon(
                handle,
                source.as_ref(),
                &request,
                &|_| true,
                0,
            )
            .await
            {
                Ok(records) => tracing::info!(
                    "{}: upgraded {} {} → {}",
                    meta.project.slug,
                    installed.slug,
                    installed.version,
                    records
                        .first()
                        .map(|r| r.version.clone())
                        .unwrap_or_default()
                ),
                Err(e) => tracing::error!(
                    "{}: autoupgrade of {} failed: {e}",
                    meta.project.slug,
                    installed.slug
                ),
            }
        } else {
            outdated.push((installed.slug.clone(), latest.version));
        }
    }

    let _ = std::fs::write(
        ctx.global.daemon_outdated_file(&meta.project.slug),
        serde_json::to_string_pretty(&outdated).unwrap_or_else(|_| "[]".into()),
    );
}

async fn latest_entry(
    source: &std::sync::Arc<dyn cubecli_core::traits::addon_source::AddonSource>,
    request: &AddonRequest,
    meta: &cubecli_core::ProjectMeta,
) -> cubecli_core::Result<Option<cubecli_core::traits::addon_source::AddonFileEntry>> {
    let project = source.resolve(&request.slug).await?;
    let query = cubecli_core::traits::addon_source::AddonQuery {
        mc_version: meta.server.mc_version.clone(),
        loader: meta.server.core.loader(),
        kind: request.kind,
    };
    source.latest(&project, &query).await
}

// ------------------------------------------------------------------------------------------------
// service management

pub async fn install() -> Result<(), UserError> {
    let exe =
        std::env::current_exe().map_err(|e| UserError::new(format!("cannot locate exe: {e}")))?;
    let global = cubecli_core::GlobalLayout::discover()?;
    global
        .ensure_dirs()
        .map_err(|e| UserError::from(cubecli_core::CoreError::io(global.root(), e)))?;
    let spec = cubecli_runtime::service::ServiceSpec {
        exe,
        log_out: global.daemon_out_log(),
        log_err: global.daemon_err_log(),
    };
    // systemd user units need a systemd session — WSL ships without one
    // unless explicitly enabled, and the error must say how to fix that.
    if cubecli_runtime::service::detect() == "systemd" && !cubecli_runtime::env::systemd_available()
    {
        return Err(UserError::new("systemd user session is not available").with_hint(
            "inside WSL: add `[boot] systemd=true` to /etc/wsl.conf, then run `wsl --shutdown` from Windows; on Linux: check `systemctl --user` works",
        ));
    }
    cubecli_runtime::service::install(&spec).map_err(UserError::from)?;
    println!(
        "daemon installed as {} service: {}",
        cubecli_runtime::service::detect(),
        cubecli_runtime::service::installed_path().display()
    );
    println!("  logs: {}", global.daemon_out_log().display());
    if cubecli_runtime::service::detect() == "systemd" {
        println!(
            "  note: for headless machines run `loginctl enable-linger $USER` so the daemon survives logout"
        );
    }
    Ok(())
}

pub async fn uninstall() -> Result<(), UserError> {
    cubecli_runtime::service::uninstall().map_err(UserError::from)?;
    println!("daemon service removed (logs and state kept)");
    Ok(())
}

pub async fn status() -> Result<(), UserError> {
    let installed = cubecli_runtime::service::installed_path();
    if !cubecli_runtime::service::is_installed() {
        println!("daemon: not installed ({})", installed.display());
        return Ok(());
    }
    let global = cubecli_core::GlobalLayout::discover()?;
    let heartbeat = std::fs::read_to_string(global.heartbeat_file())
        .ok()
        .and_then(|t| DateTime::parse_from_rfc3339(t.trim()).ok())
        .map(|t| t.with_timezone(&Utc));
    match heartbeat {
        Some(t) if Utc::now() - t < chrono::Duration::seconds(90) => {
            println!(
                "daemon: running (heartbeat {}s ago)",
                (Utc::now() - t).num_seconds()
            );
        }
        Some(t) => println!(
            "daemon: installed but stale (last heartbeat {}s ago)",
            (Utc::now() - t).num_seconds()
        ),
        None => println!("daemon: installed but no heartbeat yet"),
    }
    println!("  definition: {}", installed.display());
    Ok(())
}
