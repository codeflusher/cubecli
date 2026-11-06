//! Modlist document: the portable `mod:slug[@version]` text format shared by
//! `tools export-modlist`, `project export-modlist`, and
//! `project import-modlist`. Pure text handling — no sources, no network.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::error::{CoreError, Result};
use crate::model::AddonSourceId;
use crate::pipeline::parse_request;

pub const FORMAT_VERSION: &str = "1";

const NOT_FOUND_HEADER: &str = "# not found:";

/// One modlist entry: `mod:<slug>[@<version>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModlistEntry {
    pub slug: String,
    pub version: Option<String>,
}

/// An item that could not be resolved during export — preserved by `parse`
/// and re-rendered by `to_text` so the block survives round-trips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotFoundEntry {
    pub label: String,
    pub reason: String,
}

/// A full modlist document plus its informational header notes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModlistDocument {
    /// From the `# source:` header — advisory, drives import source choice.
    pub source: Option<AddonSourceId>,
    /// Rendered as header comments (`# excluded (client-only): …`).
    pub notes: Vec<String>,
    pub entries: Vec<ModlistEntry>,
    pub not_found: Vec<NotFoundEntry>,
}

impl ModlistDocument {
    /// Serialize. `generated` is the `# generated:` line payload (injected so
    /// tests stay deterministic); `None` omits the line.
    pub fn to_text(&self, generated: Option<&str>) -> String {
        let mut lines = vec![format!("# cubecli modlist v{FORMAT_VERSION}")];
        if let Some(source) = self.source {
            lines.push(format!("# source: {}", source.slug()));
        }
        if let Some(generated) = generated {
            lines.push(format!("# generated: {generated}"));
        }
        for note in &self.notes {
            lines.push(format!("# {note}"));
        }
        for entry in &self.entries {
            lines.push(entry_line(entry));
        }
        if !self.not_found.is_empty() {
            lines.push(String::new());
            lines.push(NOT_FOUND_HEADER.to_string());
            for nf in &self.not_found {
                lines.push(format!("# - {}: {}", nf.label, nf.reason));
            }
        }
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }
}

/// The `mod:sodium@0.6.5` line for an entry.
pub fn entry_line(entry: &ModlistEntry) -> String {
    match &entry.version {
        Some(version) => format!("mod:{}@{}", entry.slug, version),
        None => format!("mod:{}", entry.slug),
    }
}

/// Parse a modlist document. Comments and blank lines are skipped; anything
/// that is not a comment and not a `mod:` entry is a hard error naming the line.
pub fn parse(text: &str) -> Result<ModlistDocument> {
    let mut doc = ModlistDocument::default();
    let mut in_not_found = false;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let rest = rest.trim();
            if rest == "not found:" {
                in_not_found = true;
            } else if let Some(value) = rest.strip_prefix("- ") {
                if in_not_found {
                    let (label, reason) = value.split_once(':').unwrap_or((value, ""));
                    doc.not_found.push(NotFoundEntry {
                        label: label.trim().to_string(),
                        reason: reason.trim().to_string(),
                    });
                }
            } else if let Some(value) = rest.strip_prefix("source:") {
                // Advisory: unknown values are ignored, not errors.
                doc.source = AddonSourceId::parse(value.trim());
            }
            continue;
        }
        in_not_found = false;
        let request = parse_request(line).map_err(|_| {
            CoreError::invalid_config(
                "modlist",
                format!(
                    "line {}: not a modlist entry (expected `mod:<slug>[@<version>]`)",
                    index + 1
                ),
            )
        })?;
        if request.slug.is_empty() {
            return Err(CoreError::invalid_config(
                "modlist",
                format!("line {}: empty slug", index + 1),
            ));
        }
        if let Some(version) = &request.version {
            if version.is_empty() {
                return Err(CoreError::invalid_config(
                    "modlist",
                    format!("line {}: empty version pin", index + 1),
                ));
            }
        }
        if request.kind != crate::model::AddonKind::Mod {
            return Err(CoreError::invalid_config(
                "modlist",
                format!(
                    "line {}: only `mod:` entries are supported (got `{}`)",
                    index + 1,
                    request.kind.slug()
                ),
            ));
        }
        if request
            .version
            .as_deref()
            .is_some_and(|v| v.trim().is_empty())
        {
            return Err(CoreError::invalid_config(
                "modlist",
                format!("line {}: empty version after `@`", index + 1),
            ));
        }
        doc.entries.push(ModlistEntry {
            slug: request.slug,
            version: request.version,
        });
    }
    Ok(doc)
}

/// Base64-encode modlist text (`--base64`).
pub fn encode_base64(text: &str) -> String {
    BASE64.encode(text.as_bytes())
}

/// Decode `--base64` input back into modlist text. Surrounding whitespace
/// (shell paste artifacts, trailing newline in files) is tolerated.
pub fn decode_base64(input: &str) -> Result<String> {
    let trimmed = input.trim();
    let bytes = BASE64
        .decode(trimmed)
        .map_err(|e| CoreError::invalid_config("modlist", format!("invalid base64: {e}")))?;
    String::from_utf8(bytes)
        .map_err(|e| CoreError::invalid_config("modlist", format!("base64 is not UTF-8: {e}")))
}

/// Enumerate mod jars for `tools export-modlist`: `*.jar` in `root` itself
/// plus `root/mods/`, non-recursive, sorted for deterministic output.
pub fn scan_jars(root: &Path) -> Vec<PathBuf> {
    let mut jars: Vec<PathBuf> = Vec::new();
    for dir in [root.to_path_buf(), root.join("mods")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // missing dir (e.g. no mods/) is fine
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
            {
                jars.push(path);
            }
        }
    }
    jars.sort();
    jars.dedup();
    jars
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# cubecli modlist v1
# source: modrinth
# generated: 2026-11-04T10:00:00+00:00
# excluded (client-only): optifine
mod:fabric-api
mod:sodium@0.6.5

# not found:
# - mods/jei-legacy.jar: not recognized on modrinth
";

    #[test]
    fn roundtrip_preserves_entries_source_and_not_found() {
        let doc = parse(SAMPLE).unwrap();
        assert_eq!(doc.source, Some(AddonSourceId::Modrinth));
        assert_eq!(
            doc.entries,
            vec![
                ModlistEntry {
                    slug: "fabric-api".into(),
                    version: None
                },
                ModlistEntry {
                    slug: "sodium".into(),
                    version: Some("0.6.5".into())
                },
            ]
        );
        assert_eq!(doc.not_found.len(), 1);
        assert_eq!(doc.not_found[0].label, "mods/jei-legacy.jar");
        assert_eq!(doc.not_found[0].reason, "not recognized on modrinth");

        let rebuilt = parse(&doc.to_text(Some("2026-11-04T10:00:00+00:00"))).unwrap();
        assert_eq!(rebuilt.entries, doc.entries);
        assert_eq!(rebuilt.source, doc.source);
        assert_eq!(rebuilt.not_found, doc.not_found);
    }

    #[test]
    fn text_layout_matches_the_documented_format() {
        let doc = ModlistDocument {
            source: Some(AddonSourceId::CurseForge),
            notes: vec!["excluded (client-only): optifine".into()],
            entries: vec![ModlistEntry {
                slug: "sodium".into(),
                version: Some("0.6.5".into()),
            }],
            not_found: vec![NotFoundEntry {
                label: "x.jar".into(),
                reason: "not recognized on curseforge".into(),
            }],
        };
        let text = doc.to_text(Some("now"));
        assert!(text.starts_with("# cubecli modlist v1\n"));
        assert!(text.contains("# source: curseforge\n"));
        assert!(text.contains("# generated: now\n"));
        assert!(text.contains("# excluded (client-only): optifine\n"));
        assert!(text.contains("mod:sodium@0.6.5\n"));
        assert!(text.contains("# not found:\n# - x.jar: not recognized on curseforge\n"));
    }

    #[test]
    fn parse_skips_comments_and_blank_lines() {
        let doc = parse("# just a comment\n\nmod:xe\n# - not part of a block\n").unwrap();
        assert_eq!(doc.entries.len(), 1);
        assert!(doc.not_found.is_empty());
        assert_eq!(doc.source, None);
    }

    #[test]
    fn parse_rejects_non_mod_lines_with_line_number() {
        let err = parse("mod:ok\nplugin:essentialsx\n").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("line 2"), "{message}");
        assert!(message.contains("plugin"), "{message}");

        let err = parse("not-an-entry\n").unwrap_err();
        assert!(err.to_string().contains("line 1"));
    }

    #[test]
    fn parse_rejects_empty_slug_or_version() {
        assert!(parse("mod:\n").is_err());
        assert!(parse("mod:sodium@\n").is_err());
    }

    #[test]
    fn entry_line_with_and_without_pin() {
        assert_eq!(
            entry_line(&ModlistEntry {
                slug: "sodium".into(),
                version: Some("0.6.5".into())
            }),
            "mod:sodium@0.6.5"
        );
        assert_eq!(
            entry_line(&ModlistEntry {
                slug: "xe".into(),
                version: None
            }),
            "mod:xe"
        );
    }

    #[test]
    fn base64_roundtrip_and_invalid_input_fails() {
        let text = "# cubecli modlist\nmod:sodium@0.6.5\n";
        let encoded = encode_base64(text);
        assert!(!encoded.contains('\n'));
        assert_eq!(decode_base64(&encoded).unwrap(), text);
        // surrounding whitespace (pasted from a shell) is tolerated
        assert_eq!(decode_base64(&format!("  {encoded}\n")).unwrap(), text);
        assert!(decode_base64("!!!not-base64!!!").is_err());
    }

    #[test]
    fn scan_jars_enumerates_root_and_mods_sorted_and_deduped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("mods")).unwrap();
        std::fs::create_dir(root.join("mods/nested")).unwrap();
        for rel in ["b.jar", "mods/a.jar", "notes.txt", "mods/nested/deep.jar"] {
            std::fs::write(root.join(rel), b"x").unwrap();
        }
        // uppercase extension must be picked up too
        std::fs::write(root.join("C.JAR"), b"x").unwrap();

        let jars = scan_jars(root);
        let names: Vec<String> = jars
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().display().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["C.JAR", "b.jar", "mods/a.jar"], // sorted, no nested, no .txt
            "unexpected scan order: {names:?}"
        );
    }

    #[test]
    fn scan_jars_missing_mods_dir_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("lonely.jar"), b"x").unwrap();
        assert_eq!(scan_jars(tmp.path()).len(), 1);
        assert!(scan_jars(&tmp.path().join("empty")).is_empty());
    }
}
