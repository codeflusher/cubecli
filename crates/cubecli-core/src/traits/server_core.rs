//! Server core providers — one implementation per core (vanilla, paper,
//! purpur, fabric, neoforge, forge).

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::error::Result;
use crate::model::{
    BuildChannel, Checksum, CoreBuild, CoreKind, JavaRequirement, LaunchSpec, McVersion,
};

/// The currently installed core of a project (for upgrade checks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCore {
    pub core: CoreKind,
    pub mc_version: String,
    pub build: String,
}

/// Everything needed to provision one (mc_version, build) of a core.
/// Download URLs are always taken verbatim from the provider API —
/// never constructed client-side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    /// Java feature required for the installer AND at runtime.
    pub java: JavaRequirement,
    pub downloads: Vec<DownloadSpec>,
    /// Executed in order after the downloads.
    pub steps: Vec<PostInstallStep>,
    /// Recorded into meta and used by both runtime backends.
    pub launch: LaunchSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadSpec {
    pub url: Url,
    pub checksum: Option<Checksum>,
    /// Destination relative to the project root.
    pub dest: PathBuf,
    pub executable: bool,
}

/// A step the provisioning pipeline runs after the downloads land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostInstallStep {
    /// e.g. `java -jar installer.jar --installServer` for forge/neoforge.
    /// `expect_paths` (relative to project root) must exist afterwards or
    /// provisioning fails fast.
    RunCommand {
        argv: Vec<String>,
        /// Run with the resolved JDK instead of the current `java`.
        needs_java: bool,
        timeout: Duration,
        expect_paths: Vec<PathBuf>,
    },
    WriteTextFile {
        /// Relative to project root.
        path: PathBuf,
        contents: String,
    },
}

#[async_trait]
pub trait ServerCoreProvider: Send + Sync {
    fn kind(&self) -> CoreKind;

    /// Servable MC versions, newest first. Snapshots filtered out unless the
    /// provider has no stable filtering.
    async fn list_versions(&self) -> Result<Vec<McVersion>>;

    /// Builds for one MC version, newest first. Vanilla/fabric return a
    /// single synthetic build.
    async fn list_builds(&self, mc_version: &str) -> Result<Vec<CoreBuild>>;

    /// Newest build on the given channel.
    async fn latest_build(&self, mc_version: &str, channel: BuildChannel) -> Result<CoreBuild>;

    /// For `project upgrade`: is there a newer build than `current`?
    async fn check_upgrade(&self, current: &InstalledCore) -> Result<Option<CoreBuild>>;

    /// Concrete, verifiable install plan for a pinned (mc_version, build).
    async fn install_plan(&self, mc_version: &str, build: &str) -> Result<InstallPlan>;
}
