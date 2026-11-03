//! Daemon service installation: launchd user agent (macOS) / systemd user
//! unit (Linux). Idempotent — rewrites the definition only when its content
//! changes, then reloads.

pub mod launchd;
pub mod systemd;

use std::path::PathBuf;

use crate::error::{Result, RuntimeError};

pub const LABEL: &str = "dev.cubecli.daemon";
pub const SYSTEMD_UNIT: &str = "cubecli-daemon.service";

/// What the service needs to know about the executable and its state.
#[derive(Debug, Clone)]
pub struct ServiceSpec {
    pub exe: PathBuf,
    pub log_out: PathBuf,
    pub log_err: PathBuf,
}

/// Which service system applies on this machine.
pub fn detect() -> &'static str {
    if cfg!(target_os = "macos") {
        "launchd"
    } else {
        "systemd"
    }
}

fn launchd_plist_path() -> PathBuf {
    dirs::home_dir()
        .expect("home dir")
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn systemd_unit_path() -> PathBuf {
    dirs::home_dir()
        .expect("home dir")
        .join(".config/systemd/user/cubecli-daemon.service")
}

pub fn installed_path() -> PathBuf {
    match detect() {
        "launchd" => launchd_plist_path(),
        _ => systemd_unit_path(),
    }
}

pub fn definition(spec: &ServiceSpec) -> String {
    match detect() {
        "launchd" => launchd::render(spec),
        _ => systemd::render(spec),
    }
}

/// Install (or refresh) the service definition and load it.
pub fn install(spec: &ServiceSpec) -> Result<()> {
    let path = installed_path();
    let definition = definition(spec);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RuntimeError::io(parent, e))?;
    }
    // Idempotent: skip rewriting (and reloading) unchanged definitions.
    if std::fs::read_to_string(&path).ok().as_deref() == Some(definition.as_str()) {
        return Ok(());
    }
    std::fs::write(&path, &definition).map_err(|e| RuntimeError::io(&path, e))?;
    match detect() {
        "launchd" => launchd::reload(&path),
        _ => systemd::reload(),
    }
}

/// Unload and remove the service definition.
pub fn uninstall() -> Result<()> {
    let path = installed_path();
    match detect() {
        "launchd" => launchd::unload(&path)?,
        _ => systemd::unload()?,
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(RuntimeError::Io { path, source }),
    }
    Ok(())
}
