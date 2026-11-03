//! Live-API integration specs — hit real Mojang/Paper/Modrinth endpoints
//! and download real jars. Opt-in: `CUBECLI_LIVE=1 cargo test --test cli_live`.
//! These are slow (tens of MB) and depend on third-party uptime; CI runs the
//! offline `cli_e2e.rs` suite instead.

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

fn live() -> bool {
    std::env::var("CUBECLI_LIVE").ok().as_deref() == Some("1")
}

fn bin() -> Command {
    let mut c = Command::cargo_bin("cubecli").unwrap();
    c.env_remove("CUBECLI_NO_DOWNLOAD");
    c
}

/// create --yes provisions a real paper server: Fill v3 listing → build JSON
/// → sha256-verified download → launch spec in meta.
#[test]
#[ignore = "live API + ~50 MB download; run with CUBECLI_LIVE=1"]
fn live_create_provisions_bootable_paper() {
    if !live() {
        return;
    }
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap().path().join("server");
    std::fs::create_dir_all(&project).unwrap();

    bin()
        .env("HOME", home.path())
        .current_dir(&project)
        .args(["create", "--yes"])
        .assert()
        .success()
        .stdout(contains("created project"));

    let meta = std::fs::read_to_string(project.join(".cubecli.meta")).unwrap();
    assert!(meta.contains("eula_accepted = false"));
    assert!(meta.contains("[server.launch]"));
    // the downloaded jar exists and meta points at it
    let jar: Vec<_> = std::fs::read_dir(&project)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".jar"))
        .collect();
    assert_eq!(jar.len(), 1, "exactly the paper jar was downloaded");
}

/// Modrinth install end-to-end against the real registry (small file).
#[test]
#[ignore = "live API; run with CUBECLI_LIVE=1"]
fn live_modrinth_addon_install() {
    if !live() {
        return;
    }
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap().path().join("server");
    std::fs::create_dir_all(&project).unwrap();

    bin()
        .env("HOME", home.path())
        .env("CUBECLI_NO_DOWNLOAD", "1")
        .current_dir(&project)
        .args([
            "create",
            "--yes",
            "--core",
            "fabric",
            "--mc-version",
            "1.21.8",
        ])
        .assert()
        .success();

    bin()
        .env("HOME", home.path())
        .current_dir(&project)
        .args(["project", "install", "mod:fabric-api"])
        .assert()
        .success()
        .stdout(contains("installed mod"));

    let mods: Vec<_> = std::fs::read_dir(project.join("mods"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(mods.len(), 1);

    bin()
        .env("HOME", home.path())
        .current_dir(&project)
        .args(["project", "addon-list", "--outdated"])
        .assert()
        .success()
        .stdout(contains("fabric-api"));
}

/// Paper upgrade check against the live Fill API.
#[test]
#[ignore = "live API; run with CUBECLI_LIVE=1"]
fn live_upgrade_check() {
    if !live() {
        return;
    }
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap().path().join("server");
    std::fs::create_dir_all(&project).unwrap();

    bin()
        .env("HOME", home.path())
        .env("CUBECLI_NO_DOWNLOAD", "1")
        .current_dir(&project)
        .args(["create", "--yes"])
        .assert()
        .success();

    bin()
        .env("HOME", home.path())
        .current_dir(&project)
        .args(["project", "upgrade", "--check"])
        .assert()
        .success()
        .stdout(contains("upgrade available"));
}
