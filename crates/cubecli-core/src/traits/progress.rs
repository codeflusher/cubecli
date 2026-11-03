//! Sink for long-running operations (downloads, installs). Console and TUI
//! each provide their own reporter; libraries only know this trait.

/// Progress reporter for downloads and multi-step provisioning.
pub trait ProgressReporter: Send + Sync {
    /// A new unit of work started (e.g. a download), with total bytes if known.
    fn started(&self, _what: &str, _total_bytes: Option<u64>) {}
    /// Bytes (or units) completed since the last call.
    fn progressed(&self, _delta: u64) {}
    fn finished(&self, _what: &str) {}
    /// Free-form status line.
    fn message(&self, _msg: &str) {}
}

/// Discards everything.
pub struct NoopProgress;

impl ProgressReporter for NoopProgress {}

/// Logs progress through `tracing` (used by the daemon where no TTY exists).
pub struct TracingProgress;

impl ProgressReporter for TracingProgress {
    fn started(&self, what: &str, total_bytes: Option<u64>) {
        tracing::info!(what, total_bytes, "started");
    }
    fn progressed(&self, delta: u64) {
        tracing::debug!(delta, "progress");
    }
    fn finished(&self, what: &str) {
        tracing::info!(what, "finished");
    }
    fn message(&self, msg: &str) {
        tracing::info!("{msg}");
    }
}
