//! Integration specs driving the real `cubecli` binary via assert_cmd.
//! Each test gets an isolated HOME (so `~/.cubecli` lands in a tempdir) and
//! an isolated project dir. Network-dependent provisioning is disabled via
//! `CUBECLI_NO_DOWNLOAD=1` so the suite runs fully offline; live-API specs
//! live in `cli_live.rs` behind `CUBECLI_LIVE=1`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
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

// ------------------------------------------------------------------------------------------------
// modlist export/import

/// Seed installed addon records into a project's `.cubecli.meta` by
/// replacing the empty `installed = []` placeholder with `[[addons.installed]]`
/// tables (re-defining the key would make the TOML invalid).
fn install_addons_in_meta(project: &Path, toml_records: &str) {
    let meta_path = project.join(".cubecli.meta");
    let meta = std::fs::read_to_string(&meta_path).unwrap();
    let updated = meta.replacen("installed = []", toml_records, 1);
    assert_ne!(
        updated, meta,
        "meta has no `installed = []` placeholder to replace"
    );
    std::fs::write(meta_path, updated).unwrap();
}

const TWO_MODS_ONE_PLUGIN: &str = r#"
[[addons.installed]]
slug = "sodium"
source_id = "modrinth"
provider_id = "AANobbMI"
kind = "mod"
version = "0.6.5"
file = "sodium-0.6.5.jar"
dir = "mods"
installed_at = "2026-01-01T00:00:00Z"

[[addons.installed]]
slug = "fabric-api"
source_id = "modrinth"
provider_id = "P7dR8mSH"
kind = "mod"
version = "0.100.0"
file = "fabric-api.jar"
dir = "mods"
installed_at = "2026-01-01T00:00:00Z"

[[addons.installed]]
slug = "essentialsx"
source_id = "modrinth"
provider_id = "gid"
kind = "plugin"
version = "2.20"
file = "essentialsx.jar"
dir = "plugins"
installed_at = "2026-01-01T00:00:00Z"
"#;

#[test]
fn tools_help_lists_export_modlist() {
    let (mut cmd, _env) = cli();
    cmd.args(["tools", "export-modlist", "--help"])
        .assert()
        .success()
        .stdout(contains("mod jars"))
        .stdout(contains("--server-only"))
        .stdout(contains("--mod-source"));
}

#[test]
fn tools_export_modlist_fails_in_a_dir_without_jars() {
    let (mut cmd, _env) = cli();
    cmd.args(["tools", "export-modlist"])
        .assert()
        .failure()
        .stderr(contains("no .jar files"));
}

#[test]
fn project_export_modlist_reads_meta_offline() {
    let (mut cmd, env) = cli();
    let project = create_project(&env, "server");
    install_addons_in_meta(&project, TWO_MODS_ONE_PLUGIN);

    cmd.current_dir(&project)
        .args(["project", "export-modlist"])
        .assert()
        .success()
        .stdout(contains("# cubecli modlist v1"))
        .stdout(contains("# source: modrinth"))
        .stdout(contains("mod:fabric-api@0.100.0"))
        .stdout(contains("mod:sodium@0.6.5"))
        .stdout(contains("# not found:").not())
        // entries sorted, plugins never leak into a modlist
        .stdout(contains("essentialsx").not());
}

#[test]
fn project_export_modlist_base64_writes_decodable_file() {
    let (mut cmd, env) = cli();
    let project = create_project(&env, "server");
    install_addons_in_meta(&project, TWO_MODS_ONE_PLUGIN);
    let out = project.join("mods.b64");

    cmd.current_dir(&project)
        .args(["project", "export-modlist", "--base64", "-o"])
        .arg(&out)
        .assert()
        .success()
        .stdout(contains("wrote"));

    let encoded = std::fs::read_to_string(&out).unwrap();
    assert!(!encoded.contains('\n'), "base64 output is a single line");
    let decoded =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded.trim()).unwrap();
    let text = String::from_utf8(decoded).unwrap();
    assert!(text.contains("mod:sodium@0.6.5"));
    assert!(text.contains("# source: modrinth"));
}

#[test]
fn project_export_modlist_outside_project_fails() {
    let (mut cmd, env) = cli();
    cmd.current_dir(env.cwd.path())
        .args(["project", "export-modlist"])
        .assert()
        .failure()
        .stderr(contains("not a cubecli project"));
}

#[test]
fn project_import_modlist_rejects_bad_input() {
    // One isolated HOME for all cases; a fresh command per invocation
    // (assert_cmd accumulates args across `args()` calls).
    let (_, env) = cli();
    let project = create_project(&env, "server");
    let mut run = |args: &[&str]| {
        let mut command = Command::cargo_bin("cubecli").unwrap();
        command
            .env("HOME", &env.home)
            .env("CUBECLI_NO_DOWNLOAD", "1")
            .current_dir(&project)
            .args(args)
            .assert()
    };

    // no input at all
    run(&["project", "import-modlist"])
        .failure()
        .stderr(contains("no modlist input"));

    // malformed entry line (names the offending line)
    run(&["project", "import-modlist", "mod:ok\ngarbage-line"])
        .failure()
        .stderr(contains("not a modlist entry"));

    // invalid base64
    run(&["project", "import-modlist", "--base64", "!!!not-base64!!!"])
        .failure()
        .stderr(contains("invalid base64"));

    // -i pointing at a missing file
    run(&["project", "import-modlist", "-i", "does-not-exist.modlist"])
        .failure()
        .stderr(contains("does-not-exist.modlist"));

    // a modlist with no entries
    run(&["project", "import-modlist", "# only comments\n"])
        .failure()
        .stderr(contains("contains no mods"));
}

#[test]
fn project_import_modlist_incompatible_core_reports_not_found_offline() {
    // `create --yes` defaults to paper, which loads no mods — every entry
    // fails the compat check before any network call, so this is fully
    // offline-deterministic.
    let (mut cmd, env) = cli();
    let project = create_project(&env, "server");

    cmd.current_dir(&project)
        .args(["project", "import-modlist", "mod:sodium@1.2.3"])
        .assert()
        .failure()
        .stdout(contains("importing 1 mod(s)"))
        .stdout(contains("imported 0 of 1 mods"))
        .stdout(contains("not found (1)"))
        .stdout(contains("warning: 1 mod(s) could not be imported"));
}
