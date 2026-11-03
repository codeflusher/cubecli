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
    assert!(yaml.contains("127.0.0.1:25599:25575"));
    assert!(yaml.contains("restart: unless-stopped"));

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
}

#[test]
fn service_renderers_are_loadable_definitions() {
    let spec = cubecli_runtime::service::ServiceSpec {
        exe: PathBuf::from("/usr/local/bin/cubecli"),
        log_out: PathBuf::from("/tmp/out.log"),
        log_err: PathBuf::from("/tmp/err.log"),
    };
    let plist = cubecli_runtime::service::launchd::render(&spec);
    assert!(plist.contains("dev.cubecli.daemon"));
    assert!(plist.contains("/usr/local/bin/cubecli"));
    assert!(plist.contains("<string>daemon</string>"));
    assert!(plist.contains("RunAtLoad"));
    assert!(plist.contains("KeepAlive"));

    let unit = cubecli_runtime::service::systemd::render(&spec);
    assert!(unit.contains("ExecStart=/usr/local/bin/cubecli daemon run"));
    assert!(unit.contains("Restart=on-failure"));
    assert!(unit.contains("WantedBy=default.target"));
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
