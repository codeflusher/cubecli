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

    // 3. SIGTERM the process group (shutdown hooks still save)
    if {
        reap_children();
        signals::pid_alive(pf.pid)
    } {
        stage = StopStage::Signal;
        signals::kill_group(pf.pid, signals::SIGTERM);
        let term_deadline = Instant::now() + grace.term;
        loop {
            reap_children();
            if !signals::pid_alive(pf.pid) || Instant::now() >= term_deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    // 4. SIGKILL
    if {
        reap_children();
        signals::pid_alive(pf.pid)
    } {
        stage = StopStage::Kill;
        signals::kill_group(pf.pid, signals::SIGKILL);
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

fn write_manual_stop(layout: &ProjectLayout) {
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

fn rcon_port_of(layout: &ProjectLayout) -> u16 {
    // Read from server.properties (source of truth for the running server);
    // fall back to the default when the file is missing.
    crate::native::properties_port(&layout.root().join("server.properties")).unwrap_or(25575)
}
