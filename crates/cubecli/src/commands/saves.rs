//! `cubecli project saves …`: backup / list / restore.

use comfy_table::Table;

use cubecli_core::ProjectHandle;
use cubecli_core::backup;
use cubecli_core::model::Backend;
use cubecli_core::traits::runtime::RuntimeBackend as _;

use crate::context::AppContext;
use crate::exit::UserError;
use crate::output::human_size;

fn discover() -> Result<ProjectHandle, UserError> {
    ProjectHandle::discover(".").map_err(UserError::from)
}

/// Is the server running (per its backend)?
async fn is_running(handle: &ProjectHandle, ctx: &AppContext) -> Result<bool, UserError> {
    let status = match handle.meta().runtime.backend {
        Backend::Compose => cubecli_runtime::ComposeBackend.status(handle).await,
        Backend::Native => {
            let native = cubecli_runtime::NativeBackend::new(std::sync::Arc::new(
                cubecli_core::FsJavaLocator::new(ctx.global.jdks_dir()),
            ));
            native.status(handle).await
        }
    };
    Ok(status.map_err(UserError::from)?.is_running())
}

/// RCON `save-off` + `save-all flush` for a consistent on-disk world.
pub(crate) async fn flush_world(handle: &ProjectHandle) -> Result<(), UserError> {
    let mut client = cubecli_runtime::native::rcon_client(handle)
        .await
        .map_err(UserError::from)?;
    client.command("save-off").await.map_err(UserError::from)?;
    let reply = client
        .command("save-all flush")
        .await
        .map_err(UserError::from)?;
    println!("  • server: {reply}");
    Ok(())
}

pub(crate) async fn unflush_world(handle: &ProjectHandle) {
    if let Ok(mut client) = cubecli_runtime::native::rcon_client(handle).await {
        let _ = client.command("save-on").await;
    }
}

pub async fn backup(ctx: &AppContext) -> Result<(), UserError> {
    let handle = discover()?;
    let running = is_running(&handle, ctx).await?;
    if running {
        flush_world(&handle).await?;
    }

    let result = {
        let layout = handle.layout().clone();
        let meta = handle.meta().clone();
        tokio::task::spawn_blocking(move || {
            cubecli_core::backup::create(&layout, &meta).map(|p| (p, meta))
        })
        .await
        .map_err(|e| UserError::new(e.to_string()))?
    };

    if running {
        unflush_world(&handle).await;
    }

    let (path, meta) = result.map_err(UserError::from)?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "backed up `{}` → {} ({})",
        meta.project.slug,
        path.display(),
        human_size(size)
    );
    Ok(())
}

pub async fn list() -> Result<(), UserError> {
    let handle = discover()?;
    let backups = backup::list(&handle.layout().backups_dir());
    if backups.is_empty() {
        println!("no backups yet — try `cubecli project saves backup`");
        return Ok(());
    }
    let mut table = Table::new();
    table.set_header(vec!["backup", "created", "size"]);
    for b in backups.iter().rev() {
        table.add_row(vec![
            b.name.as_str(),
            &b.created
                .map(|c| c.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "?".into()),
            human_size(b.size).as_str(),
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn restore(ctx: &AppContext, id: Option<String>, yes: bool) -> Result<(), UserError> {
    let handle = discover()?;
    let dir = handle.layout().backups_dir();
    let backups = backup::list(&dir);
    if backups.is_empty() {
        return Err(UserError::new("no backups exist yet"));
    }

    // Resolve the target: explicit id or the TUI picker.
    let selected = match &id {
        Some(id) => backup::find_by_id(&backups, id)
            .ok_or_else(|| {
                UserError::new(format!("no backup matches `{id}`"))
                    .with_hint("run `cubecli project saves list` to see ids")
            })?
            .clone(),
        None => {
            let items: Vec<String> = backups
                .iter()
                .rev()
                .map(|b| {
                    format!(
                        "{}  {}  ({})",
                        b.created
                            .map(|c| c.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| "?".into()),
                        b.name,
                        human_size(b.size)
                    )
                })
                .collect();
            match cubecli_tui::picker::run_picker(
                "restore a backup",
                "↑/↓ move · enter restore · esc cancel",
                items,
            )
            .await
            .map_err(|e| UserError::new(e.to_string()))?
            {
                Some(index) => backups[backups.len() - 1 - index].clone(),
                None => {
                    println!("aborted");
                    return Ok(());
                }
            }
        }
    };

    // Safety backup of the current state (that's the spec's prompt).
    let has_world = !backup::discover_worlds(handle.root()).is_empty();
    if has_world {
        let do_safety = yes
            || dialoguer::Confirm::new()
                .with_prompt("back up the current server state first?")
                .default(true)
                .interact()
                .unwrap_or(false);
        if do_safety {
            let running = is_running(&handle, ctx).await?;
            if running {
                flush_world(&handle).await?;
            }
            let layout = handle.layout().clone();
            let meta = handle.meta().clone();
            let made = tokio::task::spawn_blocking(move || backup::create(&layout, &meta))
                .await
                .map_err(|e| UserError::new(e.to_string()))?
                .map_err(UserError::from)?;
            if running {
                unflush_world(&handle).await;
            }
            println!("  • safety backup: {}", made.display());
        }
    }

    println!("restoring {} …", selected.name);
    let layout = handle.layout().clone();
    let archive = selected.path.clone();
    let manifest = tokio::task::spawn_blocking(move || backup::restore(&layout, &archive))
        .await
        .map_err(|e| UserError::new(e.to_string()))?
        .map_err(UserError::from)?;
    println!(
        "restored {} files (world state of `{}`, MC {})",
        manifest.files, manifest.slug, manifest.mc_version
    );
    println!("restart the server if it was running with the old world");
    Ok(())
}
