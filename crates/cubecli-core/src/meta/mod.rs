use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::model::{
    AddonKind, AddonSourceId, Backend, CoreKind, InstalledAddon, LaunchSpec, RestartPolicy,
};

pub mod config_keys;
pub mod migrate;

pub use migrate::CURRENT_SCHEMA_VERSION;

/// Full project metadata — the `.cubecli.meta` file.
///
/// Serde policy: `#[serde(default)]` everywhere, never `deny_unknown_fields`,
/// so older binaries can read newer files (gated by `schema_version` in
/// [`migrate`]) and partial files load with sensible defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectMeta {
    pub schema_version: u32,
    pub project: ProjectSection,
    pub server: ServerSection,
    pub runtime: RuntimeSection,
    pub backup: BackupSection,
    pub addons: AddonsSection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSection {
    /// Server slug — used in backup filenames.
    pub slug: String,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSection {
    pub core: CoreKind,
    pub mc_version: String,
    pub core_build: String,
    pub java_version: u32,
    /// Never implicitly true; only wizard acceptance may set it,
    /// and only an accepted EULA lets `project start` proceed.
    pub eula_accepted: bool,
    #[serde(default)]
    pub whitelist: Vec<String>,
    #[serde(default)]
    pub ops: Vec<String>,
    #[serde(default)]
    pub launch: Option<LaunchSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeSection {
    pub backend: Backend,
    pub restart_policy: RestartPolicy,
    /// Cron expression, used when `restart_policy == cron`.
    pub restart_cron: String,
    pub port: u16,
    pub rcon_port: u16,
    pub pvp: bool,
    pub level_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupSection {
    pub auto: bool,
    /// Cron expression or humanized duration (`6h`, `1d6h`, `30m`, `15s`).
    pub rule: String,
    /// Maximum auto backups kept (pruning deletes the oldest beyond this).
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddonsSection {
    pub default_source: AddonSourceId,
    pub autocheck: bool,
    /// Slugs the daemon upgrades automatically.
    pub autoupgrade: Vec<String>,
    #[serde(default)]
    pub installed: Vec<InstalledAddon>,
}

impl Default for ProjectMeta {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectSection::default(),
            server: ServerSection::default(),
            runtime: RuntimeSection::default(),
            backup: BackupSection::default(),
            addons: AddonsSection::default(),
        }
    }
}

impl Default for ProjectSection {
    fn default() -> Self {
        Self {
            slug: "server".into(),
            created_at: None,
        }
    }
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            core: CoreKind::Vanilla,
            mc_version: String::new(),
            core_build: String::new(),
            java_version: 0,
            eula_accepted: false,
            whitelist: Vec::new(),
            ops: Vec::new(),
            launch: None,
        }
    }
}

impl Default for RuntimeSection {
    fn default() -> Self {
        Self {
            backend: Backend::Native,
            restart_policy: RestartPolicy::ManualOnly,
            restart_cron: String::new(),
            port: 25565,
            rcon_port: 25575,
            pvp: true,
            level_name: "world".into(),
        }
    }
}

impl Default for BackupSection {
    fn default() -> Self {
        Self {
            auto: false,
            rule: "1d".into(),
            limit: 10,
        }
    }
}

impl Default for AddonsSection {
    fn default() -> Self {
        Self {
            default_source: AddonSourceId::Modrinth,
            autocheck: false,
            autoupgrade: Vec::new(),
            installed: Vec::new(),
        }
    }
}

impl ProjectMeta {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| CoreError::io(path, e))?;
        migrate::load_from_str(&text)
    }

    /// Atomically persist, stamping the current schema version.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let mut meta = self.clone();
        meta.schema_version = CURRENT_SCHEMA_VERSION;
        let text = toml::to_string_pretty(&meta)
            .map_err(|e| CoreError::Meta(format!("cannot serialize metadata: {e}")))?;
        let tmp = path.with_file_name(format!(
            "{}.tmp",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(".cubecli.meta")
        ));
        std::fs::write(&tmp, text).map_err(|e| CoreError::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))?;
        Ok(())
    }

    /// Validate invariants. Hard errors mean the project is misconfigured;
    /// warnings are advisory. Returns the warnings on success.
    pub fn validate(&self) -> Result<Vec<String>> {
        let mut warnings = Vec::new();
        let rt = &self.runtime;

        if rt.port == 0 {
            return Err(CoreError::invalid_config(
                "port",
                "must be between 1 and 65535",
            ));
        }
        if rt.port < 1024 {
            warnings.push(format!(
                "port {} is a privileged port; prefer 1024+",
                rt.port
            ));
        }
        if rt.rcon_port == 0 || rt.rcon_port == rt.port {
            return Err(CoreError::invalid_config(
                "port",
                format!(
                    "rcon port ({}) must differ from the server port",
                    rt.rcon_port
                ),
            ));
        }

        if rt.restart_policy == RestartPolicy::Cron {
            if rt.restart_cron.trim().is_empty() {
                return Err(CoreError::invalid_config(
                    "restart-cron",
                    "required when restart-policy is `cron`",
                ));
            }
            crate::schedule::ScheduleRule::parse(&rt.restart_cron)?;
        } else if !rt.restart_cron.trim().is_empty() {
            warnings.push(
                "restart-cron is set but restart-policy is not `cron` — it is ignored".into(),
            );
        }

        if self.backup.limit == 0 {
            return Err(CoreError::invalid_config(
                "auto-backup-limit",
                "must be at least 1",
            ));
        }
        if self.backup.auto {
            crate::schedule::ScheduleRule::parse(&self.backup.rule)?;
        }

        if self.server.mc_version.is_empty() {
            warnings.push("mc version is not set — provisioning is incomplete".into());
        }
        if self.server.java_version == 0 {
            warnings.push("java version unknown — it will be resolved during provisioning".into());
        }
        if !self.server.eula_accepted {
            warnings.push(
                "Minecraft EULA not accepted yet — `project start` will refuse until it is".into(),
            );
        }

        for addon in &self.addons.installed {
            if !self.server.core.supports_addon(addon.kind) {
                return Err(CoreError::IncompatibleAddon {
                    kind: addon.kind.slug().into(),
                    core: self.server.core.slug().into(),
                });
            }
        }

        Ok(warnings)
    }

    pub fn find_installed(&self, slug: &str) -> Option<&InstalledAddon> {
        self.addons
            .installed
            .iter()
            .find(|a| a.slug.eq_ignore_ascii_case(slug))
    }

    pub fn addon_kinds(&self) -> impl Iterator<Item = AddonKind> + '_ {
        self.addons.installed.iter().map(|a| a.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_meta() -> ProjectMeta {
        ProjectMeta {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectSection {
                slug: "my-server".into(),
                created_at: Some(Utc::now()),
            },
            server: ServerSection {
                core: CoreKind::Fabric,
                mc_version: "1.21.8".into(),
                core_build: "132".into(),
                java_version: 21,
                eula_accepted: true,
                whitelist: vec!["alice".into()],
                ops: vec!["bob".into()],
                launch: Some(LaunchSpec::Jar {
                    jar: "paper-1.21.8-132.jar".into(),
                    jvm_args: vec!["-Xmx4G".into()],
                    args: vec!["nogui".into()],
                }),
            },
            runtime: RuntimeSection {
                backend: Backend::Native,
                restart_policy: RestartPolicy::UnlessStopped,
                restart_cron: String::new(),
                port: 25565,
                rcon_port: 25575,
                pvp: true,
                level_name: "world".into(),
            },
            backup: BackupSection {
                auto: true,
                rule: "6h".into(),
                limit: 5,
            },
            addons: AddonsSection {
                default_source: AddonSourceId::Modrinth,
                autocheck: true,
                autoupgrade: vec!["sodium".into()],
                installed: vec![InstalledAddon {
                    slug: "sodium".into(),
                    source_id: AddonSourceId::Modrinth,
                    provider_id: "AANobbMI".into(),
                    kind: AddonKind::Mod,
                    version: "0.6.5".into(),
                    file: "sodium-0.6.5.jar".into(),
                    dir: "mods".into(),
                    sha1: Some("abc".into()),
                    installed_at: Utc::now(),
                    deps: vec![],
                }],
            },
        }
    }

    #[test]
    fn toml_roundtrip_preserves_everything() {
        let meta = full_meta();
        let text = toml::to_string_pretty(&meta).unwrap();
        let parsed = migrate::load_from_str(&text).unwrap();
        assert_eq!(parsed, meta);
        assert!(text.contains("schema_version = 1"));
        assert!(text.contains("unless-stopped"));
    }

    #[test]
    fn partial_file_loads_with_defaults() {
        let meta =
            migrate::load_from_str("[project]\nslug = \"partial\"\n[server]\ncore = \"fabric\"\n")
                .unwrap();
        assert_eq!(meta.project.slug, "partial");
        assert_eq!(meta.server.core, CoreKind::Fabric);
        assert_eq!(meta.runtime.port, 25565);
        assert!(!meta.server.eula_accepted);
        assert_eq!(meta.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        // Newer file with extra sections must still parse.
        let meta = migrate::load_from_str(
            "schema_version = 1\n[project]\nslug = \"x\"\n[bogus_section]\nfuture = true\n",
        )
        .unwrap();
        assert_eq!(meta.project.slug, "x");
    }

    #[test]
    fn newer_schema_is_rejected() {
        let err = migrate::load_from_str("schema_version = 99\n");
        assert!(matches!(err, Err(CoreError::Migration(_))));
    }

    #[test]
    fn v0_file_migrates_to_current() {
        let meta = migrate::load_from_str("[project]\nslug = \"old\"\n").unwrap();
        assert_eq!(meta.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(meta.project.slug, "old");
    }

    #[test]
    fn validate_catches_cron_policy_without_expression() {
        let mut meta = full_meta();
        meta.runtime.restart_policy = RestartPolicy::Cron;
        meta.runtime.restart_cron = String::new();
        assert!(matches!(
            meta.validate(),
            Err(CoreError::InvalidConfig { key, .. }) if key == "restart-cron"
        ));

        meta.runtime.restart_cron = "0 4 * * *".into();
        assert!(meta.validate().is_ok());
    }

    #[test]
    fn validate_incompatible_addon() {
        let mut meta = full_meta();
        meta.server.core = CoreKind::Paper; // installed addon is a mod
        assert!(matches!(
            meta.validate(),
            Err(CoreError::IncompatibleAddon { .. })
        ));
    }

    #[test]
    fn validate_port_conflicts() {
        let mut meta = full_meta();
        meta.runtime.rcon_port = meta.runtime.port;
        assert!(meta.validate().is_err());
    }

    #[test]
    fn validate_eula_warning() {
        let mut meta = full_meta();
        meta.server.eula_accepted = false;
        let warnings = meta.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("EULA")));
    }

    #[test]
    fn find_installed_is_case_insensitive() {
        let meta = full_meta();
        assert!(meta.find_installed("SODIUM").is_some());
        assert!(meta.find_installed("nope").is_none());
    }
}
