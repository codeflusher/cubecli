//! Runtime specs: real detached processes, the full stop sequence, and
//! the compose/service renderers. No real Minecraft server involved —
//! disposable `sh` processes stand in for the server JVM.

use std::time::Duration;

use cubecli_core::ProjectMeta;
use cubecli_core::model::{Backend, LaunchSpec, RestartPolicy};
use cubecli_core::paths::ProjectLayout;
use cubecli_core::traits::runtime::{RuntimeBackend, StopGrace};
use cubecli_runtime::NativeBackend;

fn open_with_meta(
    layout: &ProjectLayout,
    meta: &ProjectMeta,
) -> cubecli_core::project::ProjectHandle {
    meta.save(layout.meta_file()).unwrap();
    cubecli_core::project::ProjectHandle::open(layout.clone()).unwrap()
}

fn layout_meta() -> (tempfile::TempDir, ProjectLayout, ProjectMeta) {
    let tmp = tempfile::tempdir().unwrap();
    let layout = ProjectLayout::new(tmp.path());
    layout.ensure_state_dirs().unwrap();
    let mut meta = ProjectMeta::default();
    meta.project.slug = "spec-server".into();
    meta.server.eula_accepted = true;
    meta.server.java_version = 21;
    meta.server.launch = Some(LaunchSpec::Script {
        script: "run.sh".into(),
        args: vec![],
    });
    (tmp, layout, meta)
}

fn make_run_sh(layout: &ProjectLayout, body: &str) {
    let path = layout.root().join("run.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn backend() -> NativeBackend {
    NativeBackend::new(Arc::new(OwnedLocator))
}

struct OwnedLocator;
impl cubecli_core::traits::java::JavaLocator for OwnedLocator {
    fn locate(
        &self,
        req: cubecli_core::model::JavaRequirement,
    ) -> cubecli_core::Result<cubecli_core::traits::java::JavaInstall> {
        Ok(cubecli_core::traits::java::JavaInstall {
            feature: req.major,
            origin: cubecli_core::traits::java::JavaOrigin::Owned,
            home: PathBuf::from("/usr"),
            java_bin: PathBuf::from("/bin/sh"), // the "java binary" is sh; argv runs run.sh via LaunchSpec::Script
        })
    }
    fn all(&self) -> Vec<cubecli_core::traits::java::JavaInstall> {
        vec![]
    }
}

use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn start_pidfile_stop_full_cycle_with_zombie_free_reaping() {
    let (_tmp, layout, _meta) = layout_meta();
    // run.sh: a process that ignores nothing, sleeps briefly
    make_run_sh(&layout, "sleep 60");

    let handle = open_with_meta(&layout, &_meta);

    let b = backend();

    // start: detached, own session, pidfile written
    let status = b.start(&handle).await.unwrap();
    let pid = match &status {
        cubecli_core::traits::runtime::RuntimeStatus::Running { pid, .. } => pid.unwrap(),
        other => panic!("expected running, got {other:?}"),
    };
    let pidfile = cubecli_runtime::native::pidfile::read(&layout.pidfile())
        .unwrap()
        .unwrap();
    assert_eq!(pidfile.pid, pid);

    // own session ⇒ pgid == pid (kill_group works later)
    let pgid_out = std::process::Command::new("ps")
        .args(["-o", "pgid="])
        .args(["-p", &pid.to_string()])
        .output()
        .unwrap();
    let pgid: i32 = String::from_utf8_lossy(&pgid_out.stdout)
        .trim()
        .parse()
        .unwrap();
    assert_eq!(pgid, pid as i32, "setsid must make pgid == pid");

    // log file received the (silent) script's output — file exists at least
    assert!(layout.server_log().exists());

    // stop: no RCON listener → falls through to the signal path, then removes pidfile
    let outcome = b
        .stop(
            &handle,
            StopGrace {
                soft: Duration::from_secs(5),
                term: Duration::from_secs(5),
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(
            outcome.stage,
            cubecli_core::traits::runtime::StopStage::Signal
                | cubecli_core::traits::runtime::StopStage::Rcon
        ),
        "{outcome:?}"
    );
    assert!(
        cubecli_runtime::native::pidfile::read(&layout.pidfile())
            .unwrap()
            .is_none()
    );
    // explicit stop writes the marker
    assert!(layout.manually_stopped_marker().exists());

    // status after stop: no pidfile → stopped clean
    let status = b.status(&handle).await.unwrap();
    assert_eq!(
        status,
        cubecli_core::traits::runtime::RuntimeStatus::Stopped { clean: true }
    );
}

#[tokio::test]
async fn stop_writes_no_marker_for_daemon_initiated_stops() {
    let (_tmp, layout, _meta) = layout_meta();
    make_run_sh(&layout, "sleep 60");
    let handle = open_with_meta(&layout, &_meta);

    let pid = cubecli_runtime::native::spawn::spawn_detached(
        PathBuf::from("/bin/sh").as_path(),
        &["sh".into(), "run.sh".into()],
        layout.root(),
        &layout.server_log(),
    )
    .unwrap();
    cubecli_runtime::native::pidfile::write(
        &layout.pidfile(),
        &cubecli_runtime::native::pidfile::PidFile {
            pid,
            started_at: chrono::Utc::now(),
            argv: vec![],
            java_feature: 21,
        },
    )
    .unwrap();

    cubecli_runtime::stop::graceful(&layout, StopGrace::default(), false)
        .await
        .unwrap();
    assert!(
        !layout.manually_stopped_marker().exists(),
        "daemon-initiated stops must not block unless-stopped"
    );
    assert!(
        cubecli_runtime::native::pidfile::read(&layout.pidfile())
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn stop_is_already_stopped_without_pidfile() {
    let (_tmp, layout, _meta) = layout_meta();
    let outcome = cubecli_runtime::stop::graceful(&layout, StopGrace::default(), true)
        .await
        .unwrap();
    assert_eq!(
        outcome.stage,
        cubecli_core::traits::runtime::StopStage::AlreadyStopped
    );
    assert!(layout.manually_stopped_marker().exists());
}

#[tokio::test]
async fn start_refuses_without_eula() {
    let (_tmp, layout, mut meta) = layout_meta();
    meta.server.eula_accepted = false;
    meta.save(layout.meta_file()).unwrap();
    make_run_sh(&layout, "sleep 60");
    let handle = cubecli_core::project::ProjectHandle::open(layout.clone()).unwrap();

    let err = backend().start(&handle).await.unwrap_err();
    assert!(matches!(err, cubecli_core::CoreError::EulaNotAccepted));
    assert!(err.hint().unwrap().contains("eula=true"));
}

#[test]
fn compose_render_covers_launch_shapes_and_policies() {
    let mut meta = ProjectMeta::default();
    meta.project.slug = "my-server".into();
    meta.server.java_version = 21;
    meta.runtime.port = 25500;
    meta.runtime.rcon_port = 25599;
    meta.runtime.restart_policy = RestartPolicy::UnlessStopped;
    meta.server.launch = Some(LaunchSpec::Jar {
        jar: "paper.jar".into(),
        jvm_args: vec!["-Xmx4G".into()],
        args: vec!["nogui".into()],
    });

    let root = PathBuf::from("/srv/mc");
    let yaml = cubecli_runtime::compose::render(&meta, &root).unwrap();
    assert!(yaml.contains("image: eclipse-temurin:21-jre"));
    assert!(yaml.contains("user: \""));
    assert!(yaml.contains("/srv/mc:/data"));
    assert!(yaml.contains("java -Xmx4G -jar paper.jar nogui"));
    assert!(yaml.contains("25500:25500"));
    // host and container RCON ports are the same — the in-container server
    // listens on the port written into the bind-mounted server.properties
    assert!(yaml.contains("127.0.0.1:25599:25599"));
    assert!(!yaml.contains("25599:25575"), "no hardcoded container port");
    assert!(yaml.contains("restart: unless-stopped"));
    // freshness signature stamped on the first line
    let sig = cubecli_runtime::compose::on_disk_sig(&yaml).expect("render stamps a signature");
    assert_eq!(sig.len(), 16, "sig is a 16-hex-char fingerprint");

    // explicit max-heap overrides the provider-authored -Xmx
    meta.runtime.max_heap = Some("2G".into());
    let yaml = cubecli_runtime::compose::render(&meta, &root).unwrap();
    assert!(
        yaml.contains("command: \"java -Xmx2G -jar paper.jar nogui\""),
        "explicit max-heap replaces provider -Xmx: {yaml}"
    );
    meta.runtime.max_heap = None;

    // argfile launch
    meta.server.launch = Some(LaunchSpec::ArgFile {
        user_jvm_args: "user_jvm_args.txt".into(),
        argfile: "libraries/x/unix_args.txt".into(),
        args: vec!["nogui".into()],
    });
    let yaml = cubecli_runtime::compose::render(&meta, &root).unwrap();
    assert!(yaml.contains("@user_jvm_args.txt @libraries/x/unix_args.txt nogui"));

    // cron policy → compose restart stays off (the daemon drives it)
    meta.runtime.restart_policy = RestartPolicy::Cron;
    let yaml = cubecli_runtime::compose::render(&meta, &root).unwrap();
    assert!(yaml.contains("restart: no"));

    // manual-only → "no"
    meta.runtime.restart_policy = RestartPolicy::ManualOnly;
    let yaml = cubecli_runtime::compose::render(&meta, &root).unwrap();
    assert!(yaml.contains("restart: no"));

    // unprovisioned → error
    meta.server.launch = None;
    assert!(cubecli_runtime::compose::render(&meta, &root).is_err());

    // Windows-provisioned launch specs map to the unix pair for the linux
    // container (the installers emit both variants on every host)
    meta.server.launch = Some(LaunchSpec::Script {
        script: "run.bat".into(),
        args: vec!["nogui".into()],
    });
    let yaml = cubecli_runtime::compose::render(&meta, &root).unwrap();
    assert!(yaml.contains("sh run.sh nogui"), "bat→sh for the container: {yaml}");
    meta.server.launch = Some(LaunchSpec::ArgFile {
        user_jvm_args: "user_jvm_args.txt".into(),
        argfile: "libraries/x/windows_args.txt".into(),
        args: vec![],
    });
    let yaml = cubecli_runtime::compose::render(&meta, &root).unwrap();
    assert!(
        yaml.contains("@libraries/x/unix_args.txt"),
        "windows_args→unix_args for the container: {yaml}"
    );
}

#[test]
fn ps_status_maps_exit_codes_to_crash_evidence() {
    use cubecli_core::traits::runtime::RuntimeStatus;
    use cubecli_runtime::compose::status_from_ps;

    // no containers → stopped clean (never started)
    assert_eq!(
        status_from_ps("[]"),
        RuntimeStatus::Stopped { clean: true }
    );
    assert_eq!(status_from_ps(""), RuntimeStatus::Stopped { clean: true });

    // running → Running with the latest started_at, no pid (containers have none)
    let running = status_from_ps(
        r#"[{"State":"running","ExitCode":0,"StartedAt":"2026-01-02T03:04:05.000000000Z"}]"#,
    );
    assert!(matches!(running, RuntimeStatus::Running { pid: None, .. }));

    // exited 0 → stopped clean (a deliberate stop or clean shutdown)
    assert_eq!(
        status_from_ps(r#"[{"State":"exited","ExitCode":0}]"#),
        RuntimeStatus::Stopped { clean: true }
    );

    // exited non-zero → CRASH — this is the daemon's crash evidence
    assert_eq!(
        status_from_ps(r#"[{"State":"exited","ExitCode":137}]"#),
        RuntimeStatus::Stopped { clean: false }
    );

    // dead → crash regardless of exit code
    assert_eq!(
        status_from_ps(r#"[{"State":"dead","ExitCode":0}]"#),
        RuntimeStatus::Stopped { clean: false }
    );

    // paused is still up (not crash evidence); running wins over exited
    assert!(matches!(
        status_from_ps(r#"[{"State":"paused","ExitCode":0}]"#),
        RuntimeStatus::Running { .. }
    ));
    assert!(matches!(
        status_from_ps(
            r#"[{"State":"exited","ExitCode":1},{"State":"running","ExitCode":0}]"#
        ),
        RuntimeStatus::Running { .. }
    ));

    // NDJSON shape (newer compose versions emit one object per line)
    let ndjson = concat!(
        r#"{"State":"exited","ExitCode":1}"#,
        "\n",
        r#"{"State":"running","ExitCode":0}"#,
    );
    assert!(matches!(
        status_from_ps(ndjson),
        RuntimeStatus::Running { .. }
    ));
}

#[test]
fn ensure_fresh_regenerates_drift_and_keeps_manual_edits() {
    use cubecli_core::project::ProjectHandle;
    use cubecli_runtime::compose::{ensure_fresh, is_hand_edited, Freshness};

    let (tmp, layout, mut meta) = layout_meta();
    meta.runtime.backend = Backend::Compose;
    meta.save(layout.meta_file()).unwrap();
    let handle = ProjectHandle::open(layout.clone()).unwrap();

    // missing → written
    assert_eq!(ensure_fresh(&handle).unwrap(), Freshness::Written);
    let path = layout.root().join(cubecli_runtime::compose::COMPOSE_FILE);
    let original = std::fs::read_to_string(&path).unwrap();

    // unchanged metadata → current, file untouched
    assert_eq!(ensure_fresh(&handle).unwrap(), Freshness::Current);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

    // metadata drift (port change) → regenerated with a new signature
    meta.runtime.port = 25501;
    meta.save(layout.meta_file()).unwrap();
    let handle = ProjectHandle::open(layout.clone()).unwrap();
    assert_eq!(ensure_fresh(&handle).unwrap(), Freshness::Regenerated);
    assert!(std::fs::read_to_string(&path).unwrap().contains("25501:25501"));
    assert!(!is_hand_edited(&handle));

    // signature line stripped (hand edit) → kept as-is
    let text = std::fs::read_to_string(&path).unwrap();
    let body: String = text.lines().skip(1).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(&path, &body).unwrap();
    assert_eq!(ensure_fresh(&handle).unwrap(), Freshness::KeptManual);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    assert!(is_hand_edited(&handle));
    let _ = tmp;
}

#[test]
fn service_renderers_are_loadable_definitions() {
    let spec = cubecli_runtime::service::ServiceSpec {
        exe: PathBuf::from("/usr/local/bin/cubecli"),
        log_out: PathBuf::from("/tmp/out.log"),
        log_err: PathBuf::from("/tmp/err.log"),
    };
    #[cfg(target_os = "macos")]
    {
        let plist = cubecli_runtime::service::launchd::render(&spec);
        assert!(plist.contains("dev.cubecli.daemon"));
        assert!(plist.contains("/usr/local/bin/cubecli"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(plist.contains("RunAtLoad"));
        assert!(plist.contains("KeepAlive"));
    }
    #[cfg(target_os = "linux")]
    {
        let unit = cubecli_runtime::service::systemd::render(&spec);
        assert!(unit.contains("ExecStart=/usr/local/bin/cubecli daemon run"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }
    // detect() must name the platform's real mechanism (or "unsupported")
    assert_eq!(
        cubecli_runtime::service::detect(),
        if cfg!(target_os = "macos") {
            "launchd"
        } else if cfg!(target_os = "linux") {
            "systemd"
        } else {
            "unsupported"
        }
    );
}

#[tokio::test]
async fn status_reports_crash_when_pidfile_is_stale() {
    let (_tmp, layout, _meta) = layout_meta();
    ProjectMeta::default().save(layout.meta_file()).unwrap();
    let handle = cubecli_core::project::ProjectHandle::open(layout.clone()).unwrap();

    // stale pidfile pointing at a reaped process
    cubecli_runtime::native::pidfile::write(
        &layout.pidfile(),
        &cubecli_runtime::native::pidfile::PidFile {
            pid: 3_999_999_999 % 100_000, // unlikely to exist; death is what matters
            started_at: chrono::Utc::now(),
            argv: vec![],
            java_feature: 21,
        },
    )
    .unwrap();

    let status = backend().status(&handle).await.unwrap();
    assert_eq!(
        status,
        cubecli_core::traits::runtime::RuntimeStatus::Stopped { clean: false },
        "stale pidfile = unclean death"
    );
}
