//! `cubecli project …` handlers: config, lifecycle, logs, console, compose.

use std::sync::Arc;

use comfy_table::Table;

use cubecli_core::ProjectHandle;
use cubecli_core::backup;
use cubecli_core::meta::config_keys;
use cubecli_core::model::{Backend, CoreBuild};
use cubecli_core::pipeline::{self, ProvisionOptions};
use cubecli_core::traits::runtime::{
    RuntimeBackend, RuntimeStatus, StopGrace, StopOutcome, StopStage,
};
use cubecli_runtime::{ComposeBackend, NativeBackend};

use crate::context::AppContext;
use crate::exit::UserError;
use crate::output::ConsoleProgress;

pub fn discover() -> Result<ProjectHandle, UserError> {
    ProjectHandle::discover(".").map_err(UserError::from)
}

/// The backend selected by project metadata.
pub(crate) fn backend(ctx: &AppContext, handle: &ProjectHandle) -> Arc<dyn RuntimeBackend> {
    match handle.meta().runtime.backend {
        Backend::Compose => Arc::new(ComposeBackend),
        Backend::Native => Arc::new(NativeBackend::new(Arc::new(
            cubecli_core::FsJavaLocator::new(ctx.global.jdks_dir()),
        ))),
    }
}

/// Provision on demand: download the core + resolve Java when the project
/// has no launch spec yet (created before provisioning ran, or legacy).
pub async fn ensure_provisioned(
    handle: &mut ProjectHandle,
    ctx: &AppContext,
) -> Result<(), UserError> {
    if handle.meta().server.launch.is_some() {
        return Ok(());
    }
    let meta = handle.meta().clone();
    println!(
        "server is not provisioned yet — fetching {} {}…",
        meta.server.core.display_name(),
        meta.server.mc_version
    );
    let provider = ctx.cores.get(meta.server.core)?;
    let build = if meta.server.core_build == "latest" {
        ""
    } else {
        meta.server.core_build.as_str()
    };
    let plan = provider
        .install_plan(&meta.server.mc_version, build)
        .await
        .map_err(UserError::from)?;
    let progress = ConsoleProgress::new();
    let options = ProvisionOptions {
        progress: &progress,
        ..ProvisionOptions::new(ctx.fetcher.as_ref(), &ctx.java, ctx.java_installer.as_ref())
    };
    let report = pipeline::provision::provision(handle.layout(), &plan, &options)
        .await
        .map_err(UserError::from)?;
    handle.meta_mut().server.launch = Some(report.launch);
    handle.meta_mut().server.java_version = report.java.feature;
    handle.save()?;
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// lifecycle

pub async fn start(ctx: &AppContext) -> Result<(), UserError> {
    let mut handle = discover()?;
    ensure_provisioned(&mut handle, ctx).await?;
    let status = backend(ctx, &handle)
        .start(&handle)
        .await
        .map_err(UserError::from)?;
    match status {
        RuntimeStatus::Running { pid, .. } => {
            println!(
                "server running (pid {}) — logs: `cubecli project logs -f`",
                pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into())
            );
        }
        _ => println!("server did not report running — check `cubecli project logs`"),
    }
    Ok(())
}

pub async fn stop(_ctx: &AppContext) -> Result<(), UserError> {
    let handle = discover()?;
    let outcome: StopOutcome = match handle.meta().runtime.backend {
        Backend::Compose => ComposeBackend
            .stop(&handle, StopGrace::default())
            .await
            .map_err(UserError::from)?,
        Backend::Native => {
            let native = NativeBackend::new(Arc::new(cubecli_core::FsJavaLocator::new(
                cubecli_core::GlobalLayout::discover()?.jdks_dir(),
            )));
            native
                .stop(&handle, StopGrace::default())
                .await
                .map_err(UserError::from)?
        }
    };
    match outcome.stage {
        StopStage::AlreadyStopped => println!("server is not running"),
        StopStage::Rcon => println!(
            "stopped cleanly via RCON ({:.1}s)",
            outcome.waited.as_secs_f32()
        ),
        StopStage::Signal => println!("stopped via SIGTERM after RCON grace"),
        StopStage::Kill => println!("server killed (SIGKILL) — the world was NOT saved cleanly"),
    }
    Ok(())
}

pub async fn restart(ctx: &AppContext) -> Result<(), UserError> {
    let handle = discover()?;
    let status = backend(ctx, &handle)
        .restart(&handle)
        .await
        .map_err(UserError::from)?;
    match status {
        RuntimeStatus::Running { .. } => println!("server restarted"),
        _ => println!("restart attempted — check `cubecli project status`"),
    }
    Ok(())
}

pub async fn status(ctx: &AppContext) -> Result<(), UserError> {
    let handle = discover()?;
    let meta = handle.meta();
    let runtime = backend(ctx, &handle)
        .status(&handle)
        .await
        .map_err(UserError::from)?;

    let state = match &runtime {
        RuntimeStatus::Running { pid, started_at } => {
            let since = started_at
                .map(|s| format!(" up since {}", s.format("%Y-%m-%d %H:%M:%S")))
                .unwrap_or_default();
            format!(
                "running (pid {}){since}",
                pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into())
            )
        }
        RuntimeStatus::Stopped { clean: false } => {
            "crashed — the daemon will restart it if the policy allows".into()
        }
        RuntimeStatus::Stopped { clean: true } => "stopped".into(),
        RuntimeStatus::Unknown => "unknown".into(),
    };

    let backups = handle
        .layout()
        .backups_dir()
        .read_dir()
        .map(|d| d.count())
        .unwrap_or(0);

    let mut table = Table::new();
    table.set_header(vec!["", ""]);
    table.add_row(vec!["project", &meta.project.slug]);
    table.add_row(vec![
        "core",
        &format!(
            "{} {} (build {})",
            meta.server.core.display_name(),
            meta.server.mc_version,
            meta.server.core_build
        ),
    ]);
    table.add_row(vec!["java", &meta.server.java_version.to_string()]);
    table.add_row(vec!["port", &meta.runtime.port.to_string()]);
    table.add_row(vec!["backend", meta.runtime.backend.slug()]);
    table.add_row(vec!["restart policy", meta.runtime.restart_policy.slug()]);
    table.add_row(vec![
        "eula",
        if meta.server.eula_accepted {
            "accepted"
        } else {
            "NOT accepted"
        },
    ]);
    table.add_row(vec![
        "addons",
        &format!("{} installed", meta.addons.installed.len()),
    ]);
    table.add_row(vec!["backups", &backups.to_string()]);
    table.add_row(vec!["state", &state]);
    println!("{table}");
    Ok(())
}

pub async fn logs(ctx: &AppContext, follow: bool, lines: usize) -> Result<(), UserError> {
    let handle = discover()?;
    let b = backend(ctx, &handle);
    if follow {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(256);
        let cancel = tokio_util::sync::CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            token.cancel();
        });
        b.tail_logs(&handle, tx, cancel)
            .await
            .map_err(UserError::from)?;
        while let Some(line) = rx.recv().await {
            println!("{line}");
        }
    } else {
        match handle.meta().runtime.backend {
            Backend::Native => {
                let tail =
                    cubecli_runtime::native::tail::last_lines(&handle.layout().server_log(), lines)
                        .map_err(UserError::from)?;
                for line in tail {
                    println!("{line}");
                }
            }
            Backend::Compose => {
                let out = ComposeBackend::logs_tail(&handle, lines)
                    .await
                    .map_err(UserError::from)?;
                print!("{out}");
            }
        }
    }
    Ok(())
}

pub async fn console() -> Result<(), UserError> {
    let handle = discover()?;
    println!("cubecli console — type server commands; /exit quits");
    let mut client = cubecli_runtime::native::rcon_client(&handle)
        .await
        .map_err(UserError::from)?;
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
    loop {
        use tokio::io::AsyncBufReadExt as _;
        print!("> ");
        use std::io::Write as _;
        std::io::stdout().flush().ok();
        let mut line = String::new();
        let n = stdin
            .read_line(&mut line)
            .await
            .map_err(|e| UserError::new(e.to_string()))?;
        if n == 0 {
            break; // EOF
        }
        match line.trim() {
            "" => continue,
            "/exit" | "/quit" => break,
            cmd => match client.command(cmd).await {
                Ok(reply) => println!("{reply}"),
                Err(e) => println!("error: {e}"),
            },
        }
    }
    Ok(())
}

pub async fn compose_export() -> Result<(), UserError> {
    let handle = discover()?;
    let path = cubecli_runtime::compose::export(&handle).map_err(UserError::from)?;
    println!("wrote {}", path.display());
    println!(
        "switch to it with `cubecli project config backend=compose`, then `cubecli project start` runs docker compose"
    );
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// upgrade

pub async fn upgrade(
    ctx: &AppContext,
    check: bool,
    mc_version: Option<String>,
) -> Result<(), UserError> {
    let mut handle = discover()?;
    if handle.meta().server.launch.is_none() {
        return Err(UserError::new("server is not provisioned yet")
            .with_hint("run `cubecli project start` first"));
    }

    // Optional MC-version bump: skips the check and re-provisions directly.
    if let Some(v) = mc_version.clone() {
        handle.meta_mut().server.mc_version = v;
        handle.save()?;
    }
    let meta = handle.meta().clone();
    let provider = ctx.cores.get(meta.server.core)?;

    let target_build = if mc_version.is_some() {
        // New MC version → always re-provision (build resolves to latest).
        Some(CoreBuild {
            id: "latest".into(),
            channel: cubecli_core::model::BuildChannel::Stable,
            created_at: None,
            notes: None,
        })
    } else {
        provider
            .check_upgrade(&cubecli_core::traits::server_core::InstalledCore {
                core: meta.server.core,
                mc_version: meta.server.mc_version.clone(),
                build: meta.server.core_build.clone(),
            })
            .await
            .map_err(UserError::from)?
    };

    let Some(build) = target_build else {
        println!(
            "{} {} is on the latest build",
            meta.server.core.display_name(),
            meta.server.mc_version
        );
        return Ok(());
    };

    println!(
        "upgrade available: {} {} (build {}) → {}",
        meta.server.core.display_name(),
        meta.server.mc_version,
        meta.server.core_build,
        build.id
    );
    if check {
        return Ok(());
    }
    if !dialoguer::Confirm::new()
        .with_prompt("apply this upgrade?")
        .default(true)
        .interact()
        .unwrap_or(false)
    {
        return Ok(());
    }

    // Safety backup before touching the install.
    if !backup::discover_worlds(handle.root()).is_empty() {
        if dialoguer::Confirm::new()
            .with_prompt("back up the world first?")
            .default(true)
            .interact()
            .unwrap_or(false)
        {
            let layout = handle.layout().clone();
            let meta = handle.meta().clone();
            let made = tokio::task::spawn_blocking(move || backup::create(&layout, &meta))
                .await
                .map_err(|e| UserError::new(e.to_string()))?
                .map_err(UserError::from)?;
            println!("  • backup: {}", made.display());
        }
    }

    // Stop if running (upgrade replaces jars underneath the process).
    let was_running = backend(ctx, &handle)
        .status(&handle)
        .await
        .map_err(UserError::from)?
        .is_running();
    if was_running {
        println!("stopping the server for the upgrade…");
        backend(ctx, &handle)
            .stop(&handle, StopGrace::default())
            .await
            .map_err(UserError::from)?;
    }

    println!(
        "provisioning {} {} (build {})…",
        meta.server.core.display_name(),
        meta.server.mc_version,
        build.id
    );
    let plan = provider
        .install_plan(&meta.server.mc_version, &build.id)
        .await
        .map_err(UserError::from)?;
    let progress = ConsoleProgress::new();
    let options = ProvisionOptions {
        progress: &progress,
        ..ProvisionOptions::new(ctx.fetcher.as_ref(), &ctx.java, ctx.java_installer.as_ref())
    };
    let report = pipeline::provision::provision(handle.layout(), &plan, &options)
        .await
        .map_err(UserError::from)?;

    handle.meta_mut().server.launch = Some(report.launch);
    handle.meta_mut().server.java_version = report.java.feature;
    handle.meta_mut().server.core_build = build.id.clone();
    handle.save()?;

    println!(
        "upgraded to build {} (java {})",
        build.id, report.java.feature
    );
    if was_running {
        println!("start the server again with `cubecli project start`");
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// addons

pub async fn install(
    ctx: &AppContext,
    addon: Option<String>,
    source: Option<String>,
) -> Result<(), UserError> {
    let Some(addon) = addon else {
        return install_search(ctx, source).await;
    };
    let mut handle = discover()?;
    let request = cubecli_core::pipeline::parse_request(&addon).map_err(UserError::from)?;
    let source_id = match source.as_deref() {
        Some(name) => cubecli_core::model::AddonSourceId::parse(name).ok_or_else(|| {
            UserError::new(format!("unknown source `{name}`"))
                .with_hint("one of: modrinth, curseforge")
        })?,
        None => handle.meta().addons.default_source,
    };
    let source = ctx.addons.get(source_id)?;

    let records = cubecli_core::pipeline::install_addon(
        &mut handle,
        source.as_ref(),
        &request,
        &|dep| {
            dialoguer::Confirm::new()
                .with_prompt(format!(
                    "`{}` requires {} `{}` — install it?",
                    request.slug,
                    dep.slug,
                    request.kind.display_name().to_lowercase()
                ))
                .default(true)
                .interact()
                .unwrap_or(false)
        },
        0,
    )
    .await
    .map_err(UserError::from)?;

    for record in &records {
        println!(
            "installed {} `{}` {} → {}/{}",
            record.kind.slug(),
            record.slug,
            record.version,
            record.dir,
            record.file
        );
    }
    println!("restart the server to load it");
    Ok(())
}

/// Interactive `cubecli project install` (no addon argument): a full-screen
/// search + multi-select TUI, then the confirmed picks go through the normal
/// install pipeline one by one.
async fn install_search(ctx: &AppContext, source: Option<String>) -> Result<(), UserError> {
    use cubecli_core::model::{AddonKind, AddonSourceId};
    use cubecli_core::pipeline::AddonRequest;
    use cubecli_core::traits::addon_source::AddonProject;
    use cubecli_tui::{InstallerOutcome, SearchFetcher};

    let mut handle = discover()?;
    let source_id = match source.as_deref() {
        Some(name) => AddonSourceId::parse(name).ok_or_else(|| {
            UserError::new(format!("unknown source `{name}`"))
                .with_hint("one of: modrinth, curseforge")
        })?,
        None => handle.meta().addons.default_source,
    };
    if source_id == AddonSourceId::CurseForge && ctx.config.curseforge_key().is_none() {
        return Err(UserError::new("searching CurseForge needs an API key")
            .with_hint("run `cubecli setup curseforge <key>`, or use `--source modrinth`"));
    }
    let addon_source = ctx.addons.get(source_id)?;

    // Only offer kinds this core can actually load; the first becomes default.
    let core = handle.meta().server.core;
    let kinds: Vec<AddonKind> = AddonKind::ALL
        .into_iter()
        .filter(|k| core.supports_addon(*k))
        .collect();

    let fetcher: SearchFetcher = {
        let source = addon_source.clone();
        Arc::new(move |query, kind| {
            let source = source.clone();
            Box::pin(async move { source.search(&query, kind).await.map_err(|e| e.to_string()) })
        })
    };

    let outcome = cubecli_tui::run_installer(kinds, source_id.slug(), fetcher)
        .await
        .map_err(|e| UserError::new(e.to_string()))?;
    let InstallerOutcome::Selected(choices) = outcome else {
        println!("aborted — nothing installed");
        return Ok(());
    };
    if choices.is_empty() {
        println!("nothing selected — nothing installed");
        return Ok(());
    }

    let mut installed = 0usize;
    let mut first_error = None;
    for choice in &choices {
        let request = AddonRequest {
            kind: choice.kind,
            slug: choice.slug.clone(),
            version: None,
        };
        match cubecli_core::pipeline::install_addon(
            &mut handle,
            addon_source.as_ref(),
            &request,
            &|dep: &AddonProject| {
                dialoguer::Confirm::new()
                    .with_prompt(format!(
                        "`{}` requires {} `{}` — install it?",
                        choice.slug,
                        choice.kind.display_name().to_lowercase(),
                        dep.slug
                    ))
                    .default(true)
                    .interact()
                    .unwrap_or(false)
            },
            0,
        )
        .await
        {
            Ok(records) => {
                for record in &records {
                    println!(
                        "installed {} `{}` {} → {}/{}",
                        record.kind.slug(),
                        record.slug,
                        record.version,
                        record.dir,
                        record.file
                    );
                }
                installed += records.len();
            }
            Err(e) => {
                println!("  ✗ {}: {} — skipped", choice.slug, e);
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }

    if installed > 0 {
        println!("restart the server to load them");
    }
    match (installed, first_error) {
        (0, Some(e)) => Err(UserError::from(e)),
        _ => Ok(()),
    }
}

pub async fn uninstall(slug: String) -> Result<(), UserError> {
    let mut handle = discover()?;
    let removed =
        cubecli_core::pipeline::uninstall_addon(&mut handle, &slug).map_err(UserError::from)?;
    println!(
        "removed {} `{}` ({})",
        removed.kind.slug(),
        removed.slug,
        removed.version
    );
    println!("restart the server to unload it");
    Ok(())
}

pub async fn addon_list(ctx: &AppContext, outdated: bool) -> Result<(), UserError> {
    let handle = discover()?;
    let meta = handle.meta();
    if meta.addons.installed.is_empty() {
        println!("no addons installed — try `cubecli project install mod:sodium`");
        return Ok(());
    }

    let mut flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if outdated {
        for source_id in cubecli_core::model::AddonSourceId::ALL {
            let Ok(source) = ctx.addons.get(source_id) else {
                continue;
            };
            let results = cubecli_core::pipeline::find_outdated(&handle, source.as_ref())
                .await
                .map_err(UserError::from)?;
            for (installed, latest) in results {
                flags.insert(installed.slug.clone(), latest);
            }
        }
    }

    let mut table = Table::new();
    table.set_header(vec!["slug", "kind", "version", "source", "file", "latest"]);
    for a in &meta.addons.installed {
        let latest = flags
            .get(&a.slug)
            .map(|v| format!("↑ {v}"))
            .unwrap_or_else(|| {
                if outdated {
                    "up to date".into()
                } else {
                    "—".into()
                }
            });
        table.add_row(vec![
            a.slug.as_str(),
            a.kind.slug(),
            a.version.as_str(),
            a.source_id.slug(),
            &format!("{}/{}", a.dir, a.file),
            latest.as_str(),
        ]);
    }
    println!("{table}");
    if !flags.is_empty() {
        println!(
            "update with `cubecli project install <kind>:<slug>` (reinstall picks the latest)"
        );
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// config

/// `cubecli project config` — no args: list; `key`: get; `key=value`: set.
pub async fn config(key_value: Option<String>) -> Result<(), UserError> {
    let mut handle = discover()?;
    match key_value {
        None => list(&handle),
        Some(kv) => {
            if let Some((key, value)) = kv.split_once('=') {
                set(&mut handle, key.trim(), value.trim())
            } else {
                get(&handle, kv.trim())
            }
        }
    }
}

fn list(handle: &ProjectHandle) -> Result<(), UserError> {
    let mut table = Table::new();
    table.set_header(vec!["key", "type", "value", "description"]);
    for info in config_keys::list() {
        let value = config_keys::get(handle.meta(), info.name).unwrap_or_else(|_| "—".into());
        table.add_row(vec![info.name, info.kind, value.as_str(), info.description]);
    }
    println!("{table}");
    Ok(())
}

fn get(handle: &ProjectHandle, key: &str) -> Result<(), UserError> {
    let value = config_keys::get(handle.meta(), key).map_err(UserError::from)?;
    println!("{value}");
    Ok(())
}

fn set(handle: &mut ProjectHandle, key: &str, value: &str) -> Result<(), UserError> {
    let warnings = config_keys::set(handle.meta_mut(), key, value).map_err(UserError::from)?;
    handle.save()?;

    // Managed files follow the metadata (idempotent — only changed lines rewrite).
    if matches!(
        key,
        "port" | "pvp" | "restart-policy" | "restart-cron" | "addon-source" | "eula"
    ) {
        if key == "eula" {
            cubecli_core::managed_files::write_eula(
                handle.layout(),
                handle.meta().server.eula_accepted,
            )?;
        }
        let changed =
            cubecli_core::managed_files::sync_server_properties(handle.meta(), handle.layout())?;
        if changed {
            println!("  • server.properties updated");
        }
    }
    for warning in warnings {
        println!("  • {warning}");
    }
    println!(
        "{} = {}",
        key,
        config_keys::get(handle.meta(), key).map_err(UserError::from)?
    );
    Ok(())
}
