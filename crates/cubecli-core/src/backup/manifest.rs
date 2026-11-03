//! `backup-meta.json` — the manifest inside every backup archive.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const NAME: &str = "backup-meta.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Manifest {
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub mc_version: String,
    #[serde(default)]
    pub core: String,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub bytes: u64,
}

pub fn to_json(manifest: &Manifest) -> String {
    serde_json::to_string_pretty(manifest).unwrap_or_else(|_| "{}".into())
}

pub fn from_json(bytes: &[u8]) -> Option<Manifest> {
    serde_json::from_slice(bytes).ok()
}

/// A safety backup made before a restore — regular Manifest semantics.
pub type SafetyBackup = Manifest;
