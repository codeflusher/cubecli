//! CurseForge v1 client.
//!
//! Everything needs the `x-api-key` header — including CDN downloads from
//! `edge.forgecdn.net` (mandatory since 2026-07). There is no slug query
//! param: we search by text and match the `slug` field client-side.
//! Pagination caps at pageSize 50.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use url::Url;

use cubecli_core::error::Result;
use cubecli_core::model::{AddonKind, AddonSourceId, BuildChannel, Checksum};
use cubecli_core::traits::addon_source::{
    AddonFileEntry, AddonProject, AddonQuery, AddonSearchHit, AddonSource, DownloadedAddon,
};

use crate::cf_categories;
use crate::error::{ProviderError, Result as PResult};
use crate::http;

pub const API: &str = "https://api.curseforge.com/v1";

/// Where the API key comes from (config file or env — resolved by the bin).
pub type KeyProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct CurseForgeSource {
    client: reqwest::Client,
    base: String,
    key: KeyProvider,
    /// Cache file for class ids (in ~/.cubecli).
    cache_path: std::path::PathBuf,
}

impl CurseForgeSource {
    pub fn new(client: reqwest::Client, key: KeyProvider, cache_path: std::path::PathBuf) -> Self {
        Self::with_base(client, API.to_string(), key, cache_path)
    }

    /// Test hook: point at a mock server.
    pub fn with_base(
        client: reqwest::Client,
        base: String,
        key: KeyProvider,
        cache_path: std::path::PathBuf,
    ) -> Self {
        Self {
            client,
            base,
            key,
            cache_path,
        }
    }

    fn api_key(&self) -> PResult<String> {
        (self.key)().ok_or(ProviderError::Auth {
            service: "curseforge",
        })
    }

    async fn get(&self, path: &str) -> PResult<Value> {
        let key = self.api_key()?;
        get_json_with_key(&self.client, &key, &format!("{}{path}", self.base)).await
    }

    async fn files_json(&self, project_id: &str, query: &AddonQuery) -> PResult<Vec<Value>> {
        let url = format!(
            "/mods/{project_id}/files?gameId={}&gameVersion={}",
            cf_categories::GAME_ID,
            http::encode_segment(&query.mc_version)
        );
        let json = self.get(&url).await?;
        Ok(json
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Text search scoped to the kind's class — the same endpoint `resolve`
    /// uses, but returning every hit instead of matching one slug.
    async fn search_json(&self, query: &str, kind: AddonKind) -> PResult<Value> {
        if kind == AddonKind::Datapack {
            return Err(ProviderError::Unsupported {
                service: "curseforge",
                reason: "datapacks have no dedicated CurseForge class".into(),
            });
        }
        let class_id =
            cf_categories::class_id_for(&self.client, &self.api_key()?, &self.cache_path, kind)
                .await?;
        let url = format!(
            "/mods/search?gameId={}&searchFilter={}&classId={class_id}&pageSize=15",
            cf_categories::GAME_ID,
            http::encode_segment(query)
        );
        self.get(&url).await
    }

    fn parse_file(v: &Value, project_id: &str) -> PResult<Option<AddonFileEntry>> {
        if v.get("isClientSideOnly").and_then(Value::as_bool) == Some(true) {
            return Ok(None); // useless on a server
        }
        let Some(name) = v.get("fileName").and_then(Value::as_str) else {
            return Ok(None);
        };
        let file_id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
        Ok(Some(AddonFileEntry {
            version: v
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string(),
            file_name: name.to_string(),
            // the real CDN url is re-resolved from provider_ref at download time
            url: v
                .get("downloadUrl")
                .and_then(Value::as_str)
                .and_then(|u| Url::parse(u).ok())
                .unwrap_or_else(|| Url::parse("https://edge.forgecdn.net/").unwrap()),
            checksum: v
                .pointer("/hashes/0/value")
                .and_then(Value::as_str)
                .zip(v.pointer("/hashes/0/algo").and_then(Value::as_i64))
                .and_then(|(hex, algo)| {
                    Checksum::parse(if algo == 1 { "sha1" } else { "md5" }, hex.to_string())
                }),
            released_at: v
                .get("fileDate")
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc)),
            channel: match v.get("releaseType").and_then(Value::as_i64) {
                Some(1) => Some(BuildChannel::Stable),
                _ => Some(BuildChannel::Experimental),
            },
            provider_ref: format!("{project_id}:{file_id}"),
        }))
    }

    async fn entries(&self, project_id: &str, query: &AddonQuery) -> PResult<Vec<AddonFileEntry>> {
        let mut entries = Vec::new();
        for v in self.files_json(project_id, query).await? {
            if let Some(entry) = Self::parse_file(&v, project_id)? {
                entries.push(entry);
            }
        }
        // newest first (API order is not guaranteed)
        entries.sort_by(|a, b| b.released_at.cmp(&a.released_at));
        Ok(entries)
    }

    /// Resolve the real CDN url per file (the /files endpoint omits it).
    async fn download_url(&self, project_id: &str, file_id: u64) -> PResult<Url> {
        let json = self
            .get(&format!("/mods/{project_id}/files/{file_id}/download-url"))
            .await?;
        let raw = json
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Other("curseforge download-url missing".into()))?;
        Url::parse(raw).map_err(|e| ProviderError::Other(format!("bad CF cdn url: {e}")))
    }
}

fn parse_hits(json: &Value) -> Vec<AddonSearchHit> {
    json.get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|m| {
                    let slug = m.get("slug").and_then(Value::as_str)?;
                    Some(AddonSearchHit {
                        slug: slug.to_string(),
                        title: m
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(slug)
                            .to_string(),
                        description: m
                            .get("summary")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        author: m
                            .get("authors")
                            .and_then(Value::as_array)
                            .and_then(|a| a.first())
                            .and_then(|a| a.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        downloads: m.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// GET with the CurseForge key header (API and CDN both).
pub async fn get_json_with_key(
    client: &reqwest::Client,
    api_key: &str,
    url: &str,
) -> PResult<Value> {
    let response = client
        .get(url)
        .header("x-api-key", api_key)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| ProviderError::Http(e.to_string()))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth {
            service: "curseforge",
        });
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::RateLimited {
            service: "curseforge",
            retry_after: None,
        });
    }
    if !status.is_success() {
        return Err(ProviderError::Http(format!("GET {url} returned {status}")));
    }
    response
        .json::<Value>()
        .await
        .map_err(|e| ProviderError::Other(format!("invalid JSON from {url}: {e}")))
}

#[async_trait]
impl AddonSource for CurseForgeSource {
    fn id(&self) -> AddonSourceId {
        AddonSourceId::CurseForge
    }

    async fn resolve(&self, slug: &str) -> Result<AddonProject> {
        // classId only matters for files; search across classes and match slug.
        let key = self.api_key()?;
        let url = format!(
            "/mods/search?gameId={}&searchFilter={}&pageSize=50",
            cf_categories::GAME_ID,
            http::encode_segment(slug)
        );
        let json = crate::curseforge::get_json_with_key(
            &self.client,
            &key,
            &format!("{}{url}", self.base),
        )
        .await?;
        let found = json
            .get("data")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|m| m.get("slug").and_then(Value::as_str) == Some(slug))
            })
            .cloned()
            .ok_or_else(|| ProviderError::not_found("curseforge", format!("slug `{slug}`")))?;
        Ok(AddonProject {
            slug: slug.to_string(),
            provider_id: found
                .get("id")
                .and_then(Value::as_u64)
                .map(|i| i.to_string())
                .unwrap_or_default(),
            title: found
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(slug)
                .to_string(),
            description: found
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
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
        if query.kind == AddonKind::Datapack {
            return Err(cubecli_core::CoreError::Unsupported {
                service: "curseforge",
                reason: "datapacks have no dedicated CurseForge class".into(),
            });
        }
        let class_id = cf_categories::class_id_for(
            &self.client,
            &self.api_key()?,
            &self.cache_path,
            query.kind,
        )
        .await?;
        let project = if project.provider_id.is_empty() {
            self.resolve(&project.slug).await?
        } else {
            project.clone()
        };
        let _ = class_id; // search already scoped the project; files endpoint needs no class
        Ok(self.entries(&project.provider_id, query).await?)
    }

    async fn latest(
        &self,
        project: &AddonProject,
        query: &AddonQuery,
    ) -> Result<Option<AddonFileEntry>> {
        let entries = self.files(project, query).await?;
        Ok(entries
            .iter()
            .find(|e| e.channel != Some(BuildChannel::Experimental))
            .or_else(|| entries.first())
            .cloned())
    }

    async fn download(&self, file: &AddonFileEntry, dest_dir: &Path) -> Result<DownloadedAddon> {
        // provider_ref = "projectId:fileId" → download-url endpoint → CDN
        // (the CDN requires the API key header since 2026-07).
        let (project_id, file_id) = file
            .provider_ref
            .split_once(':')
            .ok_or_else(|| ProviderError::Other("curseforge entry missing provider_ref".into()))?;
        let url = self
            .download_url(project_id, file_id.parse().unwrap_or(0))
            .await?;
        let key = self.api_key()?;
        crate::curseforge::download_with_key(&self.client, &key, &url, dest_dir, &file.file_name)
            .await?;
        Ok(DownloadedAddon {
            path: dest_dir.join(&file.file_name),
            file_name: file.file_name.clone(),
        })
    }

    /// CurseForge files don't expose dependency relations reliably —
    /// documented limitation; Modrinth is the recommended source for deps.
    async fn required_dependencies(
        &self,
        _project: &AddonProject,
        _file: &AddonFileEntry,
        _query: &AddonQuery,
    ) -> Result<Vec<AddonProject>> {
        Ok(vec![])
    }
}

/// Stream a CDN download to `dest_dir/<file_name>` (with the key header).
pub async fn download_with_key(
    client: &reqwest::Client,
    api_key: &str,
    url: &Url,
    dest_dir: &Path,
    file_name: &str,
) -> PResult<()> {
    if let Some(parent) = dest_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ProviderError::io(dest_dir, e))?;
    }
    let response = client
        .get(url.clone())
        .header("x-api-key", api_key)
        .timeout(std::time::Duration::from_secs(30 * 60))
        .send()
        .await
        .map_err(|e| ProviderError::Http(e.to_string()))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth {
            service: "curseforge",
        });
    }
    if !status.is_success() {
        return Err(ProviderError::Http(format!(
            "CDN GET {url} returned {status}"
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ProviderError::Http(e.to_string()))?;
    let dest = dest_dir.join(file_name);
    std::fs::write(&dest, &bytes).map_err(|e| ProviderError::io(&dest, e))
}
