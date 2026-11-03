use std::fmt;

use serde::{Deserialize, Serialize};

/// The server core (engine/loader) a project runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreKind {
    Vanilla,
    Paper,
    Purpur,
    Fabric,
    NeoForge,
    Forge,
}

impl CoreKind {
    pub const ALL: [CoreKind; 6] = [
        Self::Vanilla,
        Self::Paper,
        Self::Purpur,
        Self::Fabric,
        Self::NeoForge,
        Self::Forge,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Paper => "paper",
            Self::Purpur => "purpur",
            Self::Fabric => "fabric",
            Self::NeoForge => "neoforge",
            Self::Forge => "forge",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Paper => "Paper",
            Self::Purpur => "Purpur",
            Self::Fabric => "Fabric",
            Self::NeoForge => "NeoForge",
            Self::Forge => "Forge",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Vanilla => "The official Mojang server. No plugins or mods.",
            Self::Paper => "High-performance plugin server (Bukkit API family).",
            Self::Purpur => "Paper fork with extra configuration and features.",
            Self::Fabric => "Lightweight mod loader with a fast update cycle.",
            Self::NeoForge => "Community-driven successor to Forge for modern versions.",
            Self::Forge => "The classic mod loader with the largest mod ecosystem.",
        }
    }

    /// Cores that run a `--installServer` installer jar during provisioning.
    pub fn uses_installer(self) -> bool {
        matches!(self, Self::NeoForge | Self::Forge)
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.slug().eq_ignore_ascii_case(s))
    }
}

impl fmt::Display for CoreKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip() {
        assert_eq!(
            serde_json::to_string(&CoreKind::NeoForge).unwrap(),
            "\"neoforge\""
        );
        let v: CoreKind = serde_json::from_str("\"paper\"").unwrap();
        assert_eq!(v, CoreKind::Paper);
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(CoreKind::parse("NeoForge"), Some(CoreKind::NeoForge));
        assert_eq!(CoreKind::parse("PAPER"), Some(CoreKind::Paper));
        assert_eq!(CoreKind::parse("nope"), None);
    }
}
