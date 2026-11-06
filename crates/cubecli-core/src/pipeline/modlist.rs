//! Modlist export/import orchestration, generic over [`AddonSource`]:
//! `tools export-modlist` (scan a foreign folder), `project export-modlist`
//! (read `.cubecli.meta`), and `project import-modlist` (install entries).
//! Never talks HTTP itself — identification/server-side checks flow through
//! the trait so this stays mock-testable.

use std::path::Path;
use std::sync::Arc;

use crate::error::{CoreError, Result};
use crate::meta::ProjectMeta;
use crate::model::{AddonKind, AddonSourceId, InstalledAddon};
use crate::modlist::{ModlistEntry, NotFoundEntry};
use crate::project::ProjectHandle;
use crate::traits::addon_source::{AddonProject, AddonSource, ServerSide};

/// Resolves an [`AddonSourceId`] to a usable source. The bin wires this to
/// its registry; tests use canned maps.
pub trait SourceLookup {
    fn lookup(&self, id: AddonSourceId) -> Result<Arc<dyn AddonSource>>;
}

/// Result of `tools export-modlist` — entries in scan order (deterministic).
pub struct ScanReport {
    pub entries: Vec<ModlistEntry>,
    /// Client-only mods dropped by `--server-only` (slug, declared support).
    pub excluded: Vec<(String, ServerSide)>,
    pub not_found: Vec<NotFoundEntry>,
}

/// Identify every jar in `root` (+ `root/mods`) against `source`. Unresolvable
/// jars land in `not_found` with the file name as label; one failing lookup
/// never aborts the export.
pub async fn export_from_scan(
    root: &Path,
    source: &dyn AddonSource,
    server_only: bool,
    on_progress: &dyn Fn(usize, usize, &Path),
) -> Result<ScanReport> {
    let jars = crate::modlist::scan_jars(root);
    if jars.is_empty() {
        return Err(CoreError::not_found(
            "cubecli",
            format!(
                "no .jar files in {} (looked here and in mods/)",
                root.display()
            ),
        ));
    }
    let total = jars.len();
    let mut report = ScanReport {
        entries: Vec::new(),
        excluded: Vec::new(),
        not_found: Vec::new(),
    };
    let source_slug = source.id().slug();

    for (index, jar) in jars.iter().enumerate() {
        on_progress(index, total, jar);
        let label = jar.strip_prefix(root).unwrap_or(jar).display().to_string();
        let identified = match source.identify_file(jar).await {
            Ok(identified) => identified,
            Err(e) => {
                report.not_found.push(NotFoundEntry {
                    label,
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let Some(identified) = identified else {
            report.not_found.push(NotFoundEntry {
                label,
                reason: format!("not recognized on {source_slug}"),
            });
            continue;
        };
        if let Some(kind) = identified.kind.filter(|k| *k != AddonKind::Mod) {
            report.not_found.push(NotFoundEntry {
                label,
                reason: format!("is a {}, not a mod", kind.slug()),
            });
            continue;
        }
        if let Some(server_side) = identified.server_side.filter(|s| !s.runs_on_server()) {
            if server_only {
                report
                    .excluded
                    .push((identified.project.slug.clone(), server_side));
                continue;
            }
        }
        if report
            .entries
            .iter()
            .any(|e| e.slug.eq_ignore_ascii_case(&identified.project.slug))
        {
            report.not_found.push(NotFoundEntry {
                label,
                reason: format!("duplicate of `{}`", identified.project.slug),
            });
            continue;
        }
        report.entries.push(ModlistEntry {
            slug: identified.project.slug,
            version: Some(identified.version),
        });
    }
    Ok(report)
}

/// Result of `project export-modlist` — mods recorded in metadata.
pub struct ProjectExport {
    /// Mods only, sorted by slug for reproducible output.
    pub entries: Vec<ModlistEntry>,
    /// Distinct sources of the installed records (record order) — drives the
    /// `# source:` header unless an explicit `--mod-source` override wins.
    pub distinct_sources: Vec<AddonSourceId>,
    /// Client-only mods dropped by `--server-only` (slug, declared support).
    pub excluded: Vec<(String, ServerSide)>,
    /// Mods kept despite unknown server-side metadata (source errors, no data).
    pub metadata_unknown: Vec<String>,
}

/// Build the export from installed records. No source is consulted unless
/// `server_only` — plain exports are fully offline.
pub async fn export_from_project(
    meta: &ProjectMeta,
    lookup: &dyn SourceLookup,
    server_only: bool,
    check_source_override: Option<AddonSourceId>,
) -> Result<ProjectExport> {
    let mut records: Vec<&InstalledAddon> = meta
        .addons
        .installed
        .iter()
        .filter(|a| a.kind == AddonKind::Mod)
        .collect();
    // Distinct sources follow the install order from metadata (before the
    // alphabetical sort that only serves reproducible entry output).
    let mut distinct_sources = Vec::new();
    for record in &records {
        if !distinct_sources.contains(&record.source_id) {
            distinct_sources.push(record.source_id);
        }
    }
    records.sort_by(|a, b| a.slug.to_lowercase().cmp(&b.slug.to_lowercase()));

    let mut excluded = Vec::new();
    let mut metadata_unknown = Vec::new();
    let mut dropped = Vec::new();
    if server_only {
        for record in &records {
            let check_source = check_source_override.unwrap_or(record.source_id);
            let server_side = match lookup.lookup(check_source) {
                Ok(source) => {
                    let project = AddonProject {
                        slug: record.slug.clone(),
                        provider_id: record.provider_id.clone(),
                        title: record.slug.clone(),
                        description: String::new(),
                    };
                    source.server_side(&project).await
                }
                Err(e) => Err(e),
            };
            match server_side {
                Ok(Some(ss)) if !ss.runs_on_server() => {
                    excluded.push((record.slug.clone(), ss));
                    dropped.push(record.slug.clone());
                }
                Ok(None) => metadata_unknown.push(record.slug.clone()),
                Err(e) => metadata_unknown.push(format!("{} ({})", record.slug, e)),
                _ => {} // runs on server — keep silently
            }
        }
    }

    let entries = records
        .into_iter()
        .filter(|a| !dropped.contains(&a.slug))
        .map(|a| ModlistEntry {
            slug: a.slug.clone(),
            version: if a.version.is_empty() {
                None
            } else {
                Some(a.version.clone())
            },
        })
        .collect();

    Ok(ProjectExport {
        entries,
        distinct_sources,
        excluded,
        metadata_unknown,
    })
}

/// Result of `project import-modlist`.
pub struct ImportReport {
    /// Every record written (requested mods first, then auto-accepted deps).
    pub installed: Vec<InstalledAddon>,
    /// Skipped without touching the source (slug, reason).
    pub skipped: Vec<(String, String)>,
    /// Non-fatal notes (e.g. a pin that fell back to latest).
    pub warnings: Vec<String>,
    /// Could not be installed (slug, reason).
    pub not_found: Vec<(String, String)>,
}

/// Install every entry into `project` through the normal install pipeline.
/// Per-entry failures are collected, never aborting the batch — the caller
/// decides the exit code. Dependencies are accepted via `confirm_dependency`
/// (the bin auto-accepts and logs them for batch imports).
pub async fn import_modlist(
    project: &mut ProjectHandle,
    source: &dyn AddonSource,
    entries: &[ModlistEntry],
    confirm_dependency: &dyn Fn(&AddonProject) -> bool,
) -> ImportReport {
    let mut report = ImportReport {
        installed: Vec::new(),
        skipped: Vec::new(),
        warnings: Vec::new(),
        not_found: Vec::new(),
    };
    for entry in entries {
        // Already installed at the wanted version → no redownload. An unpinned
        // entry never upgrades an existing install; a pin explicitly asks for it.
        if let Some(existing) = project.meta().find_installed(&entry.slug) {
            let up_to_date = match &entry.version {
                Some(want) => existing.version.eq_ignore_ascii_case(want),
                None => true,
            };
            if up_to_date {
                report
                    .skipped
                    .push((entry.slug.clone(), "already installed".into()));
                continue;
            }
        }

        let request = crate::pipeline::AddonRequest {
            kind: AddonKind::Mod,
            slug: entry.slug.clone(),
            version: entry.version.clone(),
        };
        match crate::pipeline::install_addon(project, source, &request, confirm_dependency, 0).await
        {
            Ok(records) => report.installed.extend(records),
            // A pin that doesn't exist for this project's MC version falls
            // back to the latest compatible build (user decision).
            Err(e) if matches!(e, CoreError::NotFound { .. }) && entry.version.is_some() => {
                let pinned = entry.version.clone().unwrap_or_default();
                let fallback = crate::pipeline::AddonRequest {
                    kind: AddonKind::Mod,
                    slug: entry.slug.clone(),
                    version: None,
                };
                match crate::pipeline::install_addon(
                    project,
                    source,
                    &fallback,
                    confirm_dependency,
                    0,
                )
                .await
                {
                    Ok(records) => {
                        let installed_version = records
                            .first()
                            .map(|r| r.version.clone())
                            .unwrap_or_default();
                        report.warnings.push(format!(
                            "`{}`: pinned version {pinned} not available — installed {installed_version}",
                            entry.slug
                        ));
                        report.installed.extend(records);
                    }
                    Err(e) => report.not_found.push((entry.slug.clone(), e.to_string())),
                }
            }
            Err(e) => report.not_found.push((entry.slug.clone(), e.to_string())),
        }
    }
    report
}
