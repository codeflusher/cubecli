use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::core_kind::CoreKind;

/// What kind of addon something is. "Addons" = plugins + mods + datapacks +
/// server resource packs (user's scope definition).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddonKind {
    Mod,
    Plugin,
    Datapack,
    ServerResourcePack,
}

impl AddonKind {
    pub const ALL: [AddonKind; 4] = [
        Self::Mod,
        Self::Plugin,
        Self::Datapack,
        Self::ServerResourcePack,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Plugin => "plugin",
            Self::Datapack => "datapack",
            Self::ServerResourcePack => "server-resource-pack",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Mod => "Mod",
            Self::Plugin => "Plugin",
            Self::Datapack => "Datapack",
            Self::ServerResourcePack => "Server resource pack",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.slug().eq_ignore_ascii_case(s))
    }
}

impl fmt::Display for AddonKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// The loader dimension used when filtering addon files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModLoader {
    Vanilla,
    PaperLike,
    Fabric,
    NeoForge,
    Forge,
}

impl CoreKind {
    pub fn loader(self) -> ModLoader {
        match self {
            Self::Vanilla => ModLoader::Vanilla,
            Self::Paper | Self::Purpur => ModLoader::PaperLike,
            Self::Fabric => ModLoader::Fabric,
            Self::NeoForge => ModLoader::NeoForge,
            Self::Forge => ModLoader::Forge,
        }
    }

    /// Whether this core can load a given addon kind.
    pub fn supports_addon(self, kind: AddonKind) -> bool {
        match kind {
            AddonKind::Datapack | AddonKind::ServerResourcePack => true,
            AddonKind::Plugin => matches!(self, Self::Paper | Self::Purpur),
            AddonKind::Mod => matches!(self, Self::Fabric | Self::NeoForge | Self::Forge),
        }
    }
}

/// Where addons come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddonSourceId {
    Modrinth,
    CurseForge,
}

impl AddonSourceId {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth",
            Self::CurseForge => "curseforge",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.slug().eq_ignore_ascii_case(s))
    }

    pub const ALL: [AddonSourceId; 2] = [Self::Modrinth, Self::CurseForge];
}

impl fmt::Display for AddonSourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// One installed addon, recorded in `.cubecli.meta`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledAddon {
    pub slug: String,
    pub source_id: AddonSourceId,
    /// Provider-internal id (Modrinth project id / CurseForge project id).
    pub provider_id: String,
    pub kind: AddonKind,
    pub version: String,
    /// File name inside its target directory.
    pub file: String,
    /// Target directory relative to the project root (includes level dir for datapacks).
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    pub installed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addon_kind_serde() {
        assert_eq!(
            serde_json::to_string(&AddonKind::ServerResourcePack).unwrap(),
            "\"server-resource-pack\""
        );
        let v: AddonKind = serde_json::from_str("\"datapack\"").unwrap();
        assert_eq!(v, AddonKind::Datapack);
    }

    #[test]
    fn compat_matrix() {
        assert!(CoreKind::Paper.supports_addon(AddonKind::Plugin));
        assert!(!CoreKind::Paper.supports_addon(AddonKind::Mod));
        assert!(CoreKind::Fabric.supports_addon(AddonKind::Mod));
        assert!(!CoreKind::Fabric.supports_addon(AddonKind::Plugin));
        assert!(!CoreKind::Vanilla.supports_addon(AddonKind::Plugin));
        assert!(CoreKind::Vanilla.supports_addon(AddonKind::Datapack));
        assert!(CoreKind::NeoForge.supports_addon(AddonKind::ServerResourcePack));
    }

    #[test]
    fn source_parse() {
        assert_eq!(
            AddonSourceId::parse("Modrinth"),
            Some(AddonSourceId::Modrinth)
        );
        assert_eq!(
            AddonSourceId::parse("curseforge"),
            Some(AddonSourceId::CurseForge)
        );
        assert_eq!(AddonSourceId::parse("nexus"), None);
    }
}
