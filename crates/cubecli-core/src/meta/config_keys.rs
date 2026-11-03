//! Typed `cubecli project config` keys — parse, validate, and apply.

use super::ProjectMeta;
use crate::error::{CoreError, Result};
use crate::model::{AddonSourceId, RestartPolicy};
use crate::schedule::ScheduleRule;

#[derive(Debug)]
pub struct KeyInfo {
    pub name: &'static str,
    pub kind: &'static str,
    pub description: &'static str,
}

pub const KEYS: &[KeyInfo] = &[
    KeyInfo {
        name: "addon-source",
        kind: "modrinth|curseforge",
        description: "default addon source for installs",
    },
    KeyInfo {
        name: "addon-autocheck",
        kind: "bool",
        description: "daemon checks installed addons for updates",
    },
    KeyInfo {
        name: "addon-autoupgrade",
        kind: "slug,slug,...",
        description: "slugs upgraded automatically (empty = none)",
    },
    KeyInfo {
        name: "auto-backup",
        kind: "bool",
        description: "automatic backups on a schedule",
    },
    KeyInfo {
        name: "auto-backup-rule",
        kind: "cron|6h|1d6h|30m|15s",
        description: "when auto backups run",
    },
    KeyInfo {
        name: "auto-backup-limit",
        kind: "int",
        description: "max auto backups kept",
    },
    KeyInfo {
        name: "port",
        kind: "int",
        description: "server port",
    },
    KeyInfo {
        name: "pvp",
        kind: "bool",
        description: "enable PvP",
    },
    KeyInfo {
        name: "restart-policy",
        kind: "manual-only|unless-stopped|cron",
        description: "how the daemon restarts the server",
    },
    KeyInfo {
        name: "restart-cron",
        kind: "cron",
        description: "restart schedule (used when restart-policy=cron)",
    },
    KeyInfo {
        name: "eula",
        kind: "bool",
        description: "accept the Minecraft EULA (https://aka.ms/MinecraftEULA)",
    },
    KeyInfo {
        name: "backend",
        kind: "native|compose",
        description: "who owns the server process",
    },
];

pub fn list() -> &'static [KeyInfo] {
    KEYS
}

pub fn get(meta: &ProjectMeta, key: &str) -> Result<String> {
    let value = match key {
        "addon-source" => meta.addons.default_source.slug().to_string(),
        "addon-autocheck" => meta.addons.autocheck.to_string(),
        "addon-autoupgrade" => meta.addons.autoupgrade.join(","),
        "auto-backup" => meta.backup.auto.to_string(),
        "auto-backup-rule" => meta.backup.rule.clone(),
        "auto-backup-limit" => meta.backup.limit.to_string(),
        "port" => meta.runtime.port.to_string(),
        "pvp" => meta.runtime.pvp.to_string(),
        "restart-policy" => meta.runtime.restart_policy.slug().to_string(),
        "restart-cron" => meta.runtime.restart_cron.clone(),
        "eula" => meta.server.eula_accepted.to_string(),
        "backend" => meta.runtime.backend.slug().to_string(),
        other => return Err(unknown_key(other)),
    };
    Ok(value)
}

/// Apply a validated value to the metadata, returning human-readable
/// warnings (side effects the user should know about).
pub fn set(meta: &mut ProjectMeta, key: &str, value: &str) -> Result<Vec<String>> {
    let value = value.trim();
    let mut warnings = Vec::new();
    match key {
        "addon-source" => {
            meta.addons.default_source = parse_enum(key, value, AddonSourceId::parse)?;
        }
        "addon-autocheck" => meta.addons.autocheck = parse_bool(key, value)?,
        "addon-autoupgrade" => {
            let slugs: Vec<String> = value
                .split([',', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            // dedup, keep order
            let mut deduped = Vec::with_capacity(slugs.len());
            for slug in slugs {
                if !deduped.contains(&slug) {
                    deduped.push(slug);
                }
            }
            meta.addons.autoupgrade = deduped;
        }
        "auto-backup" => meta.backup.auto = parse_bool(key, value)?,
        "auto-backup-rule" => {
            let rule = ScheduleRule::parse(value)
                .map_err(|e| CoreError::invalid_config(key, e.to_string()))?;
            meta.backup.rule = rule.normalize();
            if !meta.backup.auto {
                warnings.push(
                    "auto-backup is off — enable it with `cubecli project config auto-backup=true`"
                        .into(),
                );
            }
        }
        "auto-backup-limit" => {
            let limit: u32 = value.parse().map_err(|_| {
                CoreError::invalid_config(key, format!("`{value}` is not a positive integer"))
            })?;
            if limit == 0 {
                return Err(CoreError::invalid_config(key, "must be at least 1"));
            }
            meta.backup.limit = limit;
        }
        "port" => {
            let port: u16 = value.parse().map_err(|_| {
                CoreError::invalid_config(key, format!("`{value}` is not a valid port"))
            })?;
            if port == 0 {
                return Err(CoreError::invalid_config(
                    key,
                    "must be between 1 and 65535",
                ));
            }
            if port == meta.runtime.rcon_port {
                meta.runtime.rcon_port = if port == 25575 { 25576 } else { 25575 };
                warnings.push(format!(
                    "rcon port collided — moved to {}",
                    meta.runtime.rcon_port
                ));
            }
            meta.runtime.port = port;
            warnings.push("port changed — restart the server to apply".into());
        }
        "pvp" => {
            meta.runtime.pvp = parse_bool(key, value)?;
            warnings.push("pvp changed — restart the server to apply".into());
        }
        "restart-policy" => {
            meta.runtime.restart_policy = parse_enum(key, value, RestartPolicy::parse)?;
            if meta.runtime.restart_policy == RestartPolicy::Cron
                && meta.runtime.restart_cron.trim().is_empty()
            {
                warnings.push(
                    "set the schedule with `cubecli project config restart-cron=\"0 4 * * *\"`"
                        .into(),
                );
            }
        }
        "restart-cron" => {
            let rule = ScheduleRule::parse(value)
                .map_err(|e| CoreError::invalid_config(key, e.to_string()))?;
            let ScheduleRule::Cron(expr) = rule else {
                return Err(CoreError::invalid_config(
                    key,
                    "restart-cron needs a cron expression (e.g. `0 4 * * *`), not a duration",
                ));
            };
            meta.runtime.restart_cron = expr;
        }
        "eula" => {
            let accepted = parse_bool(key, value)?;
            if accepted {
                meta.server.eula_accepted = true;
                warnings.push(
                    "you accepted the Minecraft EULA (a legal agreement with Mojang) — eula.txt updated".into(),
                );
            } else {
                meta.server.eula_accepted = false;
            }
        }
        "backend" => {
            let backend = crate::model::Backend::parse(value).ok_or_else(|| {
                CoreError::invalid_config(key, format!("`{value}` is not native or compose"))
            })?;
            if meta.runtime.backend != backend {
                warnings.push(format!(
                    "backend switched to `{backend}` — takes effect on the next start"
                ));
            }
            meta.runtime.backend = backend;
        }
        other => return Err(unknown_key(other)),
    }
    Ok(warnings)
}

fn unknown_key(key: &str) -> CoreError {
    CoreError::invalid_config(
        key,
        "unknown key — run `cubecli project config` to list them",
    )
}

fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(CoreError::invalid_config(
            key,
            format!("`{value}` is not a boolean (true/false)"),
        )),
    }
}

fn parse_enum<T>(key: &str, value: &str, parse: impl Fn(&str) -> Option<T>) -> Result<T> {
    parse(value)
        .ok_or_else(|| CoreError::invalid_config(key, format!("`{value}` is not a valid value")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AddonSourceId;
    use crate::model::RestartPolicy;

    #[test]
    fn get_roundtrip_all_keys() {
        let meta = ProjectMeta::default();
        for info in KEYS {
            let _ = get(&meta, info.name).unwrap_or_else(|e| panic!("{info:?}: {e}"));
        }
    }

    #[test]
    fn set_source_and_policy() {
        let mut meta = ProjectMeta::default();
        set(&mut meta, "addon-source", "CurseForge").unwrap();
        assert_eq!(meta.addons.default_source, AddonSourceId::CurseForge);
        set(&mut meta, "restart-policy", "unless-stopped").unwrap();
        assert_eq!(meta.runtime.restart_policy, RestartPolicy::UnlessStopped);
    }

    #[test]
    fn set_bool_flexible() {
        let mut meta = ProjectMeta::default();
        set(&mut meta, "pvp", "off").unwrap();
        assert!(!meta.runtime.pvp);
        set(&mut meta, "addon-autocheck", "YES").unwrap();
        assert!(meta.addons.autocheck);
        assert!(set(&mut meta, "pvp", "maybe").is_err());
    }

    #[test]
    fn set_slug_list_dedups() {
        let mut meta = ProjectMeta::default();
        set(&mut meta, "addon-autoupgrade", "sodium, lithium ,sodium").unwrap();
        assert_eq!(meta.addons.autoupgrade, vec!["sodium", "lithium"]);
        assert_eq!(get(&meta, "addon-autoupgrade").unwrap(), "sodium,lithium");
        set(&mut meta, "addon-autoupgrade", "").unwrap();
        assert!(meta.addons.autoupgrade.is_empty());
    }

    #[test]
    fn set_rules_normalize() {
        let mut meta = ProjectMeta::default();
        set(&mut meta, "auto-backup-rule", "1d 6h").unwrap();
        assert_eq!(meta.backup.rule, "1d6h");
        set(&mut meta, "auto-backup-rule", "0 4 * * *").unwrap();
        assert_eq!(meta.backup.rule, "0 4 * * *");
        assert!(set(&mut meta, "auto-backup-rule", "nonsense").is_err());
    }

    #[test]
    fn set_port_bumps_rcon_on_collision() {
        let mut meta = ProjectMeta::default();
        let warnings = set(&mut meta, "port", "25575").unwrap();
        assert_eq!(meta.runtime.port, 25575);
        assert_eq!(meta.runtime.rcon_port, 25576);
        assert!(warnings.iter().any(|w| w.contains("25576")));
    }

    #[test]
    fn set_restart_cron_rejects_duration() {
        let mut meta = ProjectMeta::default();
        assert!(set(&mut meta, "restart-cron", "6h").is_err());
        set(&mut meta, "restart-cron", "*/30 * * * *").unwrap();
        assert_eq!(meta.runtime.restart_cron, "*/30 * * * *");
    }

    #[test]
    fn unknown_key_errors() {
        let mut meta = ProjectMeta::default();
        assert!(matches!(
            set(&mut meta, "nope", "1"),
            Err(CoreError::InvalidConfig { .. })
        ));
    }
}
