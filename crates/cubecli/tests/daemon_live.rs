//! The daemon tick loop against a real project directory: registers via the
//! real registry file, writes a real heartbeat, executes a due auto-backup.
//! Runs the actual binary as the daemon would.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;

struct Env {
    home: PathBuf,
    cwd: TempDir,
}

fn bin() -> (Command, Env) {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let mut c = Command::cargo_bin("cubecli").unwrap();
    c.env("HOME", home.path()).env("CUBECLI_NO_DOWNLOAD", "1");
    (
        c,
        Env {
            home: home.path().to_path_buf(),
            cwd,
        },
    )
}

/// The heartbeat proves the tick loop runs; a due 1s backup rule proves the
/// engine→executor wiring end to end.
#[test]
fn daemon_ticks_execute_due_backups_and_write_heartbeat() {
    let (mut cmd, env) = bin();
    let project = env.cwd.path().join("s");
    std::fs::create_dir_all(&project).unwrap();

    cmd.current_dir(&project)
        .args(["create", "--yes"])
        .assert()
        .success();

    // world content so the backup has something in it
    std::fs::create_dir_all(project.join("world")).unwrap();
    std::fs::write(project.join("world/level.dat"), b"level").unwrap();

    Command::cargo_bin("cubecli")
        .unwrap()
        .env("HOME", &env.home)
        .env("CUBECLI_NO_DOWNLOAD", "1")
        .current_dir(&project)
        .args(["project", "config", "auto-backup=true"])
        .assert()
        .success();
    Command::cargo_bin("cubecli")
        .unwrap()
        .env("HOME", &env.home)
        .env("CUBECLI_NO_DOWNLOAD", "1")
        .current_dir(&project)
        .args(["project", "config", "auto-backup-rule=1s"])
        .assert()
        .success();

    // run the daemon briefly (fresh state → backup due immediately)
    let mut daemon = std::process::Command::new(env!("CARGO_BIN_EXE_cubecli"))
        .env("HOME", &env.home)
        .env("CUBECLI_NO_DOWNLOAD", "1")
        .args(["daemon", "run", "--interval", "1"])
        .current_dir(env.cwd.path())
        .spawn()
        .expect("daemon spawns");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut backups_seen = false;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        let backups = project.join(".cubecli/backups");
        if backups.is_dir() && std::fs::read_dir(&backups).map(|d| d.count()).unwrap_or(0) > 0 {
            backups_seen = true;
            break;
        }
    }
    daemon.kill().unwrap();
    let _ = daemon.wait();

    assert!(backups_seen, "daemon never produced the due backup");
    let heartbeat = std::fs::read_to_string(env.home.join(".cubecli/daemon/heartbeat")).unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(heartbeat.trim()).is_ok());

    // the daemon state file tracks the backup
    let state = std::fs::read_to_string(env.home.join(".cubecli/daemon/state/s.json")).unwrap();
    assert!(state.contains("last_backup_at"), "{state}");
}

/// Second daemon refuses to run while the first holds the lock.
#[test]
fn daemon_single_instance_lock() {
    let (mut cmd, env) = bin();
    let project = env.cwd.path().join("s");
    std::fs::create_dir_all(&project).unwrap();
    cmd.current_dir(&project)
        .args(["create", "--yes"])
        .assert()
        .success();

    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_cubecli"))
        .env("HOME", &env.home)
        .args(["daemon", "run", "--interval", "60"])
        .current_dir(env.cwd.path())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let second = Command::cargo_bin("cubecli")
        .unwrap()
        .env("HOME", &env.home)
        .args(["daemon", "run", "--interval", "60"])
        .current_dir(env.cwd.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("already running"), "{stderr}");

    first.kill().unwrap();
    let _ = first.wait();
}
