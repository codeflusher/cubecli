//! The shared stop sequence: RCON `stop` → grace → SIGTERM group → SIGKILL.
//! Used by explicit `project stop` (which writes the manually-stopped
//! marker) and by the daemon's restarts (which must not).

use std::time::{Duration, Instant};

use cubecli_core::paths::ProjectLayout;
use cubecli_core::traits::runtime::{StopGrace, StopOutcome, StopStage};

use crate::error::{Result, RuntimeError};
use crate::native::{pidfile, signals};
use crate::rcon;

/// Stop the server of `layout` (if running). `manual` writes the
/// manually-stopped marker so the daemon's unless-stopped policy leaves it
/// down; daemon-initiated stops pass `false`.
pub async fn graceful(
    layout: &ProjectLayout,
    grace: StopGrace,
    manual: bool,
) -> Result<StopOutcome> {
    let pidfile_path = layout.pidfile();
    let Some(pf) = pidfile::read(&pidfile_path)? else {
        if manual {
            write_manual_stop(layout);
        }
        return Ok(StopOutcome {
            stage: StopStage::AlreadyStopped,
            waited: Duration::ZERO,
        });
    };

    let started = Instant::now();
    let mut stage = StopStage::Signal;

    // The marker goes down FIRST: the daemon ticks while this stop runs and
    // must not "crash-restart" the server we are deliberately stopping.
    if manual {
        write_manual_stop(layout);
    }

    // 1. the clean path: RCON `stop` (saves the world first)
    if let Ok(secret) = std::fs::read_to_string(layout.rcon_secret_file()) {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], rcon_port_of(layout)));
        let probe =
            tokio::time::timeout(Duration::from_secs(2), rcon::connect(addr, secret.trim())).await;
        if let Ok(Ok(mut client)) = probe {
            if client.command("stop").await.is_ok() {
                stage = StopStage::Rcon;
            }
        }
    } else {
        // No secret — fall through to signals.
    }

    // 2. wait for exit within the soft grace
    // (reap before each probe: a killed child of this process is a zombie
    // until waited, and kill(pid, 0) happily reports zombies as alive)
    let soft_deadline = Instant::now() + grace.soft;
    loop {
        reap_children();
        if !signals::pid_alive(pf.pid) || Instant::now() >= soft_deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // 3. SIGTERM the process group (unix) / taskkill the tree (windows) —
    // shutdown hooks still save the world
    if {
        reap_children();
        signals::pid_alive(pf.pid)
    } {
        stage = StopStage::Signal;
        terminate_tree(pf.pid);
        let term_deadline = Instant::now() + grace.term;
        loop {
            reap_children();
            if !signals::pid_alive(pf.pid) || Instant::now() >= term_deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    // 4. SIGKILL (unix) / taskkill /F (windows)
    if {
        reap_children();
        signals::pid_alive(pf.pid)
    } {
        stage = StopStage::Kill;
        force_kill_tree(pf.pid);
        let kill_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            reap_children();
            if !signals::pid_alive(pf.pid) || Instant::now() >= kill_deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if {
            reap_children();
            signals::pid_alive(pf.pid)
        } {
            return Err(RuntimeError::Other(format!(
                "server pid {} refused to die even after SIGKILL",
                pf.pid
            )));
        }
    }

    pidfile::remove(&pidfile_path);
    Ok(StopOutcome {
        stage,
        waited: started.elapsed(),
    })
}

/// Remove the manually-stopped marker (called by `project start`).
pub fn clear_manual_stop(layout: &ProjectLayout) {
    let _ = std::fs::remove_file(layout.manually_stopped_marker());
}

/// Write the manually-stopped marker. Also used by the compose backend —
/// every explicit stop must lay the marker down BEFORE the stop call lands
/// (the daemon ticks concurrently).
pub(crate) fn write_manual_stop(layout: &ProjectLayout) {
    let marker = layout.manually_stopped_marker();
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, "explicit `cubecli project stop`\n");
}

/// Reap any children of this process (unix). Detached servers are usually
/// orphaned to launchd/systemd — this only matters when the same process
/// spawned and stopped the server (the daemon's restart path).
#[cfg(unix)]
fn reap_children() {
    // SAFETY: waitpid with WNOHANG only reaps our own children.
    loop {
        let mut status = 0;
        let rc = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if rc <= 0 {
            break;
        }
    }
}

#[cfg(not(unix))]
fn reap_children() {}

/// Graceful-ish tree termination: SIGTERM to the process group (unix) or
/// `taskkill /T` without /F (windows — sends WM_CLOSE; a JVM mostly ignores
/// it, but it is the polite step before the hard kill).
#[cfg(unix)]
fn terminate_tree(pid: u32) {
    signals::kill_group(pid, signals::SIGTERM);
}

#[cfg(windows)]
fn terminate_tree(pid: u32) {
    let _ = taskkill(pid, false);
}

/// Hard tree kill: SIGKILL (unix) or `taskkill /T /F` (windows).
#[cfg(unix)]
fn force_kill_tree(pid: u32) {
    signals::kill_group(pid, signals::SIGKILL);
}

#[cfg(windows)]
fn force_kill_tree(pid: u32) {
    let _ = taskkill(pid, true);
}

/// Windows tree kill via the bundled CLI. `/T` walks child processes, `/F`
/// terminates unconditionally.
#[cfg(windows)]
fn taskkill(pid: u32, force: bool) -> bool {
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T"]);
    if force {
        cmd.arg("/F");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn rcon_port_of(layout: &ProjectLayout) -> u16 {
    // Read from server.properties (source of truth for the running server);
    // fall back to the default when the file is missing.
    crate::native::properties_port(&layout.root().join("server.properties")).unwrap_or(25575)
}
