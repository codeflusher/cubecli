use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::model::AddonKind;

/// Marker file at the project root — the whole tool keys off its existence.
pub const META_FILE: &str = ".cubecli.meta";
const STATE_DIR: &str = ".cubecli";

/// Layout of a single server project directory.
#[derive(Debug, Clone)]
pub struct ProjectLayout {
    root: PathBuf,
}

impl ProjectLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn meta_file(&self) -> PathBuf {
        self.root.join(META_FILE)
    }

    // --- hidden state (`.cubecli/`) ---

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }

    pub fn run_dir(&self) -> PathBuf {
        self.state_dir().join("run")
    }

    pub fn pidfile(&self) -> PathBuf {
        self.run_dir().join("server.pid")
    }

    /// Presence of this marker means the user explicitly ran `project stop`;
    /// the daemon must not auto-restart while it exists.
    pub fn manually_stopped_marker(&self) -> PathBuf {
        self.run_dir().join("manually-stopped")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir().join("logs")
    }

    pub fn server_log(&self) -> PathBuf {
        self.logs_dir().join("server.log")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.state_dir().join("backups")
    }

    pub fn rcon_secret_file(&self) -> PathBuf {
        self.state_dir().join("rcon.secret")
    }

    // --- server content directories ---

    pub fn mods_dir(&self) -> PathBuf {
        self.root.join("mods")
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.root.join("plugins")
    }

    pub fn server_resource_packs_dir(&self) -> PathBuf {
        self.root.join("server-resource-packs")
    }

    pub fn datapacks_dir(&self, level_name: &str) -> PathBuf {
        self.root.join(level_name).join("datapacks")
    }

    /// Target directory for an addon kind. `level_name` only matters for datapacks.
    pub fn addon_dir(&self, kind: AddonKind, level_name: &str) -> PathBuf {
        match kind {
            AddonKind::Mod => self.mods_dir(),
            AddonKind::Plugin => self.plugins_dir(),
            AddonKind::Datapack => self.datapacks_dir(level_name),
            AddonKind::ServerResourcePack => self.server_resource_packs_dir(),
        }
    }

    pub fn ensure_state_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.state_dir(),
            self.run_dir(),
            self.logs_dir(),
            self.backups_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Layout of `~/.cubecli` — global tool state shared by all projects.
#[derive(Debug, Clone)]
pub struct GlobalLayout {
    root: PathBuf,
}

impl GlobalLayout {
    pub fn discover() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| {
            CoreError::external("cubecli", "cannot determine the home directory (set $HOME)")
        })?;
        Ok(Self {
            root: home.join(".cubecli"),
        })
    }

    #[cfg(test)]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn jdks_dir(&self) -> PathBuf {
        self.root.join("jdks")
    }

    pub fn jdk_dir(&self, feature: u32) -> PathBuf {
        self.jdks_dir().join(feature.to_string())
    }

    pub fn registry_file(&self) -> PathBuf {
        self.root.join("registry.toml")
    }

    pub fn daemon_dir(&self) -> PathBuf {
        self.root.join("daemon")
    }

    pub fn daemon_state_dir(&self) -> PathBuf {
        self.daemon_dir().join("state")
    }

    pub fn daemon_state_file(&self, slug: &str) -> PathBuf {
        self.daemon_state_dir().join(format!("{slug}.json"))
    }

    pub fn daemon_outdated_file(&self, slug: &str) -> PathBuf {
        self.daemon_state_dir()
            .join(format!("{slug}.outdated.json"))
    }

    pub fn heartbeat_file(&self) -> PathBuf {
        self.daemon_dir().join("heartbeat")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn daemon_out_log(&self) -> PathBuf {
        self.logs_dir().join("daemon.out.log")
    }

    pub fn daemon_err_log(&self) -> PathBuf {
        self.logs_dir().join("daemon.err.log")
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.root().to_path_buf(),
            self.jdks_dir(),
            self.daemon_dir(),
            self.daemon_state_dir(),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_layout_paths() {
        let l = ProjectLayout::new("/srv/mc");
        assert_eq!(l.meta_file(), PathBuf::from("/srv/mc/.cubecli.meta"));
        assert_eq!(
            l.pidfile(),
            PathBuf::from("/srv/mc/.cubecli/run/server.pid")
        );
        assert_eq!(
            l.manually_stopped_marker(),
            PathBuf::from("/srv/mc/.cubecli/run/manually-stopped")
        );
        assert_eq!(
            l.server_log(),
            PathBuf::from("/srv/mc/.cubecli/logs/server.log")
        );
        assert_eq!(
            l.datapacks_dir("world"),
            PathBuf::from("/srv/mc/world/datapacks")
        );
        assert_eq!(
            l.addon_dir(AddonKind::Plugin, "world"),
            PathBuf::from("/srv/mc/plugins")
        );
        assert_eq!(
            l.addon_dir(AddonKind::Datapack, "world"),
            PathBuf::from("/srv/mc/world/datapacks")
        );
    }

    #[test]
    fn ensure_state_dirs_creates_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let l = ProjectLayout::new(tmp.path());
        l.ensure_state_dirs().unwrap();
        assert!(l.run_dir().is_dir());
        assert!(l.logs_dir().is_dir());
        assert!(l.backups_dir().is_dir());
    }
}
