//! Addon sources — Modrinth and CurseForge implementations.

use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use url::Url;

use crate::error::Result;
use crate::model::{AddonKind, AddonSourceId, BuildChannel, Checksum, ModLoader};

/// Filter applied when resolving addon files: compatible with this project's
/// MC version, loader, and addon kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonQuery {
    pub mc_version: String,
    pub loader: ModLoader,
    pub kind: AddonKind,
}

impl AddonQuery {
    /// Loader tags to query with, most specific first. Sources should try
    /// these in order until results come back (many plugins declare only
    /// `bukkit`/`spigot` even on paper).
    pub fn loaders(&self) -> Vec<&'static str> {
        match (self.kind, self.loader) {
            (AddonKind::Mod, ModLoader::Fabric) => vec!["fabric"],
            (AddonKind::Mod, ModLoader::NeoForge) => vec!["neoforge"],
            (AddonKind::Mod, ModLoader::Forge) => vec!["forge"],
            (AddonKind::Plugin, ModLoader::PaperLike) => {
                vec!["paper", "purpur", "bukkit", "spigot"]
            }
            (AddonKind::Datapack, _) => vec!["datapack"],
            (AddonKind::ServerResourcePack, _) => vec!["resourcepack"],
            _ => vec![],
        }
    }
}

/// A resolved addon project (by slug).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonProject {
    pub slug: String,
    /// Provider-internal id (Modrinth project id, CurseForge project id).
    pub provider_id: String,
    pub title: String,
    #[allow(dead_code)] // shown by a future `addon search`
    pub description: String,
}

/// One free-text search result (never downloaded directly — resolved by
/// slug through the normal install pipeline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonSearchHit {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
}

/// Server-side support as declared by an addon source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerSide {
    Required,
    Optional,
    Unsupported,
    Unknown,
}

impl ServerSide {
    /// Whether the addon can run on a server at all (`--server-only` keeps
    /// `required`/`optional` and drops `unsupported`; unknown data keeps it too).
    pub fn runs_on_server(self) -> bool {
        matches!(self, Self::Required | Self::Optional)
    }
}

/// What a source said about a local jar file (see [`AddonSource::identify_file`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifiedFile {
    pub project: AddonProject,
    pub version: String,
    pub server_side: Option<ServerSide>,
    /// `None` when the source cannot tell (e.g. CurseForge class reverse lookup).
    pub kind: Option<AddonKind>,
}

/// One downloadable file/version of an addon project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonFileEntry {
    pub version: String,
    pub file_name: String,
    pub url: Url,
    pub checksum: Option<Checksum>,
    pub released_at: Option<DateTime<Utc>>,
    /// release/beta/alpha mapped to stable/experimental.
    pub channel: Option<BuildChannel>,
    /// Opaque source-specific reference (e.g. CurseForge `projectId:fileId`)
    /// so `download` can re-resolve anything the entry alone doesn't carry.
    #[allow(dead_code)]
    pub provider_ref: String,
}

/// Result of a verified download into `dest_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedAddon {
    pub path: std::path::PathBuf,
    pub file_name: String,
}

#[async_trait]
pub trait AddonSource: Send + Sync {
    fn id(&self) -> AddonSourceId;

    async fn resolve(&self, slug: &str) -> Result<AddonProject>;

    /// Free-text search scoped to an addon kind, best matches first.
    async fn search(&self, query: &str, kind: AddonKind) -> Result<Vec<AddonSearchHit>>;

    /// All version files matching the query, newest first.
    async fn files(
        &self,
        project: &AddonProject,
        query: &AddonQuery,
    ) -> Result<Vec<AddonFileEntry>>;

    async fn latest(
        &self,
        project: &AddonProject,
        query: &AddonQuery,
    ) -> Result<Option<AddonFileEntry>>;

    /// Stream to `dest_dir/<file_name>.part`, verify the checksum, rename atomically.
    async fn download(&self, file: &AddonFileEntry, dest_dir: &Path) -> Result<DownloadedAddon>;

    /// Required dependencies of `file` (already filtered — installed addons
    /// are removed by the resolver before prompting).
    async fn required_dependencies(
        &self,
        project: &AddonProject,
        file: &AddonFileEntry,
        query: &AddonQuery,
    ) -> Result<Vec<AddonProject>>;

    /// Identify a local jar by its content (each source hashes with its own
    /// algorithm). `Ok(None)` = unrecognized on this source.
    async fn identify_file(&self, path: &Path) -> Result<Option<IdentifiedFile>> {
        let _ = path;
        Ok(None)
    }

    /// Server-side support declared for a project.
    /// `Ok(None)` = this source has no such metadata.
    async fn server_side(&self, project: &AddonProject) -> Result<Option<ServerSide>> {
        let _ = project;
        Ok(None)
    }
}
