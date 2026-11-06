//! Daemon service installation: launchd user agent (macOS), systemd user
//! unit (Linux), logon-triggered scheduled task (Windows). Idempotent —
//! rewrites the definition only when its content changes, then reloads.
//!
//! Each backend module is compiled only for its target OS; other platforms
//! get a clear "unsupported" error from [`install`]/[`uninstall`].

#[cfg(target_os = "macos")]
pub mod launchd;
#[cfg(target_os = "linux")]
pub mod systemd;
#[cfg(target_os = "windows")]
pub mod windows;

use std::path::PathBuf;

use crate::error::Result;
#[cfg(not(target_os = "windows"))]
use crate::error::RuntimeError;

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
    } else if cfg!(target_os = "linux") {
        "systemd"
    } else if cfg!(target_os = "windows") {
        "schtasks"
    } else {
        "unsupported"
    }
}

#[cfg(target_os = "macos")]
fn launchd_plist_path() -> PathBuf {
    dirs::home_dir()
        .expect("home dir")
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "linux")]
fn systemd_unit_path() -> PathBuf {
    dirs::home_dir()
        .expect("home dir")
        .join(".config/systemd/user/cubecli-daemon.service")
}

pub fn installed_path() -> PathBuf {
    match detect() {
        #[cfg(target_os = "macos")]
        "launchd" => launchd_plist_path(),
        #[cfg(target_os = "linux")]
        "systemd" => systemd_unit_path(),
        // A scheduled task has no definition file — a pseudo-path surfaces
        // the real identifier in `daemon status` output.
        #[cfg(target_os = "windows")]
        "schtasks" => PathBuf::from(windows::installed_path()),
        _ => PathBuf::from(format!(
            "<no service definition for this platform ({})>",
            detect()
        )),
    }
}

/// Is the service registered? File-based mechanisms check the definition
/// path; Windows queries the task.
pub fn is_installed() -> bool {
    #[cfg(target_os = "windows")]
    if detect() == "schtasks" {
        return windows::is_installed();
    }
    installed_path().exists()
}

pub fn definition(spec: &ServiceSpec) -> String {
    let _ = spec; // unused on platforms without a renderer
    match detect() {
        #[cfg(target_os = "macos")]
        "launchd" => launchd::render(spec),
        #[cfg(target_os = "linux")]
        "systemd" => systemd::render(spec),
        #[cfg(target_os = "windows")]
        "schtasks" => windows::render(spec),
        _ => String::new(),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn unsupported() -> RuntimeError {
    RuntimeError::Other(format!(
        "daemon service installation is not supported on this platform ({}) yet",
        detect()
    ))
}

/// Install (or refresh) the service definition and load it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
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
        #[cfg(target_os = "macos")]
        "launchd" => launchd::reload(&path),
        #[cfg(target_os = "linux")]
        "systemd" => systemd::reload(),
        // detect() agrees with the cfg gates above.
        _ => unreachable!(),
    }
}

/// Scheduled-task install (Windows).
#[cfg(target_os = "windows")]
pub fn install(spec: &ServiceSpec) -> Result<()> {
    windows::install(spec)
}

/// No service mechanism on other platforms yet — say so clearly.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn install(_spec: &ServiceSpec) -> Result<()> {
    Err(unsupported())
}

/// Unload and remove the service definition.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn uninstall() -> Result<()> {
    let path = installed_path();
    match detect() {
        #[cfg(target_os = "macos")]
        "launchd" => launchd::unload(&path)?,
        #[cfg(target_os = "linux")]
        "systemd" => systemd::unload()?,
        // detect() agrees with the cfg gates above.
        _ => unreachable!(),
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(RuntimeError::Io { path, source }),
    }
    Ok(())
}

/// Unregister the scheduled task (Windows).
#[cfg(target_os = "windows")]
pub fn uninstall() -> Result<()> {
    windows::uninstall()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn uninstall() -> Result<()> {
    Err(unsupported())
}
