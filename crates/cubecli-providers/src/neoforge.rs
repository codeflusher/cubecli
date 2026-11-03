//! NeoForge via the NeoForged maven API (there is no promotions endpoint).
//!
//! Versions embed the MC version: `26.3.0.100` → MC `26.3`,
//! `21.1.107` → MC `1.21.1`. Install runs `--installServer` with the
//! resolved JDK, then launches through the generated `run.sh`.

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

pub const VERSIONS_API: &str =
    "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
pub const INSTALLER_URL: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

pub struct NeoForgeProvider {
    client: reqwest::Client,
    versions_url: String,
    installer_base: String,
    mojang: Arc<MojangMeta>,
}

impl NeoForgeProvider {
    pub fn new(client: reqwest::Client, mojang: Arc<MojangMeta>) -> Self {
        Self::with_base(
            client,
            VERSIONS_API.to_string(),
            INSTALLER_URL.to_string(),
            mojang,
        )
    }

    /// Test hook: point at a mock server.
    pub fn with_base(
        client: reqwest::Client,
        versions_url: String,
        installer_base: String,
        mojang: Arc<MojangMeta>,
    ) -> Self {
        Self {
            client,
            versions_url,
            installer_base,
            mojang,
        }
    }

    /// All stable (non-beta) versions, newest first.
    async fn all_versions(&self) -> PResult<Vec<String>> {
        let json = http::get_json(&self.client, &self.versions_url).await?;
        let mut versions: Vec<String> = json
            .get("versions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|v| !v.ends_with("-beta") && !v.contains("craftmine"))
            .map(str::to_string)
            .collect();
        crate::version_sort::sort_desc(&mut versions);
        Ok(versions)
    }

    async fn java_requirement(&self, mc_version: &str) -> PResult<u32> {
        self.mojang.java_requirement(mc_version).await
    }
}

/// `26.3.0.100` → `26.3`; `21.1.107` → `1.21.1`; junk → None.
pub fn mc_version_of(neoforge_version: &str) -> Option<String> {
    let core = neoforge_version.split('-').next()?;
    let parts: Vec<&str> = core.split('.').collect();
    match parts.as_slice() {
        [major, minor, _build] => {
            let major: u32 = major.parse().ok()?;
            if major == 0 {
                None
            } else {
                Some(format!("1.{major}.{minor}"))
            }
        }
        [major, minor, _zero, _build] => {
            let major: u32 = major.parse().ok()?;
            Some(format!("{major}.{minor}"))
        }
        _ => None,
    }
}

#[async_trait]
impl ServerCoreProvider for NeoForgeProvider {
    fn kind(&self) -> CoreKind {
        CoreKind::NeoForge
    }

    /// MC versions derived from the neoforge version list.
    async fn list_versions(&self) -> Result<Vec<McVersion>> {
        let versions = self.all_versions().await?;
        let mut mcs: Vec<String> = versions.iter().filter_map(|v| mc_version_of(v)).collect();
        mcs.sort_unstable_by(|a, b| crate::version_sort::cmp_versions(b, a));
        mcs.dedup();
        Ok(mcs
            .into_iter()
            .map(|id| McVersion { id, stable: true })
            .collect())
    }

    async fn list_builds(&self, mc_version: &str) -> Result<Vec<CoreBuild>> {
        let versions = self.all_versions().await?;
        Ok(versions
            .into_iter()
            .filter(|v| mc_version_of(v).as_deref() == Some(mc_version))
            .map(|id| CoreBuild {
                id,
                channel: BuildChannel::Stable,
                created_at: None,
                notes: None,
            })
            .collect())
    }

    async fn latest_build(&self, mc_version: &str, _channel: BuildChannel) -> Result<CoreBuild> {
        self.list_builds(mc_version)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                cubecli_core::CoreError::not_found("neoforge", format!("builds for {mc_version}"))
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
            "{}/{version}/neoforge-{version}-installer.jar",
            self.installer_base
        ))
        .map_err(|e| ProviderError::Other(format!("bad neoforge url: {e}")))?;
        let java = self.java_requirement(mc_version).await?;

        Ok(InstallPlan {
            java: JavaRequirement { major: java },
            downloads: vec![DownloadSpec {
                url,
                checksum: None,
                dest: "neoforge-installer.jar".into(),
                executable: false,
            }],
            steps: vec![
                PostInstallStep::RunCommand {
                    argv: vec![
                        "java".into(),
                        "-jar".into(),
                        "neoforge-installer.jar".into(),
                        "--installServer".into(),
                    ],
                    needs_java: true,
                    timeout: Duration::from_secs(15 * 60),
                    expect_paths: vec!["run.sh".into()],
                },
                // Installer creates a template user_jvm_args.txt; append heap flags.
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
