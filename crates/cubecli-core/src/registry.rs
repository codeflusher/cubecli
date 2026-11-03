//! The daemon's project registry — `~/.cubecli/registry.toml`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Registry {
    pub projects: Vec<RegistryEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub slug: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

impl Registry {
    /// Load; a missing file is an empty registry.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| CoreError::Meta(format!("invalid {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| CoreError::Meta(format!("cannot serialize registry: {e}")))?;
        std::fs::write(path, text).map_err(|e| CoreError::io(path, e))
    }

    /// Add or update an entry, deduplicating by canonical path.
    pub fn upsert(&mut self, entry: RegistryEntry) {
        let canonical = dedupe_key(&entry.path);
        self.projects.retain(|e| dedupe_key(&e.path) != canonical);
        self.projects.push(entry);
    }

    pub fn remove(&mut self, path: &Path) {
        let canonical = dedupe_key(path);
        self.projects.retain(|e| dedupe_key(&e.path) != canonical);
    }
}

fn dedupe_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> RegistryEntry {
        RegistryEntry {
            slug: "server".into(),
            path: PathBuf::from(path),
            created_at: Some(Utc::now()),
        }
    }

    #[test]
    fn upsert_dedupes_by_canonical_path() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("server");
        std::fs::create_dir_all(&real).unwrap();

        let mut reg = Registry::default();
        reg.upsert(entry(real.join(".").to_str().unwrap())); // non-canonical spelling
        reg.upsert(entry(real.to_str().unwrap()));
        assert_eq!(reg.projects.len(), 1);
    }

    #[test]
    fn roundtrip_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("registry.toml");
        let mut reg = Registry::default();
        reg.upsert(entry("/srv/mc/server"));
        reg.save(&file).unwrap();

        let loaded = Registry::load(&file).unwrap();
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].path, PathBuf::from("/srv/mc/server"));

        let mut loaded = loaded;
        loaded.remove(Path::new("/srv/mc/server"));
        assert!(loaded.projects.is_empty());
    }

    #[test]
    fn missing_file_is_empty() {
        let reg = Registry::load("/nonexistent/registry.toml").unwrap();
        assert!(reg.projects.is_empty());
    }
}
