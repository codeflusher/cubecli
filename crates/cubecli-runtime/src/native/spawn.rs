//! Detached server spawn: own session (survives terminal close), stdout +
//! stderr merged into the log file, stdin null.

use std::path::Path;

use crate::error::{Result, RuntimeError};

/// Spawn fully detached; returns the pid (which is also the pgid).
pub fn spawn_detached(
    java_bin: &Path,
    argv: &[String],
    root: &Path,
    log_file: &Path,
) -> Result<u32> {
    use std::process::Stdio;

    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RuntimeError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .map_err(|e| RuntimeError::Io {
            path: log_file.to_path_buf(),
            source: e,
        })?;
    let log_err = log.try_clone().map_err(|e| RuntimeError::Io {
        path: log_file.to_path_buf(),
        source: e,
    })?;

    let mut cmd = std::process::Command::new(java_bin);
    cmd.args(argv.iter().skip(1))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .env("PATH", {
            let mut p =
                std::ffi::OsString::from(java_bin.parent().unwrap_or(Path::new("/usr/bin")));
            p.push(":");
            p.push(std::env::var_os("PATH").unwrap_or_default());
            p
        });

    // New session: full detach from the controlling terminal, pgid == pid.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let child = cmd.spawn().map_err(|e| {
        RuntimeError::Other(format!(
            "cannot spawn server ({}): {}",
            java_bin.display(),
            e
        ))
    })?;
    let pid = child.id();
    tracing::info!(pid, "server spawned");
    Ok(pid)
}
