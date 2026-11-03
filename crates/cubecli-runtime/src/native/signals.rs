//! Process liveness and group signals. Detached servers get their own
//! session (`setsid`), so `pgid == pid` and `kill(-pid, sig)` reaches the
//! whole tree.

/// True when a process with this pid exists and belongs to us (or root).
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only probes existence.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Signal the process group led by `pid` (which is the pid under setsid).
pub fn kill_group(pid: u32, sig: i32) -> bool {
    // SAFETY: plain signal delivery, no memory involved.
    unsafe { libc::kill(-(pid as libc::pid_t), sig) == 0 }
}

pub const SIGTERM: i32 = libc::SIGTERM;
pub const SIGKILL: i32 = libc::SIGKILL;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pid_alive_detects_sleeping_child() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        assert!(pid_alive(pid));
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        assert!(!pid_alive(pid));
    }

    #[tokio::test]
    async fn kill_group_signals_children() {
        use std::process::Stdio;
        // Own process group — mirrors the setsid'd server (pgid == pid).
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "trap '' TERM; sleep 60"])
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(pid_alive(pid));
        assert!(kill_group(pid, SIGTERM));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(pid_alive(pid), "TERM-ignoring child survives TERM");
        assert!(kill_group(pid, SIGKILL));
        // Reap: our own children stay zombies until waited, which pid_alive
        // would then still see. Detached servers are reaped by launchd.
        let _ = child.wait().await;
        assert!(!pid_alive(pid), "SIGKILL must finish it");
    }
}
