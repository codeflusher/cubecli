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
    // java's bin dir first, then the inherited PATH (join_paths picks the
    // platform separator).
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        [java_bin
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| Path::new("/usr/bin").to_path_buf())]
        .into_iter()
        .chain(std::env::split_paths(&inherited)),
    )
    .unwrap_or(inherited);
    cmd.args(argv.iter().skip(1))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .env("PATH", path);

    // New session: full detach from the controlling terminal, pgid == pid.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    // Windows: no setsid — just avoid flashing a console window. (Group
    // lifecycle differs until the native Windows backend; stop uses
    // taskkill /T.)
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
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
