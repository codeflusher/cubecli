//! World backups: `<slug>-YYYY-MM-DD-HH_MM.zip` in `.cubecli/backups/`.
//!
//! Contents: every world directory (root-level dir containing `level.dat`),
//! `server.properties`, and the player lists — enough to reconstruct the
//! server's state. A `backup-meta.json` manifest rides inside the archive.

pub mod manifest;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};
use zip::write::SimpleFileOptions;

use crate::error::{CoreError, Result};
use crate::meta::ProjectMeta;
use crate::paths::ProjectLayout;

/// Directories whose contents never belong in a backup.
const EXCLUDED_DIRS: &[&str] = &[
    ".cubecli",
    "mods",
    "plugins",
    "libraries",
    "versions",
    "logs",
    "cache",
    "config",
    "user_jvm_args.txt.d",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    /// From the filename; `None` when the name doesn't parse.
    pub created: Option<DateTime<Utc>>,
}

/// World directories: root-level dirs holding a `level.dat`.
pub fn discover_worlds(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut worlds = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("level.dat").is_file() {
            worlds.push(path);
        }
    }
    worlds.sort();
    worlds
}

fn timestamp() -> String {
    Local::now().format("%Y-%m-%d-%H_%M").to_string()
}

pub fn backup_name(slug: &str) -> String {
    format!("{slug}-{}.zip", timestamp())
}

/// A collision-free archive path: minute-granularity names collide when two
/// backups happen in the same minute (e.g. a safety backup right before a
/// restore), so existing files get a `-SS` seconds suffix, then -1, -2…
fn unique_backup_path(dir: &Path, slug: &str) -> PathBuf {
    let base = backup_name(slug);
    let candidate = dir.join(&base);
    if !candidate.exists() {
        return candidate;
    }
    // seconds suffix
    let secs = Local::now().format("%S").to_string();
    let stem = base.strip_suffix(".zip").unwrap_or(&base);
    let with_secs = dir.join(format!("{stem}-{secs}.zip"));
    if !with_secs.exists() {
        return with_secs;
    }
    // counter suffix
    for n in 1..1000 {
        let candidate = dir.join(format!("{stem}-{secs}-{n}.zip"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!(
        "{stem}-{secs}-{}.zip",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

/// Files collected for the archive, as (archive-relative name, absolute path).
fn collect_files(layout: &ProjectLayout, meta: &ProjectMeta) -> Result<Vec<(String, PathBuf)>> {
    let root = layout.root();
    let mut out = Vec::new();

    for world in discover_worlds(root) {
        collect_tree(root, &world, &mut out, &[])?;
    }

    for name in ["server.properties", "ops.json", "whitelist.json"] {
        let path = root.join(name);
        if path.is_file() {
            out.push((name.to_string(), path));
        }
    }
    let _ = meta;
    Ok(out)
}

fn collect_tree(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
    _exclude: &[&str],
) -> Result<()> {
    for entry in walkdir::WalkDir::new(dir).min_depth(1).sort_by_file_name() {
        let entry = entry.map_err(|e| CoreError::Runtime(format!("walk failed: {e}")))?;
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| CoreError::Runtime("walk escaped project root".into()))?;
        // Skip anything inside our excluded top-level dirs (worlds never are).
        if relative
            .components()
            .next()
            .is_some_and(|c| EXCLUDED_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref()))
        {
            continue;
        }
        out.push((relative.display().to_string(), path.to_path_buf()));
    }
    Ok(())
}

/// Create a backup archive; returns its path. Prunes afterwards.
pub fn create(layout: &ProjectLayout, meta: &ProjectMeta) -> Result<PathBuf> {
    let dir = layout.backups_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let path = unique_backup_path(&dir, &meta.project.slug);

    let files = collect_files(layout, meta)?;
    let file = std::fs::File::create(&path).map_err(|e| CoreError::io(&path, e))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut count = 0u64;
    let mut total: u64 = 0;
    for (name, disk_path) in &files {
        let data = std::fs::read(disk_path).map_err(|e| CoreError::io(disk_path, e))?;
        zip.start_file(name.as_str(), options.clone())
            .map_err(|e| CoreError::Runtime(format!("zip write failed: {e}")))?;
        zip.write_all(&data).map_err(|e| CoreError::io(&path, e))?;
        count += 1;
        total += data.len() as u64;
    }

    let manifest = manifest::Manifest {
        created_at: Some(Utc::now()),
        slug: meta.project.slug.clone(),
        mc_version: meta.server.mc_version.clone(),
        core: meta.server.core.slug().into(),
        files: count,
        bytes: total,
    };
    zip.start_file(manifest::NAME, options)
        .map_err(|e| CoreError::Runtime(format!("zip write failed: {e}")))?;
    zip.write_all(manifest::to_json(&manifest).as_bytes())
        .map_err(|e| CoreError::io(&path, e))?;
    zip.finish()
        .map_err(|e| CoreError::Runtime(format!("zip finish failed: {e}")))?;

    prune(&dir, meta.backup.limit)?;
    Ok(path)
}

/// Delete the oldest backups beyond `limit`. Returns removed paths.
pub fn prune(dir: &Path, limit: u32) -> Result<Vec<PathBuf>> {
    let mut backups = list(dir);
    if backups.len() <= limit as usize {
        return Ok(vec![]);
    }
    backups.sort_by(|a, b| a.created.cmp(&b.created).then(a.name.cmp(&b.name)));
    let excess = backups.len() - limit as usize;
    let mut removed = Vec::new();
    for backup in &backups[..excess] {
        std::fs::remove_file(&backup.path).map_err(|e| CoreError::io(&backup.path, e))?;
        removed.push(backup.path.clone());
    }
    Ok(removed)
}

/// All backups, oldest first (creation parsed from the filename).
pub fn list(dir: &Path) -> Vec<BackupInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("zip") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        // slug-YYYY-MM-DD-HH_MM.zip → timestamp part after the last '-slug' segment:
        // find the first 10-char date pattern from the right side.
        let created = parse_timestamp(&name);
        out.push(BackupInfo {
            path: path.clone(),
            name,
            size,
            created,
        });
    }
    out.sort_by(|a, b| a.created.cmp(&b.created));
    out
}

fn parse_timestamp(name: &str) -> Option<DateTime<Utc>> {
    use chrono::TimeZone as _;
    let bytes = name.as_bytes();
    for i in 0..bytes.len() {
        // %Y accepts signed years — only try at digit starts so a leading
        // "-" never turns "2022" into the year -2022.
        if !bytes[i].is_ascii_digit() {
            continue;
        }
        let rest = &name[i..];
        if rest.len() < 16 {
            break;
        }
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(&rest[..16], "%Y-%m-%d-%H_%M") {
            return Some(
                chrono::Local
                    .from_local_datetime(&naive)
                    .single()
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|| naive.and_utc()),
            );
        }
    }
    None
}

/// Match a user-supplied id (timestamp fragment or filename prefix) to a backup.
pub fn find_by_id<'a>(backups: &'a [BackupInfo], id: &str) -> Option<&'a BackupInfo> {
    let id = id.trim();
    backups.iter().find(|b| {
        b.name == id
            || b.name.starts_with(id)
            || b.name
                .rsplit_once('-')
                .map(|(_, stamp)| stamp.starts_with(id))
                .unwrap_or(false)
            || b.name.contains(&format!("-{id}"))
    })
}

/// Extract `archive` into the project root (worlds + configs only).
/// Zip-slip: entries whose canonical path escapes the root abort the restore
/// before anything is written.
pub fn restore(layout: &ProjectLayout, archive: &Path) -> Result<manifest::Manifest> {
    let file = std::fs::File::open(archive).map_err(|e| CoreError::io(archive, e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Runtime(format!("cannot read archive: {e}")))?;
    let root = layout
        .root()
        .canonicalize()
        .map_err(|e| CoreError::io(layout.root(), e))?;

    // validate every entry first (all-or-nothing)
    let mut plan = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| CoreError::Runtime(format!("bad archive entry {i}: {e}")))?;
        let Some(relative) = entry.enclosed_name() else {
            let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
            return Err(CoreError::Runtime(format!(
                "archive entry {name} escapes the project directory — refusing to restore"
            )));
        };
        let dest = root.join(&relative);
        let content = {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| CoreError::Runtime(format!("read failed: {e}")))?;
            buf
        };
        plan.push((dest, relative.display().to_string(), content));
    }

    for (dest, _name, content) in plan {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
        }
        std::fs::write(&dest, content).map_err(|e| CoreError::io(&dest, e))?;
    }

    // Manifest if present (already validated above; read it back cleanly).
    drop(zip);
    let file = std::fs::File::open(archive).map_err(|e| CoreError::io(archive, e))?;
    let mut zip: zip::ZipArchive<std::fs::File> = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Runtime(format!("cannot read archive: {e}")))?;
    let meta = if zip.by_name(manifest::NAME).is_ok() {
        let mut entry = zip.by_name(manifest::NAME).unwrap();
        let mut buf = Vec::new();
        let _ = entry.read_to_end(&mut buf);
        manifest::from_json(&buf).unwrap_or_default()
    } else {
        manifest::Manifest::default()
    };
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_fixture(root: &Path) {
        for rel in [
            "world/region/r.0.0.mca",
            "world/level.dat",
            "world/DIM-1/level.dat",
            "server.properties",
            "mods/some-mod.jar",
            "libraries/net/thing.jar",
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("content of {rel}")).unwrap();
        }
    }

    fn layout_meta() -> (tempfile::TempDir, ProjectLayout, ProjectMeta) {
        let tmp = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(tmp.path());
        layout.ensure_state_dirs().unwrap();
        let mut meta = ProjectMeta::default();
        meta.project.slug = "test-server".into();
        (tmp, layout, meta)
    }

    #[test]
    fn discover_worlds_finds_level_dat_dirs_only() {
        let tmp = tempfile::tempdir().unwrap();
        world_fixture(tmp.path());
        let worlds = discover_worlds(tmp.path());
        assert_eq!(worlds, vec![tmp.path().join("world")]);
    }

    #[test]
    fn create_and_restore_roundtrip() {
        let (_tmp, layout, meta) = layout_meta();
        world_fixture(layout.root());

        let archive = create(&layout, &meta).unwrap();
        assert!(archive.is_file());
        assert!(
            archive
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("test-server-")
        );

        // mutate + delete, then restore
        std::fs::remove_dir_all(layout.root().join("world")).unwrap();
        std::fs::write(layout.root().join("server.properties"), "corrupted").unwrap();
        let restored = restore(&layout, &archive).unwrap();
        assert_eq!(restored.slug, "test-server");
        assert_eq!(
            std::fs::read_to_string(layout.root().join("server.properties")).unwrap(),
            "content of server.properties"
        );
        assert!(layout.root().join("world/region/r.0.0.mca").is_file());
        // addons live outside backups — restore must leave them alone
        assert!(layout.root().join("mods/some-mod.jar").exists());
    }

    #[test]
    fn prune_keeps_limit_deletes_oldest() {
        let (_tmp, layout, mut meta) = layout_meta();
        world_fixture(layout.root());
        meta.backup.limit = 3;

        // fabricate three "old" backups with known names
        let dir = layout.backups_dir();
        for stamp in ["2020-01-01-00_00", "2021-01-01-00_00", "2022-01-01-00_00"] {
            std::fs::write(dir.join(format!("test-server-{stamp}.zip")), "old").unwrap();
        }
        let removed = prune(&dir, 3).unwrap();
        assert!(removed.is_empty());

        let archive = create(&layout, &meta).unwrap(); // 4th backup → prune oldest
        let remaining = list(&dir);
        assert_eq!(remaining.len(), 3);
        assert!(!remaining.iter().any(|b| b.name.contains("2020-01-01")));
        assert!(archive.exists());
    }

    #[test]
    fn find_by_id_matches_partial_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test-server-2026-10-31-14_30.zip"), "x").unwrap();
        let backups = list(dir.path());
        assert_eq!(backups.len(), 1);
        assert!(find_by_id(&backups, "2026-10-31-14_30").is_some());
        assert!(find_by_id(&backups, "14_30").is_some());
        assert!(find_by_id(&backups, "test-server-2026-10-31-14_30.zip").is_some());
        assert!(find_by_id(&backups, "1999").is_none());
    }

    #[test]
    fn same_minute_backups_never_overwrite_each_other() {
        let (_tmp, layout, meta) = layout_meta();
        world_fixture(layout.root());

        let first = create(&layout, &meta).unwrap();
        let second = create(&layout, &meta).unwrap();
        assert_ne!(first, second, "same-minute backup must not clobber");
        assert!(first.is_file());
        assert!(second.is_file());
        assert_eq!(list(&layout.backups_dir()).len(), 2);
    }

    #[test]
    fn restore_refuses_escaping_entries() {
        let (_tmp, layout, meta) = layout_meta();

        // craft a zip with an escaping entry
        let evil = layout.root().join("evil.zip");
        let file = std::fs::File::create(&evil).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("../escaped.txt", options.clone()).unwrap();
        zip.write_all(b"pwned").unwrap();
        zip.finish().unwrap();

        let err = restore(&layout, &evil).unwrap_err();
        assert!(err.to_string().contains("escapes"));
        assert!(!layout.root().parent().unwrap().join("escaped.txt").exists());
        let _ = meta;
    }
}
