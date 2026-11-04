//! Host environment probes: WSL, systemd, docker. Read-only — used by
//! `cubecli doctor` and preflight checks.

/// Pure parser: does `/proc/version` text indicate a Microsoft WSL kernel?
pub fn proc_version_is_wsl(text: &str) -> bool {
    text.to_ascii_lowercase().contains("microsoft")
}

/// WSL major version from `/proc/version` text (WSL2 kernels self-identify;
/// WSL1 only says "Microsoft").
pub fn proc_version_wsl_version(text: &str) -> Option<u8> {
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("wsl2") {
        Some(2)
    } else if proc_version_is_wsl(text) {
        Some(1)
    } else {
        None
    }
}

#[cfg(unix)]
fn proc_version() -> Option<String> {
    std::fs::read_to_string("/proc/version").ok()
}

/// Are we running inside WSL? (Linux only — `/proc/version` probe.)
pub fn is_wsl() -> bool {
    #[cfg(unix)]
    {
        proc_version()
            .map(|text| proc_version_is_wsl(&text))
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// WSL major version, when running inside WSL.
pub fn wsl_version() -> Option<u8> {
    #[cfg(unix)]
    {
        proc_version().and_then(|text| proc_version_wsl_version(&text))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Is a systemd user session available? `daemon install` on Linux (and WSL)
/// needs one; WSL ships without it unless `[boot] systemd=true` is set.
#[cfg(unix)]
pub fn systemd_available() -> bool {
    std::path::Path::new("/run/systemd/system").exists()
}

#[cfg(not(unix))]
pub fn systemd_available() -> bool {
    false
}

/// What `docker_probe` found about the local docker installation.
#[derive(Debug, Clone, Default)]
pub struct DockerProbe {
    pub cli_found: bool,
    pub daemon_ok: bool,
    pub server_version: Option<String>,
    pub context: Option<String>,
}

impl DockerProbe {
    /// Is the docker CLI usable at all (compose plugin aside)?
    pub fn usable(&self) -> bool {
        self.cli_found && self.daemon_ok
    }
}

/// Probe the docker CLI and daemon. `CUBECLI_DOCKER_BIN` (tests) and
/// `DOCKER_HOST` / docker contexts are honored by the CLI itself.
pub async fn docker_probe() -> DockerProbe {
    let exe = std::env::var_os("CUBECLI_DOCKER_BIN").unwrap_or_else(|| "docker".into());
    let mut probe = DockerProbe::default();
    let info = tokio::process::Command::new(&exe)
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .await;
    let Ok(output) = info else {
        return probe; // CLI not found
    };
    probe.cli_found = true;
    if output.status.success() {
        probe.daemon_ok = true;
        probe.server_version = Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    if let Ok(ctx) = tokio::process::Command::new(&exe)
        .args(["context", "show"])
        .output()
        .await
    {
        if ctx.status.success() {
            probe.context = Some(String::from_utf8_lossy(&ctx.stdout).trim().to_string());
        }
    }
    probe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_version_parsing() {
        let wsl2 = "Linux version 5.15.167.4-microsoft-standard-WSL2 (root@...) #1 SMP x86_64";
        let wsl1 = "Linux version 4.4.0-19041-Microsoft (Microsoft@Microsoft.com) #1236-Microsoft";
        let linux = "Linux version 6.8.0-45-generic (buildd@lcy02-amd64) #59-Ubuntu SMP";

        assert!(proc_version_is_wsl(wsl2));
        assert!(proc_version_is_wsl(wsl1));
        assert!(!proc_version_is_wsl(linux));

        assert_eq!(proc_version_wsl_version(wsl2), Some(2));
        assert_eq!(proc_version_wsl_version(wsl1), Some(1));
        assert_eq!(proc_version_wsl_version(linux), None);
    }
}
