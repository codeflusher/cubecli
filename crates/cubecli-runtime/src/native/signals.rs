//! Process liveness and group signals. Detached servers get their own
//! session (`setsid`), so `pgid == pid` and `kill(-pid, sig)` reaches the
//! whole tree.
//!
//! Unix is the supported native-backend platform; the Windows stubs keep the
//! target compiling with degraded liveness probes until the native Windows
//! backend lands.

/// True when a process with this pid exists and belongs to us (or root).
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only probes existence.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Signal the process group led by `pid` (which is the pid under setsid).
#[cfg(unix)]
pub fn kill_group(pid: u32, sig: i32) -> bool {
    // SAFETY: plain signal delivery, no memory involved.
    unsafe { libc::kill(-(pid as libc::pid_t), sig) == 0 }
}

#[cfg(unix)]
pub const SIGTERM: i32 = libc::SIGTERM;
#[cfg(unix)]
pub const SIGKILL: i32 = libc::SIGKILL;

// Windows: liveness via a query-only process handle (exit code != STILL_ACTIVE).
// Group signalling has no equivalent — the stop ladder uses `taskkill /T`.
#[cfg(windows)]
pub fn pid_alive(pid: u32) -> bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    // Minimal kernel32 bindings — a full `windows`-crate dependency just for
    // two calls is not worth it.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(
            desired_access: u32,
            inherit_handle: i32,
            process_id: u32,
        ) -> *mut core::ffi::c_void;
        fn GetExitCodeProcess(process: *mut core::ffi::c_void, exit_code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
    }
    // SAFETY: query-only handle, closed before return; no memory is shared.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        ok != 0 && exit_code != STILL_ACTIVE
    }
}

#[cfg(windows)]
pub fn kill_group(_pid: u32, _sig: i32) -> bool {
    false // no process groups on Windows; the stop ladder uses taskkill
}

#[cfg(windows)]
pub const SIGTERM: i32 = 15;
#[cfg(windows)]
pub const SIGKILL: i32 = 9;

#[cfg(all(test, unix))]
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
