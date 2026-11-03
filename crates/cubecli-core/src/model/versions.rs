use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A Minecraft version (e.g. `1.21.8`, `26.3`). New-style version ids coexist
/// with the old `1.x` scheme — never assume a `1.` prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McVersion {
    pub id: String,
    #[serde(default = "crate::model::versions::default_true")]
    pub stable: bool,
}

fn default_true() -> bool {
    true
}

/// Release channel of a core build. Only `Stable` builds are offered by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildChannel {
    #[default]
    Stable,
    Experimental,
}

impl BuildChannel {
    /// Map provider-specific channel strings (`STABLE`, `release`, `beta`, …).
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "STABLE" | "RELEASE" => Self::Stable,
            _ => Self::Experimental,
        }
    }
}

/// A concrete build of a server core for one MC version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreBuild {
    /// Build id as a string (`"132"`, loader version `"0.20.1"`, …).
    pub id: String,
    #[serde(default)]
    pub channel: BuildChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Required Java feature version (e.g. `21`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JavaRequirement {
    pub major: u32,
}

/// A download checksum with its algorithm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checksum {
    Sha1(String),
    Sha256(String),
    Md5(String),
}

impl Checksum {
    pub fn algorithm(&self) -> &'static str {
        match self {
            Self::Sha1(_) => "sha1",
            Self::Sha256(_) => "sha256",
            Self::Md5(_) => "md5",
        }
    }

    pub fn expected_hex(&self) -> &str {
        match self {
            Self::Sha1(h) | Self::Sha256(h) | Self::Md5(h) => h,
        }
    }

    pub fn parse(algorithm: &str, hex: impl Into<String>) -> Option<Self> {
        let hex = hex.into();
        match algorithm.to_ascii_lowercase().as_str() {
            "sha1" => Some(Self::Sha1(hex)),
            "sha256" | "sha-256" => Some(Self::Sha256(hex)),
            "md5" => Some(Self::Md5(hex)),
            _ => None,
        }
    }

    /// Case-insensitive hex comparison against a computed digest.
    pub fn matches(&self, actual_hex: &str) -> bool {
        self.expected_hex().eq_ignore_ascii_case(actual_hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_parse() {
        assert_eq!(BuildChannel::parse("STABLE"), BuildChannel::Stable);
        assert_eq!(BuildChannel::parse("release"), BuildChannel::Stable);
        assert_eq!(
            BuildChannel::parse("EXPERIMENTAL"),
            BuildChannel::Experimental
        );
        assert_eq!(BuildChannel::parse("beta"), BuildChannel::Experimental);
    }

    #[test]
    fn checksum_compare_case_insensitive() {
        let c = Checksum::Sha1("ABCDEF".into());
        assert!(c.matches("abcdef"));
        assert!(!c.matches("000000"));
        assert_eq!(c.algorithm(), "sha1");
        assert_eq!(
            Checksum::parse("SHA256", "ff"),
            Some(Checksum::Sha256("ff".into()))
        );
        assert_eq!(Checksum::parse("crc32", "ff"), None);
    }
}
