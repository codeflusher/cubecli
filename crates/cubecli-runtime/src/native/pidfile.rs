//! The server pidfile — a *hint*, never the authority (RCON probe wins).

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Result, RuntimeError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PidFile {
    pub pid: u32,
    pub started_at: DateTime<Utc>,
    pub argv: Vec<String>,
    #[serde(default)]
    pub java_feature: u32,
}

pub fn write(path: &Path, file: &PidFile) -> Result<()> {
    let text = serde_json::to_string_pretty(file)
        .map_err(|e| RuntimeError::Other(format!("cannot serialize pidfile: {e}")))?;
    std::fs::write(path, text).map_err(|e| RuntimeError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

pub fn read(path: &Path) -> Result<Option<PidFile>> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| RuntimeError::Other(format!("corrupt pidfile {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("server.pid");
        let file = PidFile {
            pid: 1234,
            started_at: Utc::now(),
            argv: vec!["java".into(), "-jar".into(), "server.jar".into()],
            java_feature: 21,
        };
        write(&path, &file).unwrap();
        assert_eq!(read(&path).unwrap().unwrap().pid, 1234);
        remove(&path);
        assert!(read(&path).unwrap().is_none());
    }
}
