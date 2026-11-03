//! Console output helpers: progress reporting and small tables.

use std::sync::Mutex;

use cubecli_core::traits::progress::ProgressReporter;

/// Terse terminal progress: `↓ file (size)` → `✓ file`. Chunk progress is
/// silent (downloads are short-lived; bars land with the TUI later).
pub struct ConsoleProgress {
    quiet: bool,
    seen: Mutex<Vec<String>>,
}

impl ConsoleProgress {
    pub fn new() -> Self {
        Self {
            quiet: false,
            seen: Mutex::new(Vec::new()),
        }
    }

    pub fn quiet() -> Self {
        Self {
            quiet: true,
            seen: Mutex::new(Vec::new()),
        }
    }

    fn mark_started(&self, what: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        if seen.iter().any(|s| s == what) {
            false
        } else {
            seen.push(what.to_string());
            true
        }
    }
}

impl Default for ConsoleProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressReporter for ConsoleProgress {
    fn started(&self, what: &str, total_bytes: Option<u64>) {
        if self.quiet {
            return;
        }
        if self.mark_started(what) {
            match total_bytes {
                Some(bytes) => println!("  ↓ {} ({})", what, human_size(bytes)),
                None => println!("  ↓ {what}"),
            }
        }
    }

    fn finished(&self, what: &str) {
        if !self.quiet {
            println!("  ✓ {what}");
        }
    }

    fn message(&self, msg: &str) {
        if !self.quiet {
            println!("  • {msg}");
        }
    }
}

pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GiB", b / GB)
    } else if b >= MB {
        format!("{:.1} MiB", b / MB)
    } else if b >= KB {
        format!("{:.1} KiB", b / KB)
    } else {
        format!("{bytes} B")
    }
}
