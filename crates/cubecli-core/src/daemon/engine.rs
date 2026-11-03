//! The pure decision function at the heart of the daemon.

use chrono::{DateTime, Utc};

use crate::model::RestartPolicy;
use crate::schedule::ScheduleRule;

/// Everything `decide` needs — the executor assembles this per project.
#[derive(Debug, Clone)]
pub struct ProjectSnapshot {
    pub running: bool,
    pub manually_stopped: bool,
    /// Unclean death evidence (pidfile present, process gone) — lets a
    /// freshly started daemon adopt a server that crashed while it was down.
    pub crash_evidence: bool,
    pub restart_policy: RestartPolicy,
    /// Raw cron expression; empty unless policy is cron.
    pub restart_cron: String,
    pub backup_auto: bool,
    /// Cron expression or humanized interval.
    pub backup_rule: String,
    pub autocheck_due: bool,
    pub state: super::state::DaemonState,
    pub now: DateTime<Utc>,
    /// Tick interval — bounds how far back a missed cron boundary is sought.
    pub tick: chrono::Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Server should be up but is gone → restart it (after backoff).
    RestartCrashed { backoff_secs: u64 },
    /// restart-policy = cron boundary reached.
    CronRestart,
    /// Auto backup due.
    Backup,
    /// Addon update check due.
    CheckUpdates,
}

/// Crash backoff ladder: 30s → 60 → 120 → 300 (cap).
pub fn next_backoff(current: u64) -> u64 {
    match current {
        0 => 30,
        n if n < 300 => (n * 2).min(300),
        n => n.min(300),
    }
}

/// Backoff resets after the server stayed up this long.
const STABLE_AFTER: chrono::Duration = chrono::Duration::minutes(5);

pub fn decide(snap: &ProjectSnapshot) -> Vec<Action> {
    let mut actions = Vec::new();
    let state = &snap.state;

    // ---- crash restart (unless-stopped / cron policies; never manual-only)
    let expected = state.expected_running || (snap.crash_evidence && !snap.manually_stopped);
    let crashed = expected && !snap.running && !snap.manually_stopped;
    if crashed && snap.restart_policy != RestartPolicy::ManualOnly {
        // Backoff gate: not before last_exit + backoff.
        let ready = state
            .last_exit_at
            .map(|exit| {
                snap.now >= exit + chrono::Duration::seconds(state.crash_backoff_secs as i64)
            })
            .unwrap_or(true);
        if ready {
            actions.push(Action::RestartCrashed {
                backoff_secs: state.crash_backoff_secs,
            });
        }
    }

    // ---- cron restart policy (once per boundary)
    if snap.restart_policy == RestartPolicy::Cron && !snap.manually_stopped {
        if let Some(fire) = cron_due(
            &snap.restart_cron,
            state.last_cron_fire,
            snap.now,
            snap.tick,
        ) {
            let _ = fire;
            actions.push(Action::CronRestart);
        }
    }

    // ---- auto backup
    if snap.backup_auto {
        if let Some(rule) = ScheduleRule::parse(&snap.backup_rule).ok() {
            let base = state.last_backup_at.unwrap_or(snap.now);
            // A server that never backed up yet is due immediately.
            let due = state.last_backup_at.is_none_or(|_| {
                rule.next_after(base)
                    .ok()
                    .flatten()
                    .is_some_and(|next| next <= snap.now)
            });
            if due {
                actions.push(Action::Backup);
            }
        }
    }

    // ---- addon update checks
    if snap.autocheck_due {
        actions.push(Action::CheckUpdates);
    }

    actions
}

/// The next cron boundary at-or-before `now` that hasn't fired yet.
/// `last_cron_fire` makes firing once-only even across ticks/restarts.
pub fn cron_due(
    expr: &str,
    last_cron_fire: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    tick: chrono::Duration,
) -> Option<DateTime<Utc>> {
    let rule = ScheduleRule::parse(expr).ok()?;
    let base = match last_cron_fire {
        Some(fired) => fired,
        None => now - tick, // only fire for boundaries within this tick
    };
    let next = rule.next_after(base).ok().flatten()?;
    (next <= now).then_some(next)
}

/// Executor-side bookkeeping: call with the new status every tick.
pub fn update_state_after_tick(state: &mut super::state::DaemonState, snap: &ProjectSnapshot) {
    if snap.running {
        // stability resets the backoff ladder
        match state.stable_since {
            Some(since) if snap.now - since >= STABLE_AFTER => state.crash_backoff_secs = 0,
            Some(_) => {}
            None => {
                state.stable_since = Some(snap.now);
                state.crash_backoff_secs = 0;
            }
        }
        state.expected_running = true;
        state.last_exit_at = None;
    } else {
        // Adopt crash evidence as expectation (daemon was down when it died).
        if snap.crash_evidence && !snap.manually_stopped {
            state.expected_running = true;
            if state.last_exit_at.is_none() {
                state.last_exit_at = None; // unknown death time → restart immediately
            }
        } else if state.expected_running && state.last_exit_at.is_none() {
            // just died under our watch
            state.last_exit_at = Some(snap.now);
        }
        if snap.manually_stopped {
            state.expected_running = false;
            state.last_exit_at = None;
        }
    }
}

/// After executing a crash restart, grow the ladder.
pub fn state_after_restart(state: &mut super::state::DaemonState) {
    state.crash_backoff_secs = next_backoff(state.crash_backoff_secs);
    state.expected_running = true;
    state.last_exit_at = None;
    state.stable_since = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::state::DaemonState;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn snap(now: DateTime<Utc>, state: DaemonState) -> ProjectSnapshot {
        ProjectSnapshot {
            running: false,
            manually_stopped: false,
            crash_evidence: false,
            restart_policy: RestartPolicy::UnlessStopped,
            restart_cron: String::new(),
            backup_auto: false,
            backup_rule: "1d".into(),
            autocheck_due: false,
            state,
            now,
            tick: chrono::Duration::seconds(30),
        }
    }

    #[test]
    fn adopts_crash_evidence_from_fresh_daemon() {
        // Daemon was down when the server crashed: crash evidence alone
        // (no expected_running in state) must trigger the restart.
        let now = at(2026, 10, 31, 12, 0);
        let mut s = snap(now, DaemonState::default());
        s.crash_evidence = true;
        let actions = decide(&s);
        assert_eq!(actions, vec![Action::RestartCrashed { backoff_secs: 0 }]);

        // …but a manual-stop marker still wins.
        s.manually_stopped = true;
        assert!(decide(&s).is_empty());
    }

    #[test]
    fn no_action_when_manual_stop_marker_present() {
        let mut state = DaemonState::default();
        state.expected_running = true;
        let mut s = snap(at(2026, 10, 31, 12, 0), state);
        s.manually_stopped = true;
        assert!(decide(&s).is_empty());
    }

    #[test]
    fn no_restart_for_manual_only_policy() {
        let mut state = DaemonState::default();
        state.expected_running = true;
        let mut s = snap(at(2026, 10, 31, 12, 0), state);
        s.restart_policy = RestartPolicy::ManualOnly;
        assert!(decide(&s).is_empty());
    }

    #[test]
    fn crash_restarts_when_expected_and_not_stopped() {
        let mut state = DaemonState::default();
        state.expected_running = true;
        let s = snap(at(2026, 10, 31, 12, 0), state);
        assert_eq!(decide(&s), vec![Action::RestartCrashed { backoff_secs: 0 }]);
    }

    #[test]
    fn backoff_gate_holds_until_ready_then_restarts() {
        let now = at(2026, 10, 31, 12, 0);
        let mut state = DaemonState::default();
        state.expected_running = true;
        state.crash_backoff_secs = 120;
        state.last_exit_at = Some(now - chrono::Duration::seconds(60)); // too soon
        let s = snap(now, state.clone());
        assert!(decide(&s).is_empty(), "must wait out the backoff");

        let s = snap(now + chrono::Duration::seconds(61), state);
        assert_eq!(
            decide(&s),
            vec![Action::RestartCrashed { backoff_secs: 120 }]
        );
    }

    #[test]
    fn backoff_ladder_grows_and_caps() {
        assert_eq!(next_backoff(0), 30);
        assert_eq!(next_backoff(30), 60);
        assert_eq!(next_backoff(60), 120);
        assert_eq!(next_backoff(120), 240);
        assert_eq!(next_backoff(240), 300);
        assert_eq!(next_backoff(300), 300);
    }

    #[test]
    fn cron_fires_once_per_boundary() {
        let now = at(2026, 10, 31, 12, 0);
        let mut s = snap(now, DaemonState::default());
        s.restart_policy = RestartPolicy::Cron;
        s.restart_cron = "0 4 * * *".into(); // daily 4am

        // 12:00 — today's 4am already passed within unknown history: no fire
        assert!(
            decide(&s).is_empty(),
            "first tick must not fire old boundaries"
        );

        // simulate fired at 4am today: next boundary = tomorrow 4am → not due
        s.state.last_cron_fire = Some(at(2026, 10, 31, 4, 0));
        assert!(decide(&s).is_empty());

        // jump to 4am tomorrow + a tick: boundary fires
        let tomorrow = at(2026, 11, 1, 4, 0) + chrono::Duration::seconds(10);
        let mut s2 = snap(tomorrow, s.state.clone());
        s2.restart_policy = RestartPolicy::Cron;
        s2.restart_cron = "0 4 * * *".into();
        let actions = decide(&s2);
        assert!(actions.contains(&Action::CronRestart));

        // cron restarts also apply to a crashed server
        s2.state.expected_running = true;
        let actions = decide(&s2);
        assert!(actions.contains(&Action::CronRestart));
        assert!(actions.contains(&Action::RestartCrashed { backoff_secs: 0 }));
    }

    #[test]
    fn backup_due_immediately_then_after_interval() {
        let now = at(2026, 10, 31, 12, 0);
        let mut s = snap(now, DaemonState::default());
        s.backup_auto = true;
        s.backup_rule = "6h".into();

        assert!(
            decide(&s).contains(&Action::Backup),
            "never backed up → due"
        );

        s.state.last_backup_at = Some(now);
        assert!(!decide(&s).contains(&Action::Backup));

        s.state.last_backup_at = Some(now - chrono::Duration::hours(7));
        assert!(decide(&s).contains(&Action::Backup), "6h elapsed → due");
    }

    #[test]
    fn cron_backup_rule_supported() {
        let now = at(2026, 10, 31, 12, 0);
        let mut s = snap(now, DaemonState::default());
        s.backup_auto = true;
        s.backup_rule = "0 4 * * *".into();
        assert!(
            decide(&s).contains(&Action::Backup),
            "cron rule due when never backed up"
        );
        s.state.last_backup_at = Some(now);
        assert!(!decide(&s).contains(&Action::Backup));
    }

    #[test]
    fn autocheck_only_when_due() {
        let now = at(2026, 10, 31, 12, 0);
        let mut s = snap(now, DaemonState::default());
        assert!(!decide(&s).contains(&Action::CheckUpdates));
        s.autocheck_due = true;
        assert!(decide(&s).contains(&Action::CheckUpdates));
    }

    #[test]
    fn tick_state_transitions() {
        let now = at(2026, 10, 31, 12, 0);
        let mut state = DaemonState::default();

        // running: expected, no exit recorded
        let mut s = snap(now, state.clone());
        s.running = true;
        update_state_after_tick(&mut state, &s);
        assert!(state.expected_running);
        assert!(state.last_exit_at.is_none());

        // dies: exit recorded
        let mut s = snap(now, state.clone());
        s.running = false;
        update_state_after_tick(&mut state, &s);
        assert!(state.last_exit_at.is_some());

        // manual stop: expected off
        let mut s = snap(now, state.clone());
        s.manually_stopped = true;
        update_state_after_tick(&mut state, &s);
        assert!(!state.expected_running);

        // comes back and stays stable: backoff cleared
        let mut state = DaemonState::default();
        state.crash_backoff_secs = 300;
        let mut s = snap(now, state.clone());
        s.running = true;
        update_state_after_tick(&mut state, &s);
        assert_eq!(state.crash_backoff_secs, 0);
    }
}
