//! `cubecli tools …` — utilities that work outside a cubecli project.
//! Currently only `export-modlist`: identify the mod jars in the current
//! directory (a foreign modpack or plain mods folder) against an addon
//! source and emit a portable modlist.

use std::io::Write as _;
use std::path::PathBuf;

use cubecli_core::model::AddonSourceId;
use cubecli_core::modlist::ModlistDocument;
use cubecli_core::pipeline::modlist;

use crate::context::AppContext;
use crate::exit::UserError;

/// Where progress/warnings go: stderr while the modlist itself streams to
/// stdout (so `> mods.txt` stays clean), stdout once `-o` takes over.
pub(crate) enum ReportChannel {
    Stderr,
    Stdout,
}

impl ReportChannel {
    pub(crate) fn line(&self, text: &str) {
        match self {
            Self::Stderr => eprintln!("{text}"),
            Self::Stdout => println!("{text}"),
        }
    }
}

/// Parse `--mod-source` (unspecified → default) with the shared hint text.
pub(crate) fn parse_mod_source(
    value: Option<&str>,
    default: AddonSourceId,
) -> Result<AddonSourceId, UserError> {
    match value {
        None => Ok(default),
        Some(name) => AddonSourceId::parse(name).ok_or_else(|| {
            UserError::new(format!("unknown source `{name}`"))
                .with_hint("one of: modrinth, curseforge")
        }),
    }
}

/// Refuse CurseForge before any network call when no key is configured —
/// the same guard as the interactive installer.
pub(crate) fn require_curseforge_key(
    ctx: &AppContext,
    source: AddonSourceId,
) -> Result<(), UserError> {
    if source == AddonSourceId::CurseForge && ctx.config.curseforge_key().is_none() {
        return Err(UserError::new("searching CurseForge needs an API key")
            .with_hint("run `cubecli setup curseforge <key>`, or use --mod-source modrinth"));
    }
    Ok(())
}

/// Serialize a modlist document (`--base64` wraps the whole text) either to
/// a file or to stdout, and report counts + the not-found warning.
pub(crate) fn emit_modlist(
    doc: &ModlistDocument,
    base64: bool,
    output: &Option<PathBuf>,
    channel: &ReportChannel,
) -> Result<(), UserError> {
    let text = doc.to_text(Some(&format!(
        "{} by cubecli",
        chrono::Utc::now().to_rfc3339()
    )));
    if base64 {
        let encoded = cubecli_core::modlist::encode_base64(&text);
        write_payload(&encoded, output, doc, channel)?;
    } else {
        write_payload(&text, output, doc, channel)?;
    }
    if !doc.not_found.is_empty() {
        channel.line(&format!(
            "  • {} item(s) could not be resolved — listed under \"# not found\"",
            doc.not_found.len()
        ));
    }
    Ok(())
}

fn write_payload(
    payload: &str,
    output: &Option<PathBuf>,
    doc: &ModlistDocument,
    channel: &ReportChannel,
) -> Result<(), UserError> {
    match output {
        Some(path) => {
            std::fs::write(path, payload)
                .map_err(|e| UserError::from(cubecli_core::CoreError::io(path, e)))?;
            channel.line(&format!(
                "wrote {} ({} mods, {} unresolved)",
                path.display(),
                doc.entries.len(),
                doc.not_found.len()
            ));
            Ok(())
        }
        None => {
            print!("{payload}");
            std::io::stdout().flush().ok();
            Ok(())
        }
    }
}

/// `cubecli tools export-modlist` — scan the current directory for mod jars
/// and identify each against the selected source.
pub async fn export_modlist(
    base64: bool,
    output: Option<PathBuf>,
    server_only: bool,
    mod_source: Option<String>,
) -> Result<(), UserError> {
    let source_id = parse_mod_source(mod_source.as_deref(), AddonSourceId::Modrinth)?;
    let ctx = AppContext::load()?;
    require_curseforge_key(&ctx, source_id)?;
    let source = ctx.addons.get(source_id)?;

    let root = std::env::current_dir()
        .map_err(|e| UserError::from(cubecli_core::CoreError::io(".", e)))?;
    let channel = if output.is_some() {
        ReportChannel::Stdout
    } else {
        ReportChannel::Stderr
    };
    let on_progress = |index: usize, total: usize, jar: &std::path::Path| {
        channel.line(&format!(
            "  • identifying {}/{} {}",
            index + 1,
            total,
            jar.display()
        ));
    };

    let report = modlist::export_from_scan(&root, source.as_ref(), server_only, &on_progress)
        .await
        .map_err(UserError::from)?;

    let mut doc = ModlistDocument {
        source: Some(source_id),
        notes: Vec::new(),
        entries: report.entries,
        not_found: report.not_found,
    };
    if !report.excluded.is_empty() {
        doc.notes.push(format!(
            "excluded (client-only): {}",
            report
                .excluded
                .iter()
                .map(|(slug, _)| slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    emit_modlist(&doc, base64, &output, &channel)
}
