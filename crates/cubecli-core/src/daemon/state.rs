//! Per-project daemon state (`~/.cubecli/daemon/state/<slug>.json`) — what
//! makes policy decisions idempotent across ticks and daemon restarts.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonState {
    /// We believe the server should be up (set on start/restart, cleared on
    /// manual stop detection).
    pub expected_running: bool,
    pub last_backup_at: Option<DateTime<Utc>>,
    /// The last cron boundary already fired (once-only guarantee).
    pub last_cron_fire: Option<DateTime<Utc>>,
    /// Exponential crash-restart backoff (30→60→120→300, reset on stability).
    pub crash_backoff_secs: u64,
    pub last_exit_at: Option<DateTime<Utc>>,
    pub last_addon_check: Option<DateTime<Utc>>,
    pub stable_since: Option<DateTime<Utc>>,
}

pub fn load(path: &Path) -> Result<DaemonState> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CoreError::Meta(format!("corrupt daemon state {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DaemonState::default()),
        Err(source) => Err(CoreError::io(path, source)),
    }
}

pub fn save(path: &Path, state: &DaemonState) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(state)
        .map_err(|e| CoreError::Meta(format!("cannot serialize daemon state: {e}")))?;
    std::fs::write(path, text).map_err(|e| CoreError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.json");
        let state = DaemonState {
            expected_running: true,
            crash_backoff_secs: 120,
            last_backup_at: Some(Utc::now()),
            ..Default::default()
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path).unwrap(), state);
    }

    #[test]
    fn missing_file_is_default() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&tmp.path().join("nope.json")).unwrap(),
            DaemonState::default()
        );
    }
}
