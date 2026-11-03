use std::path::PathBuf;
use std::sync::Arc;

use cubecli_core::managed_files;
use cubecli_core::model::CoreKind;
use cubecli_core::pipeline::ProvisionOptions;
use cubecli_core::registry::{Registry, RegistryEntry};
use cubecli_core::{CoreError, ProjectLayout};
use cubecli_providers::CoreRegistry;
use cubecli_tui::{FetchPayload, FetchRequest, Fetcher, WizardAnswers, WizardOutcome};

use crate::context::AppContext;
use crate::exit::UserError;
use crate::output::ConsoleProgress;

pub async fn create(
    path: Option<PathBuf>,
    yes: bool,
    core: Option<String>,
    mc_version: Option<String>,
) -> Result<(), UserError> {
    let root = path.unwrap_or_else(|| PathBuf::from("."));
    let layout = ProjectLayout::new(&root);

    if layout.meta_file().exists() {
        return Err(UserError::new(format!(
            "{} already contains a project (.cubecli.meta exists)",
            layout.root().display()
        )));
    }

    // The directory must be empty: refuse to mix with existing files.
    // (.DS_Store is macOS noise — ignore it.)
    let mut entries = match std::fs::read_dir(&root) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect::<Vec<_>>(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(UserError::new(format!(
                "directory {} does not exist",
                layout.root().display()
            ))
            .with_hint("create it first, or check the path"));
        }
        Err(e) => return Err(UserError::from(CoreError::io(&root, e))),
    };
    entries.retain(|e| e.file_name().to_string_lossy() != ".DS_Store");
    if !entries.is_empty() {
        let listed = entries
            .iter()
            .take(5)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(UserError::from(CoreError::DirNotEmpty {
            path: root.canonicalize().unwrap_or_else(|_| root.clone()),
        })
        .with_hint(format!(
            "found: {listed}{}",
            if entries.len() > 5 { ", …" } else { "" }
        )));
    }

    let ctx = AppContext::load()?;
    let slug = dir_slug(&root);

    let answers = if yes {
        // Non-interactive defaults; the exact latest build resolves at provision.
        let mut a = WizardAnswers::defaults(slug);
        let core_kind = match core.as_deref() {
            Some(name) => CoreKind::parse(name).ok_or_else(|| {
                UserError::new(format!("unknown core `{name}`"))
                    .with_hint("one of: vanilla, paper, purpur, fabric, neoforge, forge")
            })?,
            None => CoreKind::Paper,
        };
        a.core = Some(core_kind);
        a.mc_version = Some(mc_version.unwrap_or_else(|| "1.21.8".into()));
        a
    } else {
        match cubecli_tui::run_wizard(WizardAnswers::defaults(slug), wizard_fetcher(&ctx)).await {
            Ok(WizardOutcome::Answers(a)) => a,
            Ok(WizardOutcome::Aborted) => {
                println!("aborted — nothing was written");
                return Ok(());
            }
            Err(e) => return Err(UserError::new(e.to_string())),
        }
    };

    let mut meta = answers
        .clone()
        .into_project_meta()
        .map_err(UserError::from)?;

    // Persist the project skeleton first so metadata survives even if the
    // core download fails midway (re-running create is refused; use start).
    layout
        .ensure_state_dirs()
        .map_err(|e| UserError::from(CoreError::io(layout.state_dir(), e)))?;
    managed_files::write_eula(&layout, meta.server.eula_accepted)?;
    managed_files::sync_server_properties(&meta, &layout)?;
    managed_files::write_player_lists(&meta, &layout)?;
    meta.save(layout.meta_file())?;

    let mut registry = Registry::load(ctx.global.registry_file())?;
    registry.upsert(RegistryEntry {
        slug: meta.project.slug.clone(),
        path: root.canonicalize().unwrap_or_else(|_| root.clone()),
        created_at: meta.project.created_at,
    });
    registry.save(ctx.global.registry_file())?;

    // Provision the core: download, run installers, resolve Java.
    // CUBECLI_NO_DOWNLOAD=1 skips the network work (used by the offline
    // integration suite; `project start` provisions on demand later).
    if std::env::var("CUBECLI_NO_DOWNLOAD").as_deref() == Ok("1") {
        println!("CUBECLI_NO_DOWNLOAD set — core download deferred to first start");
        return Ok(());
    }
    println!(
        "provisioning {} {}…",
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
    let options =
        ProvisionOptions::new(ctx.fetcher.as_ref(), &ctx.java, ctx.java_installer.as_ref());
    let options = ProvisionOptions {
        progress: &progress,
        ..options
    };
    let report = cubecli_core::pipeline::provision::provision(&layout, &plan, &options)
        .await
        .map_err(UserError::from)?;

    meta.server.launch = Some(report.launch);
    meta.server.java_version = report.java.feature;
    meta.save(layout.meta_file())?;

    println!(
        "created project `{}` in {}",
        meta.project.slug,
        layout.root().display()
    );
    println!(
        "  core: {} {} (java {})",
        meta.server.core.display_name(),
        meta.server.mc_version,
        meta.server.java_version
    );
    println!(
        "  port: {} · pvp: {} · restart: {}",
        meta.runtime.port,
        on_off(meta.runtime.pvp),
        meta.runtime.restart_policy.slug()
    );
    if !meta.server.eula_accepted {
        println!("  note: EULA not accepted yet — `project start` will refuse until it is");
    }
    println!("next: `cubecli project start`");
    Ok(())
}

/// Live version lists from the provider layer.
fn wizard_fetcher(ctx: &AppContext) -> Fetcher {
    let cores: Arc<CoreRegistry> = ctx.cores.clone();
    Arc::new(move |req: FetchRequest| {
        let cores = cores.clone();
        Box::pin(async move {
            match req {
                FetchRequest::McVersions(kind) => match cores.get(kind) {
                    Ok(provider) => provider
                        .list_versions()
                        .await
                        .map(FetchPayload::McVersions)
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                },
            }
        })
    })
}

/// Directory name → project slug (fallback "server").
fn dir_slug(root: &std::path::Path) -> String {
    let name = root
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "server".into());
    if name.is_empty() {
        "server".into()
    } else {
        name
    }
}

fn on_off(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}
