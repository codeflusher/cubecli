use crate::error::{CoreError, Result};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Parse `.cubecli.meta` text, running any pending schema migrations first.
///
/// Files written by a *newer* cubecli are rejected (never silently
/// downgraded — saving would drop fields the newer version added).
pub(crate) fn load_from_str(text: &str) -> Result<crate::meta::ProjectMeta> {
    let mut value: toml::Value =
        toml::from_str(text).map_err(|e| CoreError::Meta(format!("invalid TOML: {e}")))?;

    let version = value
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        .unwrap_or(0);
    if version < 0 {
        return Err(CoreError::Meta(
            "schema_version must be a positive integer".into(),
        ));
    }
    let version = version as u32;

    if version > CURRENT_SCHEMA_VERSION {
        return Err(CoreError::Migration(format!(
            "project was written by a newer cubecli (schema {version} > {CURRENT_SCHEMA_VERSION})"
        )));
    }

    for migration in &MIGRATIONS[version as usize..CURRENT_SCHEMA_VERSION as usize] {
        migration(&mut value)?;
    }

    let mut meta: crate::meta::ProjectMeta = value
        .try_into()
        .map_err(|e| CoreError::Meta(format!("invalid metadata structure: {e}")))?;
    meta.schema_version = CURRENT_SCHEMA_VERSION;
    Ok(meta)
}

pub type MigrationFn = fn(&mut toml::Value) -> Result<()>;

/// `MIGRATIONS[v]` upgrades schema v → v+1, so a v3 binary can still read a
/// v1 file even after the old struct shapes are gone.
pub const MIGRATIONS: &[MigrationFn] = &[migrate_v0_to_v1];

/// v0 files (pre-release, no schema_version field) predate every section
/// default — the serde defaults fill in all gaps.
fn migrate_v0_to_v1(_value: &mut toml::Value) -> Result<()> {
    Ok(())
}
