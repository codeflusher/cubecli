//! PaperMC Fill v3 client.
//!
//! The v2 API is dead (410 Gone) — everything goes through
//! `https://fill.papermc.io/v3`. Download URLs are content-addressed by
//! sha256 and must always be taken from the build JSON.

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use cubecli_core::error::Result;
use cubecli_core::model::{
    BuildChannel, Checksum, CoreBuild, CoreKind, JavaRequirement, LaunchSpec, McVersion,
};
use cubecli_core::traits::server_core::{
    DownloadSpec, InstallPlan, InstalledCore, ServerCoreProvider,
};

use crate::error::{ProviderError, Result as PResult};
use crate::http;

pub const BASE: &str = "https://fill.papermc.io/v3";

pub struct PaperProvider {
    client: reqwest::Client,
    base: String,
}

impl PaperProvider {
    pub fn new(client: reqwest::Client) -> Self {
        Self::with_base(client, BASE.to_string())
    }

    /// Test hook: point at a mock server.
    pub fn with_base(client: reqwest::Client, base: String) -> Self {
        Self { client, base }
    }

    /// Version JSON: `{ version: { java: { version: { minimum } } }, builds: [ints newest-first] }`.
    async fn version_json(&self, mc_version: &str) -> PResult<Value> {
        let url = format!(
            "{}/projects/paper/versions/{}",
            self.base,
            encode(mc_version)
        );
        http::get_json(&self.client, &url).await
    }

    async fn java_minimum(&self, mc_version: &str) -> PResult<u32> {
        let json = self.version_json(mc_version).await?;
        Ok(json
            .pointer("/version/java/version/minimum")
            .and_then(Value::as_u64)
            .unwrap_or(21) as u32)
    }

    /// One build document: `{ id, channel, downloads: { "server:default": { name, url, checksums } } }`.
    async fn build_json(&self, mc_version: &str, build: &str) -> PResult<Value> {
        let url = format!(
            "{}/projects/paper/versions/{}/builds/{}",
            self.base,
            encode(mc_version),
            encode(build)
        );
        http::get_json(&self.client, &url).await
    }

    /// Newest build on `channel` — scans back through the build list when
    /// `builds/latest` is experimental (rare).
    async fn latest_build_json(&self, mc_version: &str, channel: BuildChannel) -> PResult<Value> {
        let latest = self.build_json(mc_version, "latest").await?;
        if channel_of(&latest) == channel {
            return Ok(latest);
        }
        if channel == BuildChannel::Experimental {
            return Ok(latest);
        }
        let ids = self
            .version_json(mc_version)
            .await?
            .get("builds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for id in ids.iter().take(10) {
            let Some(id) = id.as_u64() else { continue };
            let json = self.build_json(mc_version, &id.to_string()).await?;
            if channel_of(&json) == channel {
                return Ok(json);
            }
        }
        Err(ProviderError::not_found(
            "paper",
            format!("stable build for {mc_version}"),
        ))
    }
}

fn channel_of(build_json: &Value) -> BuildChannel {
    BuildChannel::parse(
        build_json
            .get("channel")
            .and_then(Value::as_str)
            .unwrap_or("STABLE"),
    )
}

fn encode(segment: &str) -> String {
    crate::http::encode_segment(segment)
}

#[async_trait]
impl ServerCoreProvider for PaperProvider {
    fn kind(&self) -> CoreKind {
        CoreKind::Paper
    }

    async fn list_versions(&self) -> Result<Vec<McVersion>> {
        let versions =
            http::get_json(&self.client, &format!("{}/projects/paper", self.base)).await?;
        let mut ids: Vec<String> = versions
            .get("versions")
            .and_then(Value::as_object)
            .map(|groups| {
                groups
                    .values()
                    .filter_map(Value::as_array)
                    .flat_map(|arr| arr.iter().filter_map(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort_unstable_by(|a, b| crate::version_sort::cmp_versions(b, a));
        ids.dedup();
        Ok(ids
            .into_iter()
            .map(|id| McVersion { id, stable: true })
            .collect())
    }

    async fn list_builds(&self, mc_version: &str) -> Result<Vec<CoreBuild>> {
        let json = self.version_json(mc_version).await?;
        let builds = json
            .get("builds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64);
        Ok(builds
            .map(|b| CoreBuild {
                id: b.to_string(),
                channel: BuildChannel::Stable,
                created_at: None,
                notes: None,
            })
            .collect())
    }

    async fn latest_build(&self, mc_version: &str, channel: BuildChannel) -> Result<CoreBuild> {
        let json = self.latest_build_json(mc_version, channel).await?;
        Ok(CoreBuild {
            id: json
                .get("id")
                .and_then(Value::as_u64)
                .map(|i| i.to_string())
                .unwrap_or_else(|| "latest".into()),
            channel: channel_of(&json),
            created_at: None,
            notes: None,
        })
    }

    async fn check_upgrade(&self, current: &InstalledCore) -> Result<Option<CoreBuild>> {
        let latest = self
            .latest_build(&current.mc_version, BuildChannel::Stable)
            .await?;
        if latest.id != current.build {
            Ok(Some(latest))
        } else {
            Ok(None)
        }
    }

    async fn install_plan(&self, mc_version: &str, build: &str) -> Result<InstallPlan> {
        let build_json = if build.is_empty() || build == "latest" {
            self.latest_build_json(mc_version, BuildChannel::Stable)
                .await?
        } else {
            self.build_json(mc_version, build).await?
        };
        let download = build_json
            .pointer("/downloads/server:default")
            .cloned()
            .unwrap_or(Value::Null);
        let name = download
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Other("paper build JSON has no server download".into()))?
            .to_string();
        let url: Url = download
            .get("url")
            .and_then(Value::as_str)
            .and_then(|u| Url::parse(u).ok())
            .ok_or_else(|| ProviderError::Other("paper build JSON has no download url".into()))?;
        let sha256 = download
            .pointer("/checksums/sha256")
            .and_then(Value::as_str)
            .map(|s| Checksum::Sha256(s.to_string()));

        let java = self.java_minimum(mc_version).await?;

        Ok(InstallPlan {
            java: JavaRequirement { major: java },
            downloads: vec![DownloadSpec {
                url,
                checksum: sha256,
                dest: name.clone().into(),
                executable: false,
            }],
            steps: vec![],
            launch: LaunchSpec::Jar {
                jar: name,
                jvm_args: vec![],
                args: vec!["nogui".into()],
            },
        })
    }
}
