//! Global tool configuration — `~/.cubecli/config.toml`.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::model::Backend;

pub const CURSEFORGE_KEY_ENV: &str = "CUBECLI_CURSEFORGE_KEY";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// CurseForge REST API key (their API requires an approved key).
    /// The `CUBECLI_CURSEFORGE_KEY` environment variable overrides this.
    pub curseforge_api_key: Option<String>,
    pub default_backend: Backend,
    pub daemon_tick_secs: u64,
    /// Minimum interval between addon update checks (jitter is added).
    pub addon_check_interval_secs: u64,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            curseforge_api_key: None,
            default_backend: Backend::Native,
            daemon_tick_secs: 30,
            addon_check_interval_secs: 12 * 60 * 60,
        }
    }
}

impl GlobalConfig {
    /// Load, falling back to defaults when the file does not exist yet.
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
            .map_err(|e| CoreError::Meta(format!("cannot serialize config: {e}")))?;
        std::fs::write(path, text).map_err(|e| CoreError::io(path, e))?;
        Ok(())
    }

    /// CurseForge key: environment override first, then the config file.
    pub fn curseforge_key(&self) -> Option<String> {
        std::env::var(CURSEFORGE_KEY_ENV)
            .ok()
            .filter(|k| !k.is_empty())
            .or_else(|| self.curseforge_api_key.clone())
    }

    pub fn daemon_tick(&self) -> Duration {
        Duration::from_secs(self.daemon_tick_secs.max(5))
    }

    pub fn addon_check_interval(&self) -> Duration {
        Duration::from_secs(self.addon_check_interval_secs.max(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let cfg = GlobalConfig::load("/nonexistent/cubecli/config.toml").unwrap();
        assert_eq!(cfg, GlobalConfig::default());
        assert_eq!(cfg.default_backend, Backend::Native);
    }

    #[test]
    fn roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let cfg = GlobalConfig {
            curseforge_api_key: Some("key".into()),
            default_backend: Backend::Compose,
            daemon_tick_secs: 15,
            addon_check_interval_secs: 60,
        };
        cfg.save(&path).unwrap();
        let loaded = GlobalConfig::load(&path).unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn env_overrides_file() {
        unsafe { std::env::set_var(CURSEFORGE_KEY_ENV, "env-key") };
        let cfg = GlobalConfig {
            curseforge_api_key: Some("file-key".into()),
            ..Default::default()
        };
        assert_eq!(cfg.curseforge_key().as_deref(), Some("env-key"));
        unsafe { std::env::remove_var(CURSEFORGE_KEY_ENV) };
        assert_eq!(cfg.curseforge_key().as_deref(), Some("file-key"));
    }
}
