//! MinecraftForge via `promotions_slim.json` + the maven installer.
//! Modern promo keys drop the `1.` prefix (`26.3-latest`).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use url::Url;

use cubecli_core::error::Result;
use cubecli_core::model::{
    BuildChannel, CoreBuild, CoreKind, JavaRequirement, LaunchSpec, McVersion,
};
use cubecli_core::traits::server_core::{
    DownloadSpec, InstallPlan, InstalledCore, PostInstallStep, ServerCoreProvider,
};

use crate::error::{ProviderError, Result as PResult};
use crate::http;
use crate::mojang::MojangMeta;

pub const PROMOTIONS: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
pub const INSTALLER_URL: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";

pub struct ForgeProvider {
    client: reqwest::Client,
    promotions_url: String,
    installer_base: String,
    mojang: Arc<MojangMeta>,
}

/// (mc_version → { latest, recommended })
pub type Promotions = std::collections::BTreeMap<String, (Option<String>, Option<String>)>;

impl ForgeProvider {
    pub fn new(client: reqwest::Client, mojang: Arc<MojangMeta>) -> Self {
        Self::with_base(
            client,
            PROMOTIONS.to_string(),
            INSTALLER_URL.to_string(),
            mojang,
        )
    }

    /// Test hook: point at a mock server.
    pub fn with_base(
        client: reqwest::Client,
        promotions_url: String,
        installer_base: String,
        mojang: Arc<MojangMeta>,
    ) -> Self {
        Self {
            client,
            promotions_url,
            installer_base,
            mojang,
        }
    }

    async fn promotions(&self) -> PResult<Promotions> {
        let json = http::get_json(&self.client, &self.promotions_url).await?;
        let mut out = Promotions::new();
        let Some(promos) = json.get("promos").and_then(Value::as_object) else {
            return Err(ProviderError::Other(
                "forge promotions JSON malformed".into(),
            ));
        };
        for (key, value) in promos {
            let Some(build) = value.as_str().map(str::to_string) else {
                continue;
            };
            let Some((mc, tier)) = key.rsplit_once('-') else {
                continue;
            };
            let entry = out.entry(mc.to_string()).or_default();
            match tier {
                "latest" => entry.0 = Some(build),
                "recommended" => entry.1 = Some(build),
                _ => {}
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl ServerCoreProvider for ForgeProvider {
    fn kind(&self) -> CoreKind {
        CoreKind::Forge
    }

    /// MC versions with a *recommended* promo — the sane wizard list.
    async fn list_versions(&self) -> Result<Vec<McVersion>> {
        let promos = self.promotions().await?;
        let mut mcs: Vec<String> = promos
            .into_iter()
            .filter(|(_, (_, recommended))| recommended.is_some())
            .map(|(mc, _)| mc)
            .collect();
        mcs.sort_unstable_by(|a, b| crate::version_sort::cmp_versions(b, a));
        Ok(mcs
            .into_iter()
            .map(|id| McVersion { id, stable: true })
            .collect())
    }

    /// `[recommended, latest]` (recommended first: it is the default pick —
    /// forge "latest" is occasionally broken).
    async fn list_builds(&self, mc_version: &str) -> Result<Vec<CoreBuild>> {
        let promos = self.promotions().await?;
        let Some((latest, recommended)) = promos.get(mc_version) else {
            return Err(cubecli_core::CoreError::not_found(
                "forge",
                format!("builds for {mc_version}"),
            ));
        };
        let mut builds = Vec::new();
        if let Some(r) = recommended {
            builds.push(CoreBuild {
                id: r.clone(),
                channel: BuildChannel::Stable,
                created_at: None,
                notes: Some("recommended".into()),
            });
        }
        if let Some(l) = latest {
            if recommended.as_deref() != Some(l.as_str()) {
                builds.push(CoreBuild {
                    id: l.clone(),
                    channel: BuildChannel::Stable,
                    created_at: None,
                    notes: Some("latest".into()),
                });
            }
        }
        Ok(builds)
    }

    async fn latest_build(&self, mc_version: &str, _channel: BuildChannel) -> Result<CoreBuild> {
        self.list_builds(mc_version)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                cubecli_core::CoreError::not_found("forge", format!("builds for {mc_version}"))
            })
    }

    async fn check_upgrade(&self, current: &InstalledCore) -> Result<Option<CoreBuild>> {
        let builds = self.list_builds(&current.mc_version).await?;
        Ok(builds.into_iter().find(|b| b.id != current.build))
    }

    async fn install_plan(&self, mc_version: &str, build: &str) -> Result<InstallPlan> {
        let version = if build.is_empty() || build == "latest" {
            self.latest_build(mc_version, BuildChannel::Stable)
                .await?
                .id
        } else {
            build.to_string()
        };
        let url = Url::parse(&format!(
            "{}/{mc_version}-{version}/forge-{mc_version}-{version}-installer.jar",
            self.installer_base
        ))
        .map_err(|e| ProviderError::Other(format!("bad forge url: {e}")))?;
        let java = self.mojang.java_requirement(mc_version).await?;

        Ok(InstallPlan {
            java: JavaRequirement { major: java },
            downloads: vec![DownloadSpec {
                url,
                checksum: None,
                dest: "forge-installer.jar".into(),
                executable: false,
            }],
            steps: vec![
                PostInstallStep::RunCommand {
                    argv: vec![
                        "java".into(),
                        "-jar".into(),
                        "forge-installer.jar".into(),
                        "--installServer".into(),
                    ],
                    needs_java: true,
                    timeout: Duration::from_secs(15 * 60),
                    expect_paths: vec!["run.sh".into()],
                },
                PostInstallStep::WriteTextFile {
                    path: "user_jvm_args.txt".into(),
                    contents: "-Xms1G\n-Xmx4G\n".into(),
                },
            ],
            launch: LaunchSpec::Script {
                script: "run.sh".into(),
                args: vec!["nogui".into()],
            },
        })
    }
}
