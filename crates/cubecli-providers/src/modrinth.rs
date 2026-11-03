//! Modrinth v2 client (keyless reads, 300 req/min, unique User-Agent —
//! both enforced by the shared client).
//!
//! Query params are URL-encoded JSON arrays (verified live):
//! `?loaders=%5B%22fabric%22%5D&game_versions=%5B%221.21.8%22%5D`.

use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use url::Url;

use cubecli_core::error::Result;
use cubecli_core::model::{AddonKind, AddonSourceId, BuildChannel, Checksum};
use cubecli_core::traits::addon_source::{
    AddonFileEntry, AddonProject, AddonQuery, AddonSearchHit, AddonSource, DownloadedAddon,
};

use crate::error::{ProviderError, Result as PResult};
use crate::http;

pub const API: &str = "https://api.modrinth.com/v2";

/// Hits returned per search — one TUI screenful.
pub const SEARCH_LIMIT: u32 = 15;

pub struct ModrinthSource {
    client: reqwest::Client,
    base: String,
}

impl ModrinthSource {
    pub fn new(client: reqwest::Client) -> Self {
        Self::with_base(client, API.to_string())
    }

    /// Test hook: point at a mock server.
    pub fn with_base(client: reqwest::Client, base: String) -> Self {
        Self { client, base }
    }

    async fn project_json(&self, slug_or_id: &str) -> PResult<Value> {
        let url = format!("{}/project/{}", self.base, http::encode_segment(slug_or_id));
        http::get_json(&self.client, &url).await
    }

    async fn versions_json(&self, slug: &str, query: &AddonQuery) -> PResult<Vec<Value>> {
        let loaders = serde_json::to_string(&query.loaders()).unwrap_or_else(|_| "[]".into());
        let game_versions =
            serde_json::to_string(&[&query.mc_version]).unwrap_or_else(|_| "[]".into());
        let params = json_array_param(&loaders, "loaders")
            + "&"
            + &json_array_param(&game_versions, "game_versions");
        let url = format!(
            "{}/project/{}/version?{}",
            self.base,
            http::encode_segment(slug),
            params
        );
        let json = http::get_json(&self.client, &url).await?;
        Ok(json.as_array().cloned().unwrap_or_default())
    }

    async fn search_json(&self, query: &str, kind: AddonKind) -> PResult<Value> {
        let facets = serde_json::to_string(&[&[format!("project_type:{}", project_type(kind))]])
            .unwrap_or_else(|_| "[]".into());
        let url = format!(
            "{}/search?query={}&limit={SEARCH_LIMIT}&{}",
            self.base,
            http::encode_segment(query),
            json_array_param(&facets, "facets")
        );
        http::get_json(&self.client, &url).await
    }

    fn parse_project(slug_or_id: &str, json: &Value) -> PResult<AddonProject> {
        Ok(AddonProject {
            slug: json
                .get("slug")
                .and_then(Value::as_str)
                .unwrap_or(slug_or_id)
                .to_string(),
            provider_id: json
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: json
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            description: json
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// Newest-first entries with a downloadable primary file.
    async fn entries(&self, slug: &str, query: &AddonQuery) -> PResult<Vec<AddonFileEntry>> {
        let mut entries = Vec::new();
        for v in self.versions_json(slug, query).await? {
            if let Some(entry) = parse_file_entry(&v)? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

/// `"[\"fabric\",\"paper\"]"` → `loaders=%5B%22fabric%22%2C%22paper%22%5D` style.
fn json_array_param(json_array: &str, name: &str) -> String {
    let encoded = json_array
        .replace('"', "%22")
        .replace('[', "%5B")
        .replace(']', "%5D")
        .replace(',', "%2C")
        .replace(' ', "");
    format!("{name}={encoded}")
}

fn parse_file_entry(v: &Value) -> PResult<Option<AddonFileEntry>> {
    let primary = v.get("files").and_then(Value::as_array).and_then(|files| {
        files
            .iter()
            .find(|f| f.get("primary").and_then(Value::as_bool) == Some(true))
            .or_else(|| files.first())
    });
    let Some(file) = primary else {
        return Ok(None);
    };
    let url: Url = file
        .get("url")
        .and_then(Value::as_str)
        .and_then(|u| Url::parse(u).ok())
        .ok_or_else(|| ProviderError::Other("modrinth file has no url".into()))?;
    Ok(Some(AddonFileEntry {
        version: v
            .get("version_number")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        file_name: file
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        url,
        checksum: file
            .pointer("/hashes/sha1")
            .and_then(Value::as_str)
            .map(|s| Checksum::Sha1(s.to_string())),
        released_at: v
            .get("date_published")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
        channel: match v.get("version_type").and_then(Value::as_str) {
            Some("release") => Some(BuildChannel::Stable),
            _ => Some(BuildChannel::Experimental),
        },
        provider_ref: v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }))
}

/// Newest stable entry (falls back to newest of any channel).
pub fn pick_latest(entries: &[AddonFileEntry]) -> Option<&AddonFileEntry> {
    entries
        .iter()
        .find(|e| e.channel != Some(BuildChannel::Experimental))
        .or_else(|| entries.first())
}

fn parse_hits(json: &Value) -> Vec<AddonSearchHit> {
    json.get("hits")
        .and_then(Value::as_array)
        .map(|hits| {
            hits.iter()
                .filter_map(|h| {
                    let slug = h.get("slug").and_then(Value::as_str)?;
                    Some(AddonSearchHit {
                        slug: slug.to_string(),
                        title: h
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or(slug)
                            .to_string(),
                        description: h
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        author: h
                            .get("author")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        downloads: h.get("downloads").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

struct NoProgress;
impl cubecli_core::traits::progress::ProgressReporter for NoProgress {}

#[async_trait]
impl AddonSource for ModrinthSource {
    fn id(&self) -> AddonSourceId {
        AddonSourceId::Modrinth
    }

    async fn resolve(&self, slug: &str) -> Result<AddonProject> {
        let json = self.project_json(slug).await?;
        Ok(Self::parse_project(slug, &json)?)
    }

    async fn search(&self, query: &str, kind: AddonKind) -> Result<Vec<AddonSearchHit>> {
        let json = self.search_json(query, kind).await?;
        Ok(parse_hits(&json))
    }

    async fn files(
        &self,
        project: &AddonProject,
        query: &AddonQuery,
    ) -> Result<Vec<AddonFileEntry>> {
        Ok(self.entries(&project.slug, query).await?)
    }

    async fn latest(
        &self,
        project: &AddonProject,
        query: &AddonQuery,
    ) -> Result<Option<AddonFileEntry>> {
        let entries = self.entries(&project.slug, query).await?;
        Ok(pick_latest(&entries).cloned())
    }

    async fn download(&self, file: &AddonFileEntry, dest_dir: &Path) -> Result<DownloadedAddon> {
        let dest = dest_dir.join(&file.file_name);
        crate::download::download_to(
            &self.client,
            &file.url,
            &dest,
            file.checksum.as_ref(),
            &NoProgress,
        )
        .await?;
        Ok(DownloadedAddon {
            path: dest,
            file_name: file.file_name.clone(),
        })
    }

    async fn required_dependencies(
        &self,
        project: &AddonProject,
        file: &AddonFileEntry,
        query: &AddonQuery,
    ) -> Result<Vec<AddonProject>> {
        // Dependency ids live on the version JSON — refetch the (filtered)
        // version list and match the installed version number.
        let versions = self.versions_json(&project.slug, query).await?;
        let Some(version) = versions.iter().find(|v| {
            v.get("version_number").and_then(Value::as_str) == Some(file.version.as_str())
        }) else {
            return Ok(vec![]);
        };
        let mut deps = Vec::new();
        for dep in version
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if dep.get("dependency_type").and_then(Value::as_str) != Some("required") {
                continue;
            }
            let Some(dep_id) = dep.get("project_id").and_then(Value::as_str) else {
                continue;
            };
            if dep_id.is_empty() {
                continue;
            }
            let json = self.project_json(dep_id).await?;
            deps.push(Self::parse_project(dep_id, &json)?);
        }
        Ok(deps)
    }
}

/// Modrinth `project_type` for our addon kind.
pub fn project_type(kind: AddonKind) -> &'static str {
    match kind {
        AddonKind::Mod => "mod",
        AddonKind::Plugin => "plugin",
        AddonKind::Datapack => "datapack",
        AddonKind::ServerResourcePack => "resourcepack",
    }
}

/// Validate that a resolved project actually is the kind the user asked for.
pub fn check_project_type(kind: AddonKind, project_json: &Value) -> PResult<()> {
    let actual = project_json.get("project_type").and_then(Value::as_str);
    if actual == Some(project_type(kind)) {
        Ok(())
    } else {
        Err(ProviderError::Unsupported {
            service: "modrinth",
            reason: format!(
                "`{}` is a {actual:?}, not a {} — check the kind prefix",
                project_json
                    .get("slug")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                project_type(kind)
            ),
        })
    }
}
