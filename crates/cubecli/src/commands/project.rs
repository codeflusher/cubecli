//! `cubecli project …` handlers: config, lifecycle, logs, console, compose.

use std::path::PathBuf;
use std::sync::Arc;

use comfy_table::Table;

use cubecli_core::ProjectHandle;
use cubecli_core::backup;
use cubecli_core::meta::ProjectMeta;
use cubecli_core::meta::config_keys;
use cubecli_core::model::{AddonSourceId, Backend, CoreBuild};
use cubecli_core::pipeline::{self, ProvisionOptions};
use cubecli_core::traits::runtime::{RuntimeBackend, RuntimeStatus, StopGrace, StopStage};
use cubecli_runtime::{ComposeBackend, NativeBackend};

use crate::cli::PlayerListCommand;
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
    if handle.meta().runtime.backend == Backend::Compose
        && cubecli_runtime::compose::is_hand_edited(&handle)
    {
        println!("note: hand-edited compose.yaml kept — `cubecli project compose` regenerates it");
    }
    Ok(())
}

pub async fn stop(ctx: &AppContext) -> Result<(), UserError> {
    let handle = discover()?;
    let outcome = backend(ctx, &handle)
        .stop(&handle, StopGrace::default())
        .await
        .map_err(UserError::from)?;
    match outcome.stage {
        StopStage::AlreadyStopped => println!("server is not running"),
        StopStage::Rcon => println!(
            "stopped cleanly via RCON ({:.1}s)",
            outcome.waited.as_secs_f32()
        ),
        // Native: SIGTERM to the process group. Compose: SIGTERM to the
        // container's main process (the JVM's shutdown hooks save the world).
        StopStage::Signal => println!("stopped via SIGTERM ({:.1}s)", outcome.waited.as_secs_f32()),
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
    // Any status failure (docker unavailable, …) degrades to an unknown
    // state instead of aborting the whole table — the error text carries
    // the actionable detail.
    let runtime = match backend(ctx, &handle).status(&handle).await {
        Ok(status) => status,
        Err(e) => {
            println!("state unknown — {e}");
            RuntimeStatus::Unknown
        }
    };

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
        for line in b.read_logs(&handle, lines).await.map_err(UserError::from)? {
            println!("{line}");
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
    match ComposeBackend::validate(&handle).await {
        Ok(()) => println!("validated with `docker compose config`"),
        Err(cubecli_runtime::RuntimeError::Docker { message, .. }) => {
            println!("skipped validation — docker is unavailable: {message}");
        }
        Err(e) => return Err(UserError::from(e)),
    }
    println!(
        "switch to it with `cubecli project config backend=compose`, then `cubecli project start` runs docker compose"
    );
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// modlist export/import

/// Wires the CLI's addon registry into the export pipeline's source lookup.
struct RegistryLookup<'a>(&'a AppContext);

impl cubecli_core::pipeline::modlist::SourceLookup for RegistryLookup<'_> {
    fn lookup(
        &self,
        id: cubecli_core::model::AddonSourceId,
    ) -> cubecli_core::Result<std::sync::Arc<dyn cubecli_core::traits::addon_source::AddonSource>>
    {
        self.0.addons.get(id)
    }
}

/// `--mod-source` for project export: `None` = use each record's own source.
fn parse_mod_source_option(value: Option<&str>) -> Result<Option<AddonSourceId>, UserError> {
    match value {
        None => Ok(None),
        Some(name) => AddonSourceId::parse(name).map(Some).ok_or_else(|| {
            UserError::new(format!("unknown source `{name}`"))
                .with_hint("one of: modrinth, curseforge")
        }),
    }
}

/// `cubecli project export-modlist` — the installed mods as a portable
/// modlist. Offline unless `--server-only` asks the sources for metadata.
pub async fn export_modlist(
    ctx: &AppContext,
    base64: bool,
    output: Option<PathBuf>,
    server_only: bool,
    mod_source: Option<String>,
) -> Result<(), UserError> {
    let handle = discover()?;
    let check_override = parse_mod_source_option(mod_source.as_deref())?;
    let export = cubecli_core::pipeline::modlist::export_from_project(
        handle.meta(),
        &RegistryLookup(ctx),
        server_only,
        check_override,
    )
    .await
    .map_err(UserError::from)?;
    if export.entries.is_empty() {
        println!("no mods installed — try `cubecli project install mod:sodium`");
        return Ok(());
    }

    // Header source: the explicit override wins; otherwise the single source
    // all records agree on (mixed records omit the line).
    let header_source = check_override.or_else(|| {
        if export.distinct_sources.len() == 1 {
            Some(export.distinct_sources[0])
        } else {
            None
        }
    });
    let mut doc = cubecli_core::modlist::ModlistDocument {
        source: header_source,
        notes: Vec::new(),
        entries: export.entries,
        not_found: Vec::new(),
    };
    if !export.excluded.is_empty() {
        doc.notes.push(format!(
            "excluded (client-only): {}",
            export
                .excluded
                .iter()
                .map(|(slug, _)| slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let channel = if output.is_some() {
        crate::commands::tools::ReportChannel::Stdout
    } else {
        crate::commands::tools::ReportChannel::Stderr
    };
    for slug in &export.metadata_unknown {
        channel.line(&format!(
            "  • could not check server-side support for `{slug}` — kept"
        ));
    }
    crate::commands::tools::emit_modlist(&doc, base64, &output, &channel)
}

/// `cubecli project import-modlist` — parse a modlist (inline text, a file
/// with `-i`, base64 with `--base64`) and install every entry through the
/// normal install pipeline. Failures are collected into a not-found warning
/// block; the command only errors out when nothing could be imported at all.
pub async fn import_modlist(
    ctx: &AppContext,
    input: Option<String>,
    input_file: bool,
    base64: bool,
) -> Result<(), UserError> {
    let mut handle = discover()?;
    let raw = match (&input, input_file) {
        (Some(text), false) => text.clone(),
        (Some(path), true) => std::fs::read_to_string(path).map_err(|e| {
            UserError::from(cubecli_core::CoreError::io(std::path::Path::new(path), e))
        })?,
        (None, _) => {
            return Err(UserError::new("no modlist input").with_hint(
                "pass the modlist text (or base64 with --base64), or a file path with -i",
            ));
        }
    };
    let text = if base64 {
        cubecli_core::modlist::decode_base64(&raw).map_err(UserError::from)?
    } else {
        raw
    };
    let doc = cubecli_core::modlist::parse(&text).map_err(UserError::from)?;
    if doc.entries.is_empty() {
        return Err(UserError::new("the modlist contains no mods")
            .with_hint("entries look like `mod:sodium` or `mod:sodium@0.6.5`"));
    }

    // The modlist header names the source it came from; otherwise the
    // project's own default applies.
    let source_id = doc.source.unwrap_or(handle.meta().addons.default_source);
    crate::commands::tools::require_curseforge_key(ctx, source_id)?;
    let source = ctx.addons.get(source_id)?;

    println!(
        "importing {} mod(s) via {} into project `{}`…",
        doc.entries.len(),
        source_id.slug(),
        handle.meta().project.slug
    );
    let report = cubecli_core::pipeline::modlist::import_modlist(
        &mut handle,
        source.as_ref(),
        &doc.entries,
        &|dep| {
            println!("  • auto-accepted dependency `{}`", dep.slug);
            true
        },
    )
    .await;

    for record in &report.installed {
        println!(
            "installed {} `{}` {} → {}/{}",
            record.kind.slug(),
            record.slug,
            record.version,
            record.dir,
            record.file
        );
    }
    for (slug, reason) in &report.skipped {
        println!("  • skipped `{slug}` ({reason})");
    }
    for warning in &report.warnings {
        println!("  • {warning}");
    }

    let imported = doc
        .entries
        .iter()
        .filter(|e| {
            report
                .installed
                .iter()
                .any(|r| r.slug.eq_ignore_ascii_case(&e.slug))
        })
        .count();
    println!(
        "imported {imported} of {} mods{}",
        doc.entries.len(),
        if report.skipped.is_empty() {
            String::new()
        } else {
            format!(
                " ({} already installed)",
                report.skipped.len()
            )
        }
    );
    if !report.not_found.is_empty() {
        println!("  • not found ({}):", report.not_found.len());
        for (slug, reason) in &report.not_found {
            println!("  •   `{slug}` — {reason}");
        }
        println!(
            "warning: {} mod(s) could not be imported — see the list above",
            report.not_found.len()
        );
    }

    if imported == 0 && report.skipped.is_empty() && !report.not_found.is_empty() {
        let (slug, reason) = &report.not_found[0];
        return Err(UserError::new(format!(
            "no mods could be imported — `{slug}`: {reason}"
        )));
    }
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
// whitelist & ops

/// Which managed player list a command targets.
#[derive(Clone, Copy, PartialEq)]
enum ListKind {
    Whitelist,
    Ops,
}

impl ListKind {
    fn file(self) -> &'static str {
        match self {
            ListKind::Whitelist => "whitelist.json",
            ListKind::Ops => "ops.json",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ListKind::Whitelist => "whitelist",
            ListKind::Ops => "ops",
        }
    }

    fn meta_list(self, meta: &ProjectMeta) -> &Vec<String> {
        match self {
            ListKind::Whitelist => &meta.server.whitelist,
            ListKind::Ops => &meta.server.ops,
        }
    }

    fn meta_list_mut(self, meta: &mut ProjectMeta) -> &mut Vec<String> {
        match self {
            ListKind::Whitelist => &mut meta.server.whitelist,
            ListKind::Ops => &mut meta.server.ops,
        }
    }

    /// RCON commands that apply one change to a live server. The server
    /// resolves UUIDs itself — correct for offline mode too.
    fn live_commands(self, add: bool, name: &str) -> Vec<String> {
        match (self, add) {
            (ListKind::Whitelist, true) => vec![
                format!("whitelist add {name}"),
                "whitelist on".into(),
                "whitelist reload".into(),
            ],
            (ListKind::Whitelist, false) => {
                vec![
                    format!("whitelist remove {name}"),
                    "whitelist reload".into(),
                ]
            }
            (ListKind::Ops, true) => vec![format!("op {name}")],
            (ListKind::Ops, false) => vec![format!("deop {name}")],
        }
    }
}

pub async fn whitelist(ctx: &AppContext, command: PlayerListCommand) -> Result<(), UserError> {
    player_list(ctx, ListKind::Whitelist, command).await
}

pub async fn op(ctx: &AppContext, command: PlayerListCommand) -> Result<(), UserError> {
    player_list(ctx, ListKind::Ops, command).await
}

async fn player_list(
    ctx: &AppContext,
    kind: ListKind,
    command: PlayerListCommand,
) -> Result<(), UserError> {
    let mut handle = discover()?;
    match command {
        PlayerListCommand::List => {
            let list = kind.meta_list(handle.meta());
            if list.is_empty() {
                println!("{}: (empty)", kind.label());
            } else {
                for name in list {
                    println!("{name}");
                }
            }
            if kind == ListKind::Whitelist {
                println!(
                    "white-list is {} in server.properties ({} entries)",
                    if list.is_empty() { "off" } else { "on" },
                    list.len()
                );
            }
            Ok(())
        }
        PlayerListCommand::Add { names } => {
            let mut to_apply = Vec::new();
            let mut meta_changed = false;
            {
                let list = kind.meta_list_mut(handle.meta_mut());
                let mut seen = Vec::new();
                for name in names {
                    let name = name.trim().to_string();
                    if name.is_empty() || seen.contains(&name) {
                        continue;
                    }
                    seen.push(name.clone());
                    if list.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                        // already recorded — still re-apply, repairing projects
                        // whose player lists never made it to disk
                        println!(
                            "{name} is already in the {} list — re-applying",
                            kind.label()
                        );
                    } else {
                        list.push(name.clone());
                        meta_changed = true;
                        println!("added {name} to the {} list", kind.label());
                    }
                    to_apply.push(name);
                }
            }
            if to_apply.is_empty() {
                return Ok(());
            }
            if meta_changed {
                handle.save()?;
            }
            apply_player_list(ctx, &mut handle, kind, true, &to_apply).await
        }
        PlayerListCommand::Remove { names } => {
            let mut removed = Vec::new();
            {
                let list = kind.meta_list_mut(handle.meta_mut());
                for name in names {
                    let name = name.trim();
                    let before = list.len();
                    list.retain(|n| !n.eq_ignore_ascii_case(name));
                    if list.len() < before {
                        removed.push(name.to_string());
                    }
                }
            }
            if removed.is_empty() {
                println!("none of those were in the {} list", kind.label());
                return Ok(());
            }
            handle.save()?;
            apply_player_list(ctx, &mut handle, kind, false, &removed).await
        }
    }
}

/// Persist a whitelist/ops change to `server.properties` and the JSON player
/// lists. On a running server the change goes through RCON (the server then
/// resolves UUIDs itself and rewrites its files); otherwise names are
/// resolved via the Mojang profiles API and the files written directly.
async fn apply_player_list(
    ctx: &AppContext,
    handle: &mut ProjectHandle,
    kind: ListKind,
    add: bool,
    names: &[String],
) -> Result<(), UserError> {
    if cubecli_core::managed_files::sync_server_properties(handle.meta(), handle.layout())
        .map_err(UserError::from)?
    {
        println!("  • server.properties updated");
    }

    let running = backend(ctx, handle)
        .status(handle)
        .await
        .map_err(UserError::from)?
        .is_running();

    if running {
        match cubecli_runtime::native::rcon_client(handle).await {
            Ok(mut rcon) => {
                for name in names {
                    for command in kind.live_commands(add, name) {
                        match rcon.command(&command).await {
                            Ok(reply) => println!("{} → {}", command, reply.trim()),
                            Err(e) => println!("{command} → error: {e}"),
                        }
                    }
                }
                return Ok(());
            }
            Err(e) => println!(
                "  • RCON unreachable ({e}) — writing files instead; applies on next start"
            ),
        }
    }

    let mojang = ctx.cores.mojang();
    let mut uuids: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let all = kind.meta_list(handle.meta()).clone();
    for name in &all {
        if uuids.contains_key(name) {
            continue;
        }
        match mojang.uuid_for_name(name).await {
            Ok(uuid) => {
                uuids.insert(name.clone(), uuid);
            }
            Err(e) => println!(
                "  • no UUID for `{name}` ({e}) — skipped; add while the server runs, or check the spelling"
            ),
        }
    }
    let unresolved =
        cubecli_core::managed_files::write_player_lists(handle.meta(), handle.layout(), &|n| {
            uuids.get(n).cloned()
        })
        .map_err(UserError::from)?;
    println!(
        "{} — {} of {} entries written",
        kind.file(),
        all.len() - unresolved.len(),
        all.len()
    );
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

    // ArgFile launches carry JVM flags in user_jvm_args.txt — keep it in
    // step with the metadata.
    if key == "max-heap" {
        let changed = cubecli_core::managed_files::apply_max_heap(
            handle.layout(),
            handle.meta().runtime.max_heap.as_deref(),
        )?;
        if changed {
            println!("  • user_jvm_args.txt updated");
        }
    }

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
