//! Server files cubecli manages: EULA, `server.properties` (targeted keys
//! only), the RCON secret, and the player lists.

use crate::error::{CoreError, Result};
use crate::meta::ProjectMeta;
use crate::paths::ProjectLayout;
use crate::properties::PropertiesFile;

/// Read the project's RCON password, generating and persisting one (mode
/// 0600) when missing.
pub fn rcon_secret(layout: &ProjectLayout) -> Result<String> {
    let path = layout.rcon_secret_file();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let secret = generate_secret();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    std::fs::write(&path, &secret).map_err(|e| CoreError::io(&path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| CoreError::io(&path, e))?;
    }
    Ok(secret)
}

/// 32 hex chars of OS-seeded randomness — plenty for an RCON password bound
/// to loopback.
fn generate_secret() -> String {
    use rand::RngExt as _;
    let mut bytes = [0u8; 16];
    rand::rng().fill(bytes.as_mut_slice());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Write `eula.txt`. `true` is only ever written because the user explicitly
/// accepted (wizard) — nothing else in the tool may flip it.
pub fn write_eula(layout: &ProjectLayout, accepted: bool) -> Result<()> {
    let path = layout.root().join("eula.txt");
    let text = if accepted {
        "# Accepted via cubecli create\neula=true\n"
    } else {
        "# Not accepted yet — `cubecli` will refuse to start until the EULA is accepted\neula=false\n"
    };
    std::fs::write(&path, text).map_err(|e| CoreError::io(&path, e))
}

/// Sync cubecli-managed keys into `server.properties` (targeted line edits;
/// other content is untouched). Returns true when anything changed.
pub fn sync_server_properties(meta: &ProjectMeta, layout: &ProjectLayout) -> Result<bool> {
    let path = layout.root().join("server.properties");
    let secret = rcon_secret(layout)?;
    let mut props = PropertiesFile::read(&path)?;
    let changed = props.apply(&[
        ("server-port", &meta.runtime.port.to_string()),
        ("pvp", if meta.runtime.pvp { "true" } else { "false" }),
        ("level-name", &meta.runtime.level_name),
        (
            "motd",
            &format!("{} — powered by cubecli", meta.project.slug),
        ),
        ("enable-rcon", "true"),
        ("rcon.port", &meta.runtime.rcon_port.to_string()),
        ("rcon.password", &secret),
    ]);
    if changed {
        props.write(&path)?;
    }
    Ok(changed)
}

/// Write `ops.json` / `whitelist.json`.
///
/// Vanilla requires UUID+name pairs; username→UUID resolution happens at
/// provisioning time (or lazily via RCON on first start). Until names are
/// resolved we only write empty files so first boot is clean.
pub fn write_player_lists(meta: &ProjectMeta, layout: &ProjectLayout) -> Result<()> {
    let _ = meta;
    for name in ["ops.json", "whitelist.json"] {
        let path = layout.root().join(name);
        if !path.exists() {
            std::fs::write(&path, "[]\n").map_err(|e| CoreError::io(&path, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, ProjectLayout) {
        let tmp = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(tmp.path());
        layout.ensure_state_dirs().unwrap();
        (tmp, layout)
    }

    #[test]
    fn secret_is_generated_once_and_persisted() {
        let (_tmp, layout) = layout();
        let first = rcon_secret(&layout).unwrap();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(rcon_secret(&layout).unwrap(), first);
    }

    #[cfg(unix)]
    #[test]
    fn secret_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, layout) = layout();
        let _ = rcon_secret(&layout).unwrap();
        let mode = std::fs::metadata(layout.rcon_secret_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn eula_explicit_only() {
        let (_tmp, layout) = layout();
        write_eula(&layout, true).unwrap();
        assert!(
            std::fs::read_to_string(layout.root().join("eula.txt"))
                .unwrap()
                .contains("eula=true")
        );
        write_eula(&layout, false).unwrap();
        assert!(
            std::fs::read_to_string(layout.root().join("eula.txt"))
                .unwrap()
                .contains("eula=false")
        );
    }

    #[test]
    fn properties_sync_is_idempotent_and_preserves_content() {
        let (_tmp, layout) = layout();
        std::fs::write(
            layout.root().join("server.properties"),
            "# my edits\nview-distance=12\n",
        )
        .unwrap();

        let meta = ProjectMeta::default();
        assert!(sync_server_properties(&meta, &layout).unwrap());
        assert!(!sync_server_properties(&meta, &layout).unwrap());

        let text = std::fs::read_to_string(layout.root().join("server.properties")).unwrap();
        assert!(text.contains("# my edits"));
        assert!(text.contains("view-distance=12"));
        assert!(text.contains("server-port=25565"));
        assert!(text.contains("enable-rcon=true"));
        assert!(text.contains("rcon.port=25575"));
    }

    #[test]
    fn player_lists_created_empty() {
        let (_tmp, layout) = layout();
        let meta = ProjectMeta::default();
        write_player_lists(&meta, &layout).unwrap();
        assert_eq!(
            std::fs::read_to_string(layout.root().join("ops.json")).unwrap(),
            "[]\n"
        );
        assert_eq!(
            std::fs::read_to_string(layout.root().join("whitelist.json")).unwrap(),
            "[]\n"
        );
    }
}
