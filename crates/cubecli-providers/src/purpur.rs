//! Purpur v2 client (`api.purpurmc.org/v2`). Checksums are MD5-only —
//! verified but labeled weak.

use std::sync::Arc;

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
use crate::mojang::MojangMeta;

pub const BASE: &str = "https://api.purpurmc.org/v2/purpur";

pub struct PurpurProvider {
    client: reqwest::Client,
    base: String,
    mojang: Arc<MojangMeta>,
}

impl PurpurProvider {
    pub fn new(client: reqwest::Client, mojang: Arc<MojangMeta>) -> Self {
        Self::with_base(client, BASE.to_string(), mojang)
    }

    /// Test hook: point at a mock server.
    pub fn with_base(client: reqwest::Client, base: String, mojang: Arc<MojangMeta>) -> Self {
        Self {
            client,
            base,
            mojang,
        }
    }

    /// Build metadata (same shape for `/{v}/latest` and `/{v}/{build}`):
    /// flat `{ build: "2497", md5: "...", result: "SUCCESS", ... }`.
    async fn build_json(&self, mc_version: &str, build: &str) -> PResult<Value> {
        let url = format!("{}/{mc_version}/{build}", self.base);
        http::get_json(&self.client, &url).await
    }
}

#[async_trait]
impl ServerCoreProvider for PurpurProvider {
    fn kind(&self) -> CoreKind {
        CoreKind::Purpur
    }

    async fn list_versions(&self) -> Result<Vec<McVersion>> {
        let json = http::get_json(&self.client, &self.base).await?;
        let mut ids: Vec<String> = json
            .get("versions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        crate::version_sort::sort_desc(&mut ids);
        Ok(ids
            .into_iter()
            .map(|id| McVersion { id, stable: true })
            .collect())
    }

    async fn list_builds(&self, mc_version: &str) -> Result<Vec<CoreBuild>> {
        let json = http::get_json(&self.client, &format!("{}/{mc_version}", self.base)).await?;
        let mut builds: Vec<u64> = json
            .pointer("/builds/all")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|b| b.as_str().and_then(|s| s.parse().ok()))
            .collect();
        builds.sort_unstable_by(|a, b| b.cmp(a));
        builds.dedup();
        Ok(builds
            .into_iter()
            .map(|b| CoreBuild {
                id: b.to_string(),
                channel: BuildChannel::Stable,
                created_at: None,
                notes: None,
            })
            .collect())
    }

    async fn latest_build(&self, mc_version: &str, _channel: BuildChannel) -> Result<CoreBuild> {
        let json = self.build_json(mc_version, "latest").await?;
        Ok(CoreBuild {
            id: json
                .get("build")
                .and_then(Value::as_str)
                .unwrap_or("latest")
                .to_string(),
            channel: if json.get("result").and_then(Value::as_str) == Some("SUCCESS") {
                BuildChannel::Stable
            } else {
                BuildChannel::Experimental
            },
            created_at: json
                .get("timestamp")
                .and_then(Value::as_i64)
                .map(|ms| chrono::DateTime::from_timestamp_millis(ms))
                .flatten(),
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
        let build_id = if build.is_empty() || build == "latest" {
            self.latest_build(mc_version, BuildChannel::Stable)
                .await?
                .id
        } else {
            build.to_string()
        };
        let json = self.build_json(mc_version, &build_id).await?;
        let md5 = json
            .get("md5")
            .and_then(Value::as_str)
            .map(|s| Checksum::Md5(s.to_string()));

        let url = Url::parse(&format!("{}/{mc_version}/{build_id}/download", self.base))
            .map_err(|e| ProviderError::Other(format!("bad purpur download url: {e}")))?;
        let dest = format!("purpur-{mc_version}-{build_id}.jar");
        let java = self.mojang.java_requirement(mc_version).await?;

        Ok(InstallPlan {
            java: JavaRequirement { major: java },
            downloads: vec![DownloadSpec {
                url,
                checksum: md5,
                dest: dest.clone().into(),
                executable: false,
            }],
            steps: vec![],
            launch: LaunchSpec::Jar {
                jar: dest,
                jvm_args: vec![],
                args: vec!["nogui".into()],
            },
        })
    }
}
