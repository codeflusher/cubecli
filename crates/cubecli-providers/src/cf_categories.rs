//! CurseForge class-id discovery.
//!
//! The numeric classIds (mods / plugins / resource packs / …) are NOT
//! hardcoded — they're resolved from `/categories?gameId=432` (`isClass`
//! entries) and cached locally for 24 h. This survives CurseForge renaming
//! or renumbering, which community mappings have historically gotten wrong.

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use cubecli_core::model::AddonKind;

use crate::error::{ProviderError, Result as PResult};

pub const GAME_ID: u32 = 432;
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Which class we want, by its (fairly stable) slug on CurseForge.
fn class_slug(kind: AddonKind) -> Option<&'static str> {
    match kind {
        AddonKind::Mod => Some("mods"),
        AddonKind::Plugin => Some("bukkit-plugins"),
        AddonKind::ServerResourcePack => Some("resource-packs"),
        // CurseForge has no separate datapack class — datapacks live under mods.
        AddonKind::Datapack => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClassCache {
    pub fetched_at: u64,
    pub classes: std::collections::HashMap<String, u32>, // slug → classId
}

impl ClassCache {
    pub fn class_id(&self, kind: AddonKind) -> Option<u32> {
        class_slug(kind).and_then(|slug| self.classes.get(slug).copied())
    }
}

/// Load the local cache; `None` when missing or older than the TTL.
pub fn load_cache(cache_path: &Path) -> Option<ClassCache> {
    let text = std::fs::read_to_string(cache_path).ok()?;
    let cache: ClassCache = serde_json::from_str(&text).ok()?;
    let age = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .saturating_sub(Duration::from_secs(cache.fetched_at));
    (age < CACHE_TTL).then_some(cache)
}

/// Fetch `/categories?gameId=432` and keep only `isClass` entries.
pub async fn refresh(
    client: &reqwest::Client,
    api_key: &str,
    cache_path: &Path,
) -> PResult<ClassCache> {
    let url = format!("https://api.curseforge.com/v1/categories?gameId={GAME_ID}");
    let body = crate::curseforge::get_json_with_key(client, api_key, &url).await?;
    let mut classes = std::collections::HashMap::new();
    for category in body
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if category.get("isClass").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let Some(slug) = category.get("slug").and_then(Value::as_str) else {
            continue;
        };
        if let Some(id) = category.get("id").and_then(Value::as_u64) {
            classes.insert(slug.to_string(), id as u32);
        }
    }
    let cache = ClassCache {
        fetched_at: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        classes,
    };
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        cache_path,
        serde_json::to_string_pretty(&cache).unwrap_or_default(),
    );
    Ok(cache)
}

/// Cache-first class-id lookup; fails with an actionable error when cold
/// and the API is unreachable.
pub async fn class_id_for(
    client: &reqwest::Client,
    api_key: &str,
    cache_path: &Path,
    kind: AddonKind,
) -> PResult<u32> {
    if let Some(id) = load_cache(cache_path).and_then(|c| c.class_id(kind)) {
        return Ok(id);
    }
    let cache = refresh(client, api_key, cache_path).await?;
    cache.class_id(kind).ok_or_else(|| {
        ProviderError::unsupported(
            "curseforge",
            format!("no {} class found on CurseForge", kind.display_name()),
        )
    })
}
