//! Adoptium Temurin JDK downloads (`api.adoptium.net/v3`).
//!
//! `/binary/latest/...` 307-redirects through GitHub to the release asset;
//! checksums come from `/assets/latest/...` (sha256 of the tarball).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use cubecli_core::error::{CoreError, Result};
use cubecli_core::model::{Checksum, JavaRequirement};
use cubecli_core::traits::java::{JavaInstall, JavaOrigin, JavaProvisioner};
use cubecli_core::traits::progress::ProgressReporter;

use crate::download::download_to;
use crate::error::{ProviderError, Result as PResult};
use crate::http;

pub const API: &str = "https://api.adoptium.net/v3";

pub struct AdoptiumClient {
    client: reqwest::Client,
    /// API base (`https://api.adoptium.net/v3`) — injectable for tests.
    base: String,
    /// Where owned JDKs live (`~/.cubecli/jdks`).
    pub install_root: PathBuf,
}

impl AdoptiumClient {
    pub fn new(client: reqwest::Client, install_root: PathBuf) -> Self {
        Self::with_base(client, API.to_string(), install_root)
    }

    /// Test hook: point at a mock server.
    pub fn with_base(client: reqwest::Client, base: String, install_root: PathBuf) -> Self {
        Self {
            client,
            base,
            install_root,
        }
    }

    fn os() -> &'static str {
        if cfg!(target_os = "macos") {
            "mac"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "windows"
        }
    }

    fn arch() -> &'static str {
        if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x64"
        }
    }

    fn jdk_dir(&self, feature: u32) -> PathBuf {
        self.install_root.join(feature.to_string())
    }

    /// Detect the JAVA_HOME layout (macOS tarballs nest under Contents/Home).
    pub fn detect_install(dir: &Path) -> Option<(PathBuf, PathBuf)> {
        let candidates = [dir.join("Contents/Home"), dir.to_path_buf()];
        for home in candidates {
            let java_bin = home
                .join("bin")
                .join(if cfg!(windows) { "java.exe" } else { "java" });
            if java_bin.exists() {
                return Some((home, java_bin));
            }
        }
        None
    }

    /// Install (or reuse) a JDK feature into `install_root/<feature>`.
    pub async fn install(
        &self,
        feature: u32,
        progress: &dyn ProgressReporter,
    ) -> PResult<JavaInstall> {
        let dir = self.jdk_dir(feature);
        if let Some((home, java_bin)) = Self::detect_install(&dir) {
            return Ok(JavaInstall {
                feature,
                origin: JavaOrigin::Owned,
                home,
                java_bin,
            });
        }

        // Expected checksum from the assets API (sha256 of the archive).
        let assets_url = format!(
            "{}/assets/latest/{feature}/hotspot?os={}&architecture={}&image_type=jdk",
            self.base,
            Self::os(),
            Self::arch()
        );
        let assets = http::get_json(&self.client, &assets_url).await?;
        let package = assets
            .as_array()
            .and_then(|a| a.first())
            .and_then(|a| a.pointer("/binary/package"));
        let checksum = package
            .and_then(|p| p.get("checksum"))
            .and_then(Value::as_str)
            .map(|s| Checksum::Sha256(s.to_string()));
        let link: Url = package
            .and_then(|p| p.get("link"))
            .and_then(Value::as_str)
            .and_then(|s| Url::parse(s).ok())
            .ok_or_else(|| ProviderError::not_found("adoptium", format!("JDK {feature} asset")))?;
        let archive_name = package
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("jdk.tar.gz")
            .to_string();

        std::fs::create_dir_all(&dir).map_err(|e| ProviderError::io(&dir, e))?;
        let archive = dir.join(&archive_name);
        download_to(&self.client, &link, &archive, checksum.as_ref(), progress).await?;

        // Windows Adoptium assets are `.zip`; everything else `.tar.gz`.
        progress.message(&format!("extracting {archive_name}"));
        let is_zip = archive_name.ends_with(".zip");
        let dir_clone = dir.clone();
        let archive_clone = archive.clone();
        tokio::task::spawn_blocking(move || {
            if is_zip {
                extract_zip(&archive_clone, &dir_clone)
            } else {
                extract_tar_gz(&archive_clone, &dir_clone)
            }
        })
        .await
        .map_err(|e| ProviderError::Other(e.to_string()))??;
        let _ = tokio::fs::remove_file(&archive).await;

        match Self::detect_install(&dir) {
            Some((home, java_bin)) => Ok(JavaInstall {
                feature,
                origin: JavaOrigin::Owned,
                home,
                java_bin,
            }),
            None => Err(ProviderError::Other(format!(
                "JDK {feature} archive extracted but no java binary found in {}",
                dir.display()
            ))),
        }
    }
}

/// Extract a `.tar.gz`, stripping the archive's leading directory
/// (`jdk-21.0.13+9/…` → straight into `dir`). Guards against path escapes.
fn extract_tar_gz(archive: &Path, dir: &Path) -> PResult<()> {
    let file = std::fs::File::open(archive).map_err(|e| ProviderError::io(archive, e))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    tar.set_preserve_permissions(true);
    let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    for entry in tar
        .entries()
        .map_err(|e| ProviderError::Other(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| ProviderError::Other(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| ProviderError::Other(e.to_string()))?
            .to_path_buf();
        // Reject escapes outright, then strip only a *normal* leading
        // component (the package dir). Stripping blindly would turn
        // `../escaped` into `escaped` — a silent rename, not a refusal.
        let components: Vec<_> = path.components().collect();
        if components.iter().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        }) {
            return Err(ProviderError::Other(format!(
                "refusing suspicious archive path: {}",
                path.display()
            )));
        }
        let mut comps = components.into_iter().peekable();
        if matches!(comps.peek(), Some(std::path::Component::Normal(_))) {
            comps.next(); // strip the package directory
        }
        let stripped: PathBuf = comps.collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let dest = canonical_dir.join(stripped);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ProviderError::io(parent, e))?;
        }
        entry
            .unpack(&dest)
            .map_err(|e| ProviderError::io(&dest, e))?;
    }
    Ok(())
}

#[async_trait]
impl JavaProvisioner for AdoptiumClient {
    async fn ensure(
        &self,
        requirement: JavaRequirement,
        progress: &dyn ProgressReporter,
    ) -> Result<JavaInstall> {
        self.install(requirement.major, progress)
            .await
            .map_err(Into::into)
    }
}

impl AdoptiumClient {
    /// Feature versions Adoptium currently publishes.
    pub async fn available_features(&self) -> PResult<(Vec<u32>, u32)> {
        let json = http::get_json(
            &self.client,
            &format!("{}/info/available_releases", self.base),
        )
        .await?;
        let lts: Vec<u32> = json
            .get("available_lts_releases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|v| v as u32)
            .collect();
        let most_recent_lts = json
            .get("most_recent_lts")
            .and_then(Value::as_u64)
            .unwrap_or(21) as u32;
        Ok((lts, most_recent_lts))
    }
}

/// Extract a `.zip` (Windows Adoptium assets), stripping the archive's
/// leading directory (`jdk-21.0.13+9/…` → straight into `dir`) — the same
/// contract as [`extract_tar_gz`]. `enclosed_name()` rejects path escapes.
fn extract_zip(archive: &Path, dir: &Path) -> PResult<()> {
    let file = std::fs::File::open(archive).map_err(|e| ProviderError::io(archive, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| ProviderError::Other(e.to_string()))?;
    let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| ProviderError::Other(e.to_string()))?;
        let Some(path) = entry.enclosed_name() else {
            let name = entry
                .name()
                .map(|n| n.into_owned())
                .unwrap_or_else(|_| "<undecodable name>".into());
            return Err(ProviderError::Other(format!(
                "refusing suspicious archive path: {name}"
            )));
        };
        // Strip the package directory, mirroring the tar extractor. A bare
        // entry with no leading package dir is kept whole.
        let stripped: PathBuf = path.components().skip(1).collect();
        let stripped = if stripped.as_os_str().is_empty() {
            &path
        } else {
            &stripped
        };
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let dest = canonical_dir.join(stripped);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| ProviderError::io(&dest, e))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ProviderError::io(parent, e))?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| ProviderError::io(&dest, e))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| ProviderError::io(&dest, e))?;
    }
    Ok(())
}

/// Convenience used by `setup java install`: install then report.
pub async fn install_feature(
    client: &reqwest::Client,
    install_root: &Path,
    feature: u32,
    progress: &dyn ProgressReporter,
) -> Result<JavaInstall> {
    let c = AdoptiumClient::new(client.clone(), install_root.to_path_buf());
    c.install(feature, progress).await.map_err(Into::into)
}

/// Re-exported error type so callers can match without depending on the
/// provider crate internals.
pub type AdoptiumError = ProviderError;

#[allow(dead_code)]
fn _unused(_: CoreError) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write as _;

    #[test]
    fn detect_install_supports_macos_and_linux_layouts() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(AdoptiumClient::detect_install(tmp.path()).is_none());

        // macOS: Contents/Home/bin/java
        let mac = tmp.path().join("mac");
        std::fs::create_dir_all(mac.join("Contents/Home/bin")).unwrap();
        std::fs::write(mac.join("Contents/Home/bin/java"), b"").unwrap();
        let (home, java_bin) = AdoptiumClient::detect_install(&mac).unwrap();
        assert!(home.ends_with("Contents/Home"));
        assert!(java_bin.ends_with("bin/java"));

        // linux: bin/java at the root
        let linux = tmp.path().join("linux");
        std::fs::create_dir_all(linux.join("bin")).unwrap();
        std::fs::write(linux.join("bin/java"), b"").unwrap();
        let (home, _) = AdoptiumClient::detect_install(&linux).unwrap();
        assert_eq!(home, linux);
    }

    #[test]
    fn extract_strips_leading_dir_and_refuses_escapes() {
        // build a tar.gz with jdk-21/bin/java + a good file
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("jdk.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        {
            let mut tar = tar::Builder::new(gz);
            for (name, data) in [
                ("jdk-21.0.5/bin/java", &b"binary"[..]),
                ("jdk-21.0.5/release", b"metadata"),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, name, data).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        extract_tar_gz(&archive, &out).unwrap();
        assert_eq!(std::fs::read(out.join("bin/java")).unwrap(), b"binary");
        assert_eq!(std::fs::read(out.join("release")).unwrap(), b"metadata");

        // an escaping entry aborts (patch a safe name into "../escaped" —
        // same length, so the header stays structurally valid; the tar
        // Builder itself refuses `..` names, so we patch the raw bytes)
        let mut tar_bytes: Vec<u8> = Vec::new();
        {
            let mut tar = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_cksum();
            tar.append_data(&mut header, "0000safe00", &b"pwn"[..])
                .unwrap();
            tar.finish().unwrap();
        }
        let needle = b"0000safe00";
        let pos = tar_bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap();
        tar_bytes[pos..pos + needle.len()].copy_from_slice(b"../escaped");
        // recompute the header checksum (bytes 148..156 of the block hold it)
        let block_start = pos - pos % 512;
        // the checksum treats its own field as eight ASCII spaces
        tar_bytes[block_start + 148..block_start + 156].copy_from_slice(b"        ");
        let sum: u32 = tar_bytes[block_start..block_start + 512]
            .iter()
            .map(|b| *b as u32)
            .sum();
        tar_bytes[block_start + 148..block_start + 156]
            .copy_from_slice(format!("{sum:06o}\0 ").as_bytes());

        let evil = tmp.path().join("evil.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&evil).unwrap(),
                flate2::Compression::fast(),
            );
            let mut gz = gz;
            std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        }
        let out2 = tmp.path().join("out2");
        std::fs::create_dir_all(&out2).unwrap();
        let err = extract_tar_gz(&evil, &out2).unwrap_err();
        eprintln!("DEBUG evil err: {err:?}");
        assert!(err.to_string().contains("suspicious archive path"), "{err}");
    }

    #[tokio::test]
    async fn available_features_parses_release_info() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/info/available_releases"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "available_lts_releases": [8, 11, 17, 21, 25],
                "most_recent_lts": 25,
                "available_releases": [8, 25, 27]
            })))
            .mount(&server)
            .await;
        let client = AdoptiumClient::with_base(
            crate::http::client(),
            server.uri(),
            tempfile::tempdir().unwrap().path().to_path_buf(),
        );
        let (lts, most_recent) = client.available_features().await.unwrap();
        assert_eq!(lts, vec![8, 11, 17, 21, 25]);
        assert_eq!(most_recent, 25);
    }
}
