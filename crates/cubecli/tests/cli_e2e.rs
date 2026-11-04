//! Integration specs driving the real `cubecli` binary via assert_cmd.
//! Each test gets an isolated HOME (so `~/.cubecli` lands in a tempdir) and
//! an isolated project dir. Network-dependent provisioning is disabled via
//! `CUBECLI_NO_DOWNLOAD=1` so the suite runs fully offline; live-API specs
//! live in `cli_live.rs` behind `CUBECLI_LIVE=1`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

/// HOME mutation is process-global — serialize tests that rely on it.
static HOME_LOCK: Mutex<()> = Mutex::new(());

struct Env {
    home: PathBuf,
    cwd: TempDir,
}

fn cli() -> (Command, Env) {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let mut command = Command::cargo_bin("cubecli").unwrap();
    command
        .env("HOME", home.path())
        .env("CUBECLI_NO_DOWNLOAD", "1")
        .current_dir(cwd.path());
    (
        command,
        Env {
            home: home.path().to_path_buf(),
            cwd,
        },
    )
}

fn create_project(env: &Env, name: &str) -> PathBuf {
    let project = env.cwd.path().join(name);
    std::fs::create_dir_all(&project).unwrap();
    Command::cargo_bin("cubecli")
        .unwrap()
        .env("HOME", &env.home)
        .env("CUBECLI_NO_DOWNLOAD", "1")
        .current_dir(&project)
        .args(["create", "--yes"])
        .assert()
        .success();
    project
}

fn write_world(dir: &Path) {
    let world = dir.join("world/region");
    std::fs::create_dir_all(&world).unwrap();
    std::fs::write(dir.join("world/level.dat"), b"level").unwrap();
    std::fs::write(world.join("r.0.0.mca"), b"region").unwrap();
    std::fs::write(dir.join("server.properties"), "# mine\nview-distance=10\n").unwrap();
}

// ------------------------------------------------------------------------------------------------
// surface

#[test]
fn version_and_help() {
    let (mut cmd, _env) = cli();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(contains("cubecli 0.1.0"));

    let (mut cmd, _env) = cli();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("Create, configure"));

    let (mut cmd, _env) = cli();
    cmd.args(["project", "--help"])
        .assert()
        .success()
        .stdout(contains("Manage the server project"));
}

#[test]
fn unknown_command_fails_with_usage_code() {
    let (mut cmd, _env) = cli();
    cmd.arg("definitely-not-a-command")
        .assert()
        .failure()
        .code(2);
}

// ------------------------------------------------------------------------------------------------
// create

#[test]
fn create_yes_builds_project_skeleton_offline() {
    let (mut cmd, env) = cli();
    let project = create_project(&env, "server");

    for file in [
        ".cubecli.meta",
        "eula.txt",
        "server.properties",
        "ops.json",
        "whitelist.json",
    ] {
        assert!(project.join(file).exists(), "{file} missing");
    }
    assert!(project.join(".cubecli/rcon.secret").exists());

    let meta = std::fs::read_to_string(project.join(".cubecli.meta")).unwrap();
    assert!(meta.contains("core = \"paper\""));
    assert!(meta.contains("eula_accepted = false"));

    // rcon secret is 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(project.join(".cubecli/rcon.secret"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // the project is registered for the daemon
    let registry = std::fs::read_to_string(env.home.join(".cubecli/registry.toml")).unwrap();
    assert!(registry.contains("server"));

    // create refuses to run twice
    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .arg("create")
        .arg("--yes")
        .assert()
        .failure();
}

#[test]
fn create_rejects_non_empty_and_missing_dirs() {
    let (mut cmd, env) = cli();
    let dir = env.cwd.path().join("full");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("something.txt"), "x").unwrap();
    let output = cmd
        .current_dir(&dir)
        .arg("create")
        .arg("--yes")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not empty"), "{stderr}");
    assert!(
        stderr.contains("something.txt"),
        "lists offending files: {stderr}"
    );

    let (mut cmd, env) = cli();
    cmd.arg("create")
        .arg(env.cwd.path().join("does-not-exist"))
        .assert()
        .failure();
}

#[test]
fn create_unknown_core_errors_cleanly() {
    let (mut cmd, env) = cli();
    let project = env.cwd.path().join("s");
    std::fs::create_dir_all(&project).unwrap();
    let output = cmd
        .current_dir(&project)
        .env("CUBECLI_NO_DOWNLOAD", "1")
        .args(["create", "--yes", "--core", "bukkit"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown core"));
}

#[test]
fn project_commands_refuse_outside_a_project() {
    let (mut cmd, env) = cli();
    cmd.current_dir(env.cwd.path())
        .args(["project", "status"])
        .assert()
        .failure()
        .stderr(contains("not a cubecli project"));
}

// ------------------------------------------------------------------------------------------------
// config

#[test]
fn config_list_get_set_roundtrip_rewrites_properties() {
    let (mut cmd, env) = cli();
    let project = create_project(&env, "s");

    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "config"])
        .assert()
        .success()
        .stdout(contains("addon-source"));

    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "config", "port"])
        .assert()
        .success()
        .stdout(contains("25565"));

    // set port → properties rewritten + restart warning printed
    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "config", "port=25600"])
        .assert()
        .success()
        .stdout(contains("server.properties updated"));
    let props = std::fs::read_to_string(project.join("server.properties")).unwrap();
    assert!(props.contains("server-port=25600"));
    assert!(!props.contains("server-port=25565"));

    // invalid values fail with hints
    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "config", "pvp=maybe"])
        .assert()
        .failure()
        .stderr(contains("not a boolean"));

    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "config", "restart-policy=weekly"])
        .assert()
        .failure();

    // unknown key lists valid keys via the hint
    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "config", "nope=1"])
        .assert()
        .failure()
        .stderr(contains("project config` to list"));

    // eula acceptance updates eula.txt too
    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "config", "eula=true"])
        .assert()
        .success();
    assert!(
        std::fs::read_to_string(project.join("eula.txt"))
            .unwrap()
            .contains("eula=true")
    );
}

// ------------------------------------------------------------------------------------------------
// saves

#[test]
fn saves_backup_list_restore_roundtrip() {
    let (mut cmd, env) = cli();
    let project = create_project(&env, "s");
    write_world(&project);

    // backup (server not running — no RCON path)
    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "saves", "backup"])
        .assert()
        .success();

    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "saves", "list"])
        .assert()
        .success()
        .stdout(contains(".zip"));

    // mutate the world, then restore by id fragment
    std::fs::write(project.join("world/level.dat"), b"corrupted").unwrap();
    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "saves", "restore", "--yes", "2026"])
        .assert()
        .success();
    assert_eq!(
        std::fs::read(project.join("world/level.dat")).unwrap(),
        b"level"
    );

    // addons are never touched by a restore
    assert!(project.join("server.properties").exists());

    // unknown id fails
    let (mut cmd, _env) = cli();
    cmd.current_dir(&project)
        .args(["project", "saves", "restore", "--yes", "1999"])
        .assert()
        .failure()
        .stderr(contains("no backup matches"));
}

// ------------------------------------------------------------------------------------------------
// daemon + setup

#[test]
fn daemon_status_without_install() {
    let (mut cmd, _env) = cli();
    cmd.args(["daemon", "status"])
        .assert()
        .success()
        .stdout(contains("not installed"));
}

#[test]
fn setup_java_lists_local_table() {
    let (mut cmd, _env) = cli();
    // No JDKs in the isolated HOME; the Adoptium listing may fail offline —
    // the command still exits 0 with the local table.
    cmd.args(["setup", "java"]).assert().success();
}

#[test]
fn setup_curseforge_stores_key_in_redirected_home() {
    let (mut cmd, env) = cli();
    cmd.args(["setup", "curseforge", "test-key-123"])
        .assert()
        .success()
        .stdout(contains("stored"));
    let config = std::fs::read_to_string(env.home.join(".cubecli/config.toml")).unwrap();
    assert!(config.contains("test-key-123"), "{config}");

    // an empty key is rejected
    let (mut cmd, _env) = cli();
    cmd.args(["setup", "curseforge", "   "]).assert().failure();
}

// ------------------------------------------------------------------------------------------------
// doctor

#[test]
fn doctor_reports_checks_offline() {
    let (mut cmd, _env) = cli();
    // Values vary by machine (docker may exist, JDKs may be installed), so
    // assert the structure: section names and the summary line.
    cmd.args(["doctor"])
        .env("CUBECLI_DOCKER_BIN", "/nonexistent/cubecli-fake-docker")
        .assert()
        .failure() // docker check fails offline — doctor exits 1 on Fail
        .stdout(contains("cubecli doctor"))
        .stdout(contains("docker"))
        .stdout(contains("java"))
        .stdout(contains("daemon"))
        .stdout(contains("no project in this directory"));
}

#[test]
fn doctor_inside_a_project_includes_project_checks() {
    let (mut cmd, env) = cli();
    let project = create_project(&env, "doc-server");
    cmd.current_dir(&project)
        .env("CUBECLI_DOCKER_BIN", "/nonexistent/cubecli-fake-docker")
        .args(["doctor"])
        .assert()
        .stdout(contains("project/eula"))
        .stdout(contains("NOT accepted"));
}
