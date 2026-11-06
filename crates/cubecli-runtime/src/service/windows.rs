//! Windows: a logon-triggered scheduled task runs `cubecli daemon run` at
//! user level — no admin required, HOME intact, and the Task Scheduler
//! handles restart-on-crash semantics for us (the daemon run loop exits
//! cleanly on ctrl_c; for a service-style restart-on-failure you can point
//! the task's failure settings at relaunching).

use crate::error::{Result, RuntimeError};
use crate::service::ServiceSpec;

pub const TASK: &str = "dev.cubecli.daemon";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> Result<String> {
    let mut cmd = std::process::Command::new("schtasks");
    cmd.args(args);
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
    let output = cmd
        .output()
        .map_err(|e| RuntimeError::Other(format!("cannot run schtasks: {e}")))?;
    if !output.status.success() {
        return Err(RuntimeError::Other(format!(
            "schtasks {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The `/TR` payload: the exe + the daemon run subcommand.
pub fn render(spec: &ServiceSpec) -> String {
    format!("\"{}\" daemon run", spec.exe.display())
}

pub fn install(spec: &ServiceSpec) -> Result<()> {
    let tr = render(spec);
    schtasks(&[
        "/Create", "/TN", TASK, "/TR", &tr, "/SC", "ONLOGON", "/RL", "LIMITED", "/F",
    ])?;
    Ok(())
}

/// Unregister the task. A missing task is not an error (idempotent uninstall).
pub fn uninstall() -> Result<()> {
    let mut cmd = std::process::Command::new("schtasks");
    cmd.args(["/Delete", "/TN", TASK, "/F"]);
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
    let output = cmd
        .output()
        .map_err(|e| RuntimeError::Other(format!("cannot run schtasks: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.to_ascii_lowercase().contains("does not exist") {
            return Err(RuntimeError::Other(format!(
                "schtasks /Delete failed: {}",
                stderr.trim()
            )));
        }
    }
    Ok(())
}

/// Query-based install check (a scheduled task has no definition file).
pub fn is_installed() -> bool {
    let mut cmd = std::process::Command::new("schtasks");
    cmd.args(["/Query", "/TN", TASK]);
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// The task has no file definition; expose the task identifier as the
/// human-facing "path".
pub fn installed_path() -> &'static str {
    "scheduled task dev.cubecli.daemon (schtasks)"
}
