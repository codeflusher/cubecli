//! Fabric meta v2 client.
//!
//! The "server jar" endpoint returns an installer-hybrid launcher: the first
//! boot downloads the vanilla server + libraries. We only checksum the
//! downloaded hybrid — completeness is established by first boot.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use cubecli_core::error::Result;
use cubecli_core::model::{
    BuildChannel, CoreBuild, CoreKind, JavaRequirement, LaunchSpec, McVersion,
};
use cubecli_core::traits::server_core::{
    DownloadSpec, InstallPlan, InstalledCore, ServerCoreProvider,
};

use crate::error::{ProviderError, Result as PResult};
use crate::http;
use crate::mojang::MojangMeta;

pub const BASE: &str = "https://meta.fabricmc.net/v2";

pub struct FabricProvider {
    client: reqwest::Client,
    base: String,
    mojang: Arc<MojangMeta>,
}

impl FabricProvider {
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

    /// First entry of a meta listing (lists are newest-first, `stable` flagged).
    async fn first_stable(&self, kind: &str) -> PResult<String> {
        let url = format!("{}/versions/{kind}", self.base);
        let json = http::get_json(&self.client, &url).await?;
        json.as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|v| v.get("stable").and_then(Value::as_bool) == Some(true))
                    .and_then(|v| v.get("version").and_then(Value::as_str))
                    .map(str::to_string)
            })
            .ok_or_else(|| ProviderError::not_found("fabric", format!("stable {kind} version")))
    }

    async fn server_jar_url(
        &self,
        mc_version: &str,
        loader: &str,
        installer: &str,
    ) -> PResult<Url> {
        let url = format!(
            "{}/versions/loader/{}/{}/{}/server/jar",
            self.base,
            crate::http::encode_segment(mc_version),
            crate::http::encode_segment(loader),
            crate::http::encode_segment(installer)
        );
        Url::parse(&url).map_err(|e| ProviderError::Other(format!("bad fabric url: {e}")))
    }
}

#[async_trait]
impl ServerCoreProvider for FabricProvider {
    fn kind(&self) -> CoreKind {
        CoreKind::Fabric
    }

    async fn list_versions(&self) -> Result<Vec<McVersion>> {
        let json = http::get_json(&self.client, &format!("{}/versions/game", self.base)).await?;
        let ids: Vec<String> = json
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v.get("stable").and_then(Value::as_bool) == Some(true))
            .filter_map(|v| v.get("version").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        Ok(ids
            .into_iter()
            .map(|id| McVersion { id, stable: true })
            .collect())
    }

    /// One implicit build: the loader is picked at install time.
    async fn list_builds(&self, _mc_version: &str) -> Result<Vec<CoreBuild>> {
        Ok(vec![CoreBuild {
            id: "latest".into(),
            channel: BuildChannel::Stable,
            created_at: None,
            notes: None,
        }])
    }

    async fn latest_build(&self, _mc_version: &str, _channel: BuildChannel) -> Result<CoreBuild> {
        Ok(CoreBuild {
            id: "latest".into(),
            channel: BuildChannel::Stable,
            created_at: None,
            notes: None,
        })
    }

    /// Fabric upgrades are MC-version bumps; the loader floats to latest.
    async fn check_upgrade(&self, _current: &InstalledCore) -> Result<Option<CoreBuild>> {
        Ok(None)
    }

    async fn install_plan(&self, mc_version: &str, _build: &str) -> Result<InstallPlan> {
        let loader = self.first_stable("loader").await?;
        let installer = self.first_stable("installer").await?;
        let url = self.server_jar_url(mc_version, &loader, &installer).await?;
        let java = self.mojang.java_requirement(mc_version).await?;

        Ok(InstallPlan {
            java: JavaRequirement { major: java },
            downloads: vec![DownloadSpec {
                url,
                checksum: None, // hybrid launcher; no published hash
                dest: "server.jar".into(),
                executable: false,
            }],
            steps: vec![],
            launch: LaunchSpec::Jar {
                jar: "server.jar".into(),
                jvm_args: vec![],
                args: vec!["nogui".into()],
            },
        })
    }
}
