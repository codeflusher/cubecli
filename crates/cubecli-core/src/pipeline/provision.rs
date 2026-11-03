//! The provisioning pipeline: JDK-first ordering, downloads, post-install
//! steps, managed files. Generic over [`FileFetcher`] / [`JavaLocator`] /
//! [`JavaProvisioner`] so tests can mock everything.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::error::{CoreError, Result};
use crate::model::Checksum;
use crate::paths::ProjectLayout;
use crate::traits::java::{JavaInstall, JavaLocator, JavaOrigin, JavaProvisioner};
use crate::traits::progress::ProgressReporter;
use crate::traits::progress::TracingProgress;
use crate::traits::server_core::InstallPlan;

/// Transport abstraction — implemented by `cubecli-providers::download`.
#[async_trait]
pub trait FileFetcher: Send + Sync {
    /// Stream `url` into `dest` (absolute), verify `checksum`, atomic rename.
    async fn fetch(
        &self,
        url: &Url,
        checksum: Option<&Checksum>,
        dest: &Path,
        progress: &dyn ProgressReporter,
    ) -> Result<()>;
}

/// What provisioning produced.
#[derive(Debug, Clone)]
pub struct ProvisionReport {
    pub java: JavaInstall,
    pub launch: crate::model::LaunchSpec,
}

/// Knobs for one provisioning run.
pub struct ProvisionOptions<'a> {
    pub fetcher: &'a dyn FileFetcher,
    pub java_locator: &'a dyn JavaLocator,
    pub java_provisioner: &'a dyn JavaProvisioner,
    /// Auto-install a missing JDK instead of failing with `JavaMissing`.
    pub auto_install_java: bool,
    pub progress: &'a dyn ProgressReporter,
}

impl<'a> ProvisionOptions<'a> {
    pub fn new(
        fetcher: &'a dyn FileFetcher,
        java_locator: &'a dyn JavaLocator,
        java_provisioner: &'a dyn JavaProvisioner,
    ) -> Self {
        Self {
            fetcher,
            java_locator,
            java_provisioner,
            auto_install_java: true,
            progress: &TracingProgress,
        }
    }
}

/// Execute an [`InstallPlan`] inside `layout`.
///
/// Order matters and is enforced here, never per-core: JDK (forge/neoforge
/// installers need it) → downloads → post-install steps → report. Managed
/// files (eula/properties) are written by the caller around this.
pub async fn provision(
    layout: &ProjectLayout,
    plan: &InstallPlan,
    options: &ProvisionOptions<'_>,
) -> Result<ProvisionReport> {
    options
        .progress
        .message(&format!("ensuring Java {} is available", plan.java.major));

    // 1. JDK first — forge/neoforge installers refuse to run without it.
    let java = match options.java_locator.locate(plan.java) {
        Ok(install) => {
            if install.origin == JavaOrigin::System {
                options.progress.message(&format!(
                    "warning: using system Java {} — `cubecli setup java install {}` is the recommended setup",
                    install.feature, install.feature
                ));
            }
            install
        }
        Err(_) if options.auto_install_java => {
            options
                .java_provisioner
                .ensure(plan.java, options.progress)
                .await?
        }
        Err(e) => return Err(e),
    };

    // 2. Downloads (`.part` + checksum + atomic rename live in the fetcher).
    for spec in &plan.downloads {
        let dest = layout.root().join(&spec.dest);
        options
            .progress
            .message(&format!("downloading {}", spec.dest.display()));
        options
            .fetcher
            .fetch(&spec.url, spec.checksum.as_ref(), &dest, options.progress)
            .await?;
        #[cfg(unix)]
        if spec.executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| CoreError::io(&dest, e))?;
        }
    }

    // 3. Post-install steps, in order.
    for step in &plan.steps {
        match step {
            crate::traits::server_core::PostInstallStep::WriteTextFile { path, contents } => {
                let dest = layout.root().join(path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
                }
                let existing = std::fs::read_to_string(&dest).unwrap_or_default();
                let text = format!("{existing}{contents}");
                std::fs::write(&dest, text).map_err(|e| CoreError::io(&dest, e))?;
            }
            crate::traits::server_core::PostInstallStep::RunCommand {
                argv,
                needs_java,
                timeout,
                expect_paths,
            } => {
                run_command_step(layout, argv, *needs_java, &java, *timeout, options.progress)
                    .await?;
                for expected in expect_paths {
                    let p = layout.root().join(expected);
                    if !p.exists() {
                        return Err(CoreError::Runtime(format!(
                            "installer finished but {} is missing — install did not complete",
                            expected.display()
                        )));
                    }
                }
            }
        }
    }

    Ok(ProvisionReport {
        java,
        launch: plan.launch.clone(),
    })
}

/// Run one installer command with the resolved JDK, streaming output to trace.
async fn run_command_step(
    layout: &ProjectLayout,
    argv: &[String],
    needs_java: bool,
    java: &JavaInstall,
    timeout: Duration,
    progress: &dyn ProgressReporter,
) -> Result<()> {
    let mut command: Vec<String> = argv.to_vec();
    if needs_java {
        // Providers write `java` as argv[0]; substitute the resolved binary.
        if command.first().is_some_and(|c| c == "java") {
            command[0] = java.java_bin.display().to_string();
        }
    }
    progress.message(&format!("running: {}", command.join(" ")));

    let mut cmd = tokio::process::Command::new(&command[0]);
    cmd.args(&command[1..])
        .current_dir(layout.root())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Installer scripts sometimes call plain `java` themselves.
    if let Some(java_home) = java.home.parent() {
        let mut path = std::env::var_os("PATH").unwrap_or_default();
        let mut with_java = std::ffi::OsString::from(java.home.join("bin"));
        with_java.push(":");
        with_java.push(&path);
        path = with_java;
        let _ = java_home;
        cmd.env("PATH", path);
    }

    let child = cmd
        .spawn()
        .map_err(|e| CoreError::Runtime(format!("cannot run installer: {e}")))?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| CoreError::Runtime(format!("installer timed out after {timeout:?}")))?
        .map_err(|e| CoreError::Runtime(format!("installer failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let tail: String = stderr
            .lines()
            .chain(stdout.lines())
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(CoreError::Runtime(format!(
            "installer exited with {}: {}",
            output.status,
            if tail.is_empty() {
                "(no output)".into()
            } else {
                tail
            }
        )));
    }
    Ok(())
}

/// Best-effort disk preflight (warn-only): `min_disk_mb` hint for callers.
#[allow(dead_code)]
pub fn warn_if_low_disk(root: &Path, needed_mb: u64) {
    // std has no free-space API; platforms use statvfs — v1 keeps this a no-op
    // and relies on download errors instead.
    let _ = (root, needed_mb);
}

/// Convenience: type alias used by provider implementations.
pub type PathRef = PathBuf;
