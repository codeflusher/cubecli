//! The addon install/uninstall pipeline, shared by the CLI and the daemon's
//! autoupgrade. Generic over [`AddonSource`] so both providers (and future
//! ones) flow through identical compat checks, replacement, and recording.

use chrono::Utc;

use crate::error::{CoreError, Result};
use crate::model::{AddonKind, Checksum, InstalledAddon};
use crate::project::ProjectHandle;
use crate::traits::addon_source::{AddonProject, AddonQuery, AddonSource};

/// One parsed `install` request: `kind:slug[@version]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonRequest {
    pub kind: AddonKind,
    pub slug: String,
    pub version: Option<String>,
}

/// Parse `mod:sodium@0.6.5` / `plugin:essentialsx`.
pub fn parse_request(input: &str) -> Result<AddonRequest> {
    let (kind_str, rest) = input.split_once(':').ok_or_else(|| {
        CoreError::invalid_config(
            "addon",
            format!("`{input}` needs a kind prefix: mod:slug or plugin:slug"),
        )
    })?;
    let kind = AddonKind::parse(kind_str).ok_or_else(|| {
        CoreError::invalid_config(
            "addon",
            format!(
                "unknown kind `{kind_str}` — one of: {}",
                AddonKind::ALL
                    .iter()
                    .map(|k| k.slug())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })?;
    let (slug, version) = match rest.split_once('@') {
        Some((slug, version)) => (slug, Some(version.to_string())),
        None => (rest, None),
    };
    if slug.trim().is_empty() {
        return Err(CoreError::invalid_config("addon", "empty slug"));
    }
    Ok(AddonRequest {
        kind,
        slug: slug.trim().to_string(),
        version,
    })
}

/// Install (or replace) an addon and its accepted dependencies.
/// Returns every record written to meta (addon first, then dependencies).
pub async fn install_addon(
    project: &mut ProjectHandle,
    source: &dyn AddonSource,
    request: &AddonRequest,
    confirm_dependency: &dyn Fn(&AddonProject) -> bool,
    depth: u32,
) -> Result<Vec<InstalledAddon>> {
    if depth > 5 {
        return Err(CoreError::Runtime("dependency chain too deep".into()));
    }
    let core = project.meta().server.core;
    if !core.supports_addon(request.kind) {
        return Err(CoreError::IncompatibleAddon {
            kind: request.kind.slug().into(),
            core: core.slug().into(),
        });
    }

    let addon_project = source.resolve(&request.slug).await?;
    let query = AddonQuery {
        mc_version: project.meta().server.mc_version.clone(),
        loader: core.loader(),
        kind: request.kind,
    };

    let entries = source.files(&addon_project, &query).await?;
    if entries.is_empty() {
        return Err(CoreError::not_found(
            source.id().slug(),
            format!(
                "{} builds of `{}` for MC {} (loader {})",
                request.kind.slug(),
                request.slug,
                query.mc_version,
                query.loader.loader_debug()
            ),
        ));
    }
    let entry = match &request.version {
        Some(want) => entries
            .iter()
            .find(|e| e.version.eq_ignore_ascii_case(want))
            .ok_or_else(|| {
                CoreError::not_found(
                    source.id().slug(),
                    format!(
                        "version `{want}` of `{}` — available: {}",
                        request.slug,
                        entries
                            .iter()
                            .take(5)
                            .map(|e| e.version.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?,
        None => entries
            .iter()
            .find(|e| e.channel != Some(crate::model::BuildChannel::Experimental))
            .or_else(|| entries.first())
            .expect("entries nonempty"),
    };

    // Download into the target dir; replace a previous file of the same slug.
    let level_name = project.meta().runtime.level_name.clone();
    let dest_dir = project.layout().addon_dir(request.kind, &level_name);
    let dir_relative = dest_dir
        .strip_prefix(project.layout().root())
        .unwrap_or(&dest_dir)
        .display()
        .to_string();

    if let Some(existing) = project.meta().find_installed(&request.slug).cloned() {
        let old = project
            .layout()
            .root()
            .join(&existing.dir)
            .join(&existing.file);
        if old != dest_dir.join(&entry.file_name) {
            let _ = std::fs::remove_file(&old);
        }
        project
            .meta_mut()
            .addons
            .installed
            .retain(|a| a.slug != existing.slug);
    }

    std::fs::create_dir_all(&dest_dir).map_err(|e| CoreError::io(&dest_dir, e))?;
    let downloaded = source.download(entry, &dest_dir).await?;

    let installed = InstalledAddon {
        slug: addon_project.slug.clone(),
        source_id: source.id(),
        provider_id: addon_project.provider_id.clone(),
        kind: request.kind,
        version: entry.version.clone(),
        file: downloaded.file_name.clone(),
        dir: dir_relative,
        sha1: entry.checksum.as_ref().map(|c| match c {
            Checksum::Sha1(h) | Checksum::Sha256(h) | Checksum::Md5(h) => h.clone(),
        }),
        installed_at: Utc::now(),
        deps: Vec::new(),
    };

    // Required dependencies, accepted one by one (skipped when installed).
    // Sub-recursions record themselves in meta; the parent is pushed last so
    // its `deps` list can reference them.
    let mut dep_slugs = Vec::new();
    let mut dep_records = Vec::new();
    if depth < 5 {
        let dependencies = source
            .required_dependencies(&addon_project, entry, &query)
            .await?;
        for dep in dependencies {
            let already = project.meta().find_installed(&dep.slug).is_some()
                || dep_records
                    .iter()
                    .any(|r: &InstalledAddon| r.slug == dep.slug)
                || dep.slug == installed.slug;
            if already || !confirm_dependency(&dep) {
                continue;
            }
            let dep_request = AddonRequest {
                kind: request.kind,
                slug: dep.slug.clone(),
                version: None,
            };
            let mut sub = Box::pin(install_addon(
                project,
                source,
                &dep_request,
                confirm_dependency,
                depth + 1,
            ))
            .await?;
            dep_slugs.extend(sub.iter().map(|r| r.slug.clone()));
            dep_records.append(&mut sub);
        }
    }

    let mut installed = installed;
    installed.deps = dep_slugs;
    project.meta_mut().addons.installed.push(installed.clone());
    project.save()?;

    let mut records = vec![installed];
    records.append(&mut dep_records);
    Ok(records)
}

/// Remove an installed addon by slug: delete the file, drop the meta record.
pub fn uninstall_addon(project: &mut ProjectHandle, slug: &str) -> Result<InstalledAddon> {
    let removed = project
        .meta()
        .find_installed(slug)
        .cloned()
        .ok_or_else(|| CoreError::not_found("cubecli", format!("installed addon `{slug}`")))?;
    let file = project
        .layout()
        .root()
        .join(&removed.dir)
        .join(&removed.file);
    match std::fs::remove_file(&file) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(CoreError::io(file, e)),
    }
    project
        .meta_mut()
        .addons
        .installed
        .retain(|a| a.slug != removed.slug);
    project.save()?;
    Ok(removed)
}

impl crate::model::ModLoader {
    /// Human-readable loader name for error messages.
    pub fn loader_debug(&self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::PaperLike => "paper/purpur",
            Self::Fabric => "fabric",
            Self::NeoForge => "neoforge",
            Self::Forge => "forge",
        }
    }
}

/// Check whether any installed addon has a newer version available.
pub async fn find_outdated(
    project: &ProjectHandle,
    source: &dyn AddonSource,
) -> Result<Vec<(InstalledAddon, String)>> {
    let meta = project.meta();
    let mut outdated = Vec::new();
    for installed in meta.addons.installed.iter() {
        if installed.source_id != source.id() {
            continue;
        }
        let addon_project = AddonProject {
            slug: installed.slug.clone(),
            provider_id: installed.provider_id.clone(),
            title: installed.slug.clone(),
            description: String::new(),
        };
        let query = AddonQuery {
            mc_version: meta.server.mc_version.clone(),
            loader: meta.server.core.loader(),
            kind: installed.kind,
        };
        if let Some(latest) = source.latest(&addon_project, &query).await? {
            if latest.version != installed.version {
                outdated.push((installed.clone(), latest.version));
            }
        }
    }
    Ok(outdated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kind_slug_version() {
        let r = parse_request("mod:sodium").unwrap();
        assert_eq!(r.kind, AddonKind::Mod);
        assert_eq!(r.slug, "sodium");
        assert_eq!(r.version, None);

        let r = parse_request("plugin:essentialsx@2.20").unwrap();
        assert_eq!(r.kind, AddonKind::Plugin);
        assert_eq!(r.version.as_deref(), Some("2.20"));

        let r = parse_request("server-resource-pack:fancy").unwrap();
        assert_eq!(r.kind, AddonKind::ServerResourcePack);

        assert!(parse_request("sodium").is_err());
        assert!(parse_request("shader:sodium").is_err());
        assert!(parse_request("mod:").is_err());
    }
}
