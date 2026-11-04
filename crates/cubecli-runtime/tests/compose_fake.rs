//! Compose backend specs against a fake `docker` binary: a shell script that
//! journals every invocation and prints canned `ps` JSON. This pins the
//! behavioral contracts that pure renderer specs cannot see — the stop
//! marker lifecycle, restart = stop + up, and status crash mapping.
//! Unix-only: the fake is a shell script.

#![cfg(unix)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use cubecli_core::paths::ProjectLayout;
use cubecli_core::project::ProjectHandle;
use cubecli_core::traits::runtime::{RuntimeBackend, RuntimeStatus, StopGrace};
use cubecli_runtime::ComposeBackend;

/// Env vars are process-global — serialize every test that mutates them.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Installs the fake docker into the process env for the duration of the guard.
struct FakeDocker {
    _tmp: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
    journal: PathBuf,
    ps_file: PathBuf,
    originals: Vec<(&'static str, Option<OsString>)>,
}

impl Drop for FakeDocker {
    fn drop(&mut self) {
        for (key, value) in &self.originals {
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

const FAKE_SCRIPT: &str = r#"#!/bin/sh
# fake docker: journal the invocation, serve canned responses
echo "$*" >> "$FAKE_DOCKER_JOURNAL"
case " $* " in
  *" info "*)
    echo "24.0.7" ;;
  *" ps "*)
    cat "$FAKE_DOCKER_PS" ;;
  *" stop "*)
    # When enabled: the backend must lay the manually-stopped marker down
    # BEFORE the stop lands — the daemon ticks while docker stops the
    # container. Explicit `project stop` enables this; daemon-style stops
    # (restart) must not write the marker at all.
    if [ "$FAKE_DOCKER_REQUIRE_MARKER" = "1" ]; then
      [ -f "$FAKE_DOCKER_MARKER" ] || { echo "marker missing at stop time" >&2; exit 9; }
    fi ;;
esac
exit 0
"#;

/// `require_marker: Some(project_marker)` makes the fake's stop handler
/// fail unless the manually-stopped marker already exists — the ordering
/// contract for explicit stops. `None` leaves it unchecked (restart stops).
fn fake_docker(ps_json: &str, require_marker: Option<&std::path::Path>) -> FakeDocker {
    let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("docker");
    std::fs::write(&bin, FAKE_SCRIPT).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let journal = tmp.path().join("journal.log");
    std::fs::write(&journal, "").unwrap();
    let ps_file = tmp.path().join("ps.json");
    std::fs::write(&ps_file, ps_json).unwrap();

    let keys = ["CUBECLI_DOCKER_BIN", "FAKE_DOCKER_JOURNAL", "FAKE_DOCKER_PS", "FAKE_DOCKER_MARKER", "FAKE_DOCKER_REQUIRE_MARKER"];
    let originals: Vec<(&'static str, Option<OsString>)> =
        keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
    unsafe {
        std::env::set_var("CUBECLI_DOCKER_BIN", &bin);
        std::env::set_var("FAKE_DOCKER_JOURNAL", &journal);
        std::env::set_var("FAKE_DOCKER_PS", &ps_file);
        match require_marker {
            Some(marker) => {
                std::env::set_var("FAKE_DOCKER_MARKER", marker);
                std::env::set_var("FAKE_DOCKER_REQUIRE_MARKER", "1");
            }
            None => {
                std::env::remove_var("FAKE_DOCKER_MARKER");
                std::env::set_var("FAKE_DOCKER_REQUIRE_MARKER", "0");
            }
        }
    }
    FakeDocker {
        _tmp: tmp,
        _guard: guard,
        journal,
        ps_file,
        originals,
    }
}

impl FakeDocker {
    fn set_ps(&self, ps_json: &str) {
        std::fs::write(&self.ps_file, ps_json).unwrap();
    }

    fn journal(&self) -> String {
        std::fs::read_to_string(&self.journal).unwrap()
    }
}

fn compose_project() -> (tempfile::TempDir, ProjectLayout, ProjectHandle) {
    let tmp = tempfile::tempdir().unwrap();
    let layout = ProjectLayout::new(tmp.path());
    layout.ensure_state_dirs().unwrap();
    let mut meta = cubecli_core::ProjectMeta::default();
    meta.project.slug = "spec-server".into();
    meta.server.eula_accepted = true;
    meta.server.java_version = 21;
    meta.server.launch = Some(cubecli_core::model::LaunchSpec::Script {
        script: "run.sh".into(),
        args: vec![],
    });
    meta.runtime.backend = cubecli_core::model::Backend::Compose;
    meta.save(layout.meta_file()).unwrap();
    let handle = ProjectHandle::open(layout.clone()).unwrap();
    (tmp, layout, handle)
}

const RUNNING_PS: &str =
    r#"[{"Name":"cubecli-spec-server-1","State":"running","ExitCode":0,"StartedAt":"2026-01-02T03:04:05.000000000Z"}]"#;

#[tokio::test]
async fn start_clears_manual_stop_marker_and_runs_up() {
    let (_tmp, layout, handle) = compose_project();
    let fake = fake_docker(RUNNING_PS, Some(&layout.manually_stopped_marker()));
    std::fs::write(layout.manually_stopped_marker(), "old stop").unwrap();

    let status = ComposeBackend.start(&handle).await.unwrap();
    assert!(matches!(status, RuntimeStatus::Running { .. }), "{status:?}");
    assert!(
        !layout.manually_stopped_marker().exists(),
        "start is intentional — it must clear the marker"
    );
    assert!(fake.journal().contains("up -d"));
}

#[tokio::test]
async fn stop_writes_marker_before_docker_stop_lands() {
    let (_tmp, layout, handle) = compose_project();
    let fake = fake_docker(RUNNING_PS, Some(&layout.manually_stopped_marker()));

    // The fake's `stop` handler exits 9 unless the marker already exists —
    // a missing marker surfaces as a backend error and fails this test.
    let outcome = ComposeBackend
        .stop(&handle, StopGrace::default())
        .await
        .unwrap();
    assert!(layout.manually_stopped_marker().exists());
    assert!(fake.journal().contains("stop"));
    assert!(matches!(
        outcome.stage,
        cubecli_core::traits::runtime::StopStage::Signal
    ));
}

#[tokio::test]
async fn stop_on_stopped_container_still_marks_manual() {
    let (_tmp, layout, handle) = compose_project();
    let _fake = fake_docker("[]", None);

    let outcome = ComposeBackend
        .stop(&handle, StopGrace::default())
        .await
        .unwrap();
    assert!(matches!(
        outcome.stage,
        cubecli_core::traits::runtime::StopStage::AlreadyStopped
    ));
    assert!(
        layout.manually_stopped_marker().exists(),
        "an explicit stop keeps unless-stopped policies from resurrecting the server"
    );
}

#[tokio::test]
async fn restart_is_stop_then_up_not_compose_restart() {
    let (_tmp, _layout, handle) = compose_project();
    let fake = fake_docker(RUNNING_PS, None);

    ComposeBackend.restart(&handle).await.unwrap();
    let journal = fake.journal();
    let stop_at = journal
        .find(" stop ")
        .expect("restart must stop a running container");
    let up_at = journal
        .find(" up -d")
        .expect("restart must bring the container back up");
    assert!(stop_at < up_at, "stop must precede up: {journal}");
    assert!(
        !journal.contains("-p spec-server restart"),
        "plain `docker compose restart` would skip recreating a drifted container"
    );
}

#[tokio::test]
async fn restart_of_stopped_container_goes_straight_to_up() {
    let (_tmp, _layout, handle) = compose_project();
    let fake = fake_docker("[]", None);

    ComposeBackend.restart(&handle).await.unwrap();
    let journal = fake.journal();
    assert!(!journal.contains(" stop "), "nothing running to stop: {journal}");
    assert!(journal.contains(" up -d"));
}

#[tokio::test]
async fn status_maps_exited_nonzero_to_crash() {
    let (_tmp, _layout, handle) = compose_project();
    let fake = fake_docker(RUNNING_PS, None);

    let status = ComposeBackend.status(&handle).await.unwrap();
    assert!(matches!(status, RuntimeStatus::Running { .. }));

    fake.set_ps(r#"[{"State":"exited","ExitCode":137}]"#);
    let status = ComposeBackend.status(&handle).await.unwrap();
    assert_eq!(
        status,
        RuntimeStatus::Stopped { clean: false },
        "exited-nonzero is crash evidence for the daemon"
    );
}
