//! Mojang piston-meta: the vanilla server plus the machine-readable
//! Java-requirement source for every other core.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use cubecli_core::error::Result;
use cubecli_core::model::Checksum;
use cubecli_core::model::{
    BuildChannel, CoreBuild, CoreKind, JavaRequirement, LaunchSpec, McVersion,
};
use cubecli_core::traits::server_core::{
    DownloadSpec, InstallPlan, InstalledCore, ServerCoreProvider,
};

use crate::error::{ProviderError, Result as PResult};
use crate::http;

pub const VERSION_MANIFEST: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Username → UUID lookups (a different host than piston-meta).
pub const PROFILES_API: &str = "https://api.mojang.com";

/// Path after the piston-meta origin — kept in one place so the production
/// base (`new`) and `manifest_url` can never drift apart again.
fn manifest_path() -> &'static str {
    "/mc/game/version_manifest_v2.json"
}

/// Shared piston-meta access (used by all providers for Java requirements).
pub struct MojangMeta {
    client: reqwest::Client,
    base: String,
    /// api.mojang.com in production; pointed at a mock together with `base`.
    profiles_base: String,
}

impl MojangMeta {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            base: VERSION_MANIFEST
                .strip_suffix(manifest_path())
                .unwrap_or(VERSION_MANIFEST)
                .to_string(),
            profiles_base: PROFILES_API.to_string(),
        }
    }

    /// Test hook: point piston-meta AND the profiles API at a mock server.
    pub fn with_base(client: reqwest::Client, base: String) -> Self {
        Self {
            client,
            base: base.clone(),
            profiles_base: base,
        }
    }

    /// All manifest entries, newest first.
    fn manifest_url(&self) -> String {
        format!("{}{}", self.base, manifest_path())
    }

    pub async fn entries(&self) -> PResult<Vec<(String, String, String)>> {
        // (id, type, url)
        let json = http::get_json(&self.client, &self.manifest_url()).await?;
        let mut out = Vec::new();
        if let Some(versions) = json.get("versions").and_then(Value::as_array) {
            for entry in versions {
                let id = entry.get("id").and_then(Value::as_str).unwrap_or_default();
                let ty = entry
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let url = entry.get("url").and_then(Value::as_str).unwrap_or_default();
                if !id.is_empty() && !url.is_empty() {
                    out.push((id.to_string(), ty.to_string(), url.to_string()));
                }
            }
        }
        Ok(out)
    }

    pub(crate) async fn version_json(&self, mc_version: &str) -> PResult<Value> {
        let url = self
            .entries()
            .await?
            .into_iter()
            .find(|(id, _, _)| id == mc_version)
            .map(|(_, _, url)| url)
            .ok_or_else(|| ProviderError::not_found("mojang", format!("version {mc_version}")))?;
        http::get_json(&self.client, &url).await
    }

    /// Dashed canonical UUID for a Minecraft username (profiles API).
    /// Unknown accounts answer `204 No Content` → NotFound.
    pub async fn uuid_for_name(&self, name: &str) -> PResult<String> {
        let url = format!(
            "{}/users/profiles/minecraft/{}",
            self.profiles_base,
            http::encode_segment(name)
        );
        let body = http::get_text_with_retry(&self.client, &url).await?;
        if body.trim().is_empty() {
            return Err(ProviderError::not_found(
                "mojang",
                format!("profile {name}"),
            ));
        }
        let json: Value = serde_json::from_str(&body)
            .map_err(|e| ProviderError::Other(format!("invalid JSON from {url}: {e}")))?;
        let raw = json
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Other("mojang profile has no id".into()))?;
        Ok(dash_uuid(raw))
    }

    /// `javaVersion.majorVersion` — the authoritative Java requirement
    /// (verified live: 1.21.8 → 21, 26.3 → 25).
    pub async fn java_requirement(&self, mc_version: &str) -> PResult<u32> {
        let json = self.version_json(mc_version).await?;
        json.get("javaVersion")
            .and_then(|jv| jv.get("majorVersion"))
            .and_then(Value::as_u64)
            .map(|v| v as u32)
            .ok_or_else(|| {
                ProviderError::Other(format!(
                    "mojang version JSON for {mc_version} has no javaVersion.majorVersion"
                ))
            })
    }
}

/// Vanilla server provider.
pub struct MojangProvider {
    meta: Arc<MojangMeta>,
}

impl MojangProvider {
    pub fn new(meta: Arc<MojangMeta>) -> Self {
        Self { meta }
    }
}

#[async_trait]
impl ServerCoreProvider for MojangProvider {
    fn kind(&self) -> CoreKind {
        CoreKind::Vanilla
    }

    async fn list_versions(&self) -> Result<Vec<McVersion>> {
        let versions = self.meta.entries().await?;
        Ok(versions
            .into_iter()
            .filter(|(_, ty, _)| ty == "release")
            .map(|(id, _, _)| McVersion { id, stable: true })
            .collect())
    }

    async fn list_builds(&self, _mc_version: &str) -> Result<Vec<CoreBuild>> {
        Ok(vec![synthetic_build()])
    }

    async fn latest_build(&self, _mc_version: &str, _channel: BuildChannel) -> Result<CoreBuild> {
        Ok(synthetic_build())
    }

    /// Vanilla "upgrades" are MC-version bumps, handled by `--mc-version`.
    async fn check_upgrade(&self, _current: &InstalledCore) -> Result<Option<CoreBuild>> {
        Ok(None)
    }

    async fn install_plan(&self, mc_version: &str, _build: &str) -> Result<InstallPlan> {
        let version = self.meta.version_json(mc_version).await?;
        let server = version
            .get("downloads")
            .and_then(|d| d.get("server"))
            .ok_or_else(|| {
                ProviderError::not_found("mojang", format!("server download for {mc_version}"))
            })?;
        let url: Url = server
            .get("url")
            .and_then(Value::as_str)
            .and_then(|u| Url::parse(u).ok())
            .ok_or_else(|| ProviderError::Other("mojang server download has no url".into()))?;
        let sha1 = server
            .get("sha1")
            .and_then(Value::as_str)
            .map(|s| Checksum::Sha1(s.to_string()));
        let java_major = version
            .get("javaVersion")
            .and_then(|jv| jv.get("majorVersion"))
            .and_then(Value::as_u64)
            .unwrap_or(21) as u32;

        Ok(InstallPlan {
            java: JavaRequirement { major: java_major },
            downloads: vec![DownloadSpec {
                url,
                checksum: sha1,
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

fn synthetic_build() -> CoreBuild {
    CoreBuild {
        id: "release".into(),
        channel: BuildChannel::Stable,
        created_at: None,
        notes: None,
    }
}

/// The profiles API returns 32 hex chars without separators; vanilla's
/// `whitelist.json` wants the dashed canonical form.
fn dash_uuid(raw: &str) -> String {
    if raw.len() == 32 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        format!(
            "{}-{}-{}-{}-{}",
            &raw[0..8],
            &raw[8..12],
            &raw[12..16],
            &raw[16..20],
            &raw[20..32]
        )
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the production base used to keep `/mc/game` (derived via
    /// `rsplit_once('/')`), so the manifest was fetched from
    /// `mc/game/mc/game/version_manifest_v2.json` → 404. Mock specs missed it
    /// because they wire `with_base` with a bare origin. The default base
    /// must be the bare origin, reproducing VERSION_MANIFEST exactly.
    #[test]
    fn default_base_is_the_piston_meta_origin() {
        let meta = MojangMeta::new(crate::http::client());
        assert_eq!(meta.manifest_url(), VERSION_MANIFEST);
    }

    #[test]
    fn dashes_only_wellformed_hex32() {
        assert_eq!(
            dash_uuid("5fbfba05c8934325915c0fde8bec0077"),
            "5fbfba05-c893-4325-915c-0fde8bec0077"
        );
        // already dashed, wrong length, or non-hex: pass through untouched
        assert_eq!(
            dash_uuid("5fbfba05-c893-4325-915c-0fde8bec0077"),
            "5fbfba05-c893-4325-915c-0fde8bec0077"
        );
        assert_eq!(dash_uuid("short"), "short");
        assert_eq!(
            dash_uuid("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"),
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
        );
    }
}
