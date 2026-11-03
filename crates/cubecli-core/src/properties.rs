//! Targeted `server.properties` editor.
//!
//! cubecli only ever *edits lines* — comments, ordering, and unknown keys
//! pass through byte-for-byte, so user edits and future vanilla keys are
//! never clobbered by a whole-file rewrite.

use std::path::Path;

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, PartialEq)]
enum Line {
    Raw(String),
    Entry {
        key: String,
        sep: String,
        value: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOutcome {
    Updated,
    Appended,
    Unchanged,
}

impl SetOutcome {
    pub fn changed(self) -> bool {
        !matches!(self, Self::Unchanged)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropertiesFile {
    lines: Vec<Line>,
}

impl PropertiesFile {
    pub fn parse(text: &str) -> Self {
        let mut lines = Vec::new();
        for raw in text.lines() {
            lines.push(Self::parse_line(raw));
        }
        Self { lines }
    }

    /// Read a properties file; a missing file is treated as empty
    /// (provisioning creates it fresh).
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match line {
                Line::Raw(raw) => out.push_str(raw),
                Line::Entry { key, sep, value } => {
                    out.push_str(key);
                    out.push_str(sep);
                    out.push_str(value);
                }
            }
            out.push('\n');
        }
        out
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        std::fs::write(path, self.render()).map_err(|e| CoreError::io(path, e))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().find_map(|l| match l {
            Line::Entry { key: k, value, .. } if k == key => Some(value.as_str()),
            _ => None,
        })
    }

    /// Set one key. Existing entries are edited in place (keeping their
    /// original separator); missing keys are appended.
    pub fn set(&mut self, key: &str, value: &str) -> SetOutcome {
        for line in &mut self.lines {
            if let Line::Entry {
                key: k, value: v, ..
            } = line
            {
                if k == key {
                    if v == value {
                        return SetOutcome::Unchanged;
                    }
                    *v = value.to_string();
                    return SetOutcome::Updated;
                }
            }
        }
        self.lines.push(Line::Entry {
            key: key.to_string(),
            sep: "=".to_string(),
            value: value.to_string(),
        });
        SetOutcome::Appended
    }

    /// Apply several managed keys; returns true if anything changed.
    pub fn apply(&mut self, entries: &[(&str, &str)]) -> bool {
        let mut changed = false;
        for (key, value) in entries {
            changed |= self.set(key, value).changed();
        }
        changed
    }

    fn parse_line(raw: &str) -> Line {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('#') {
            return Line::Raw(raw.to_string());
        }
        // Java properties accept both `=` and `:` separators.
        let eq = raw.find('=');
        let colon = raw.find(':');
        let split = match (eq, colon) {
            (Some(e), Some(c)) => Some(e.min(c)),
            (Some(e), None) => Some(e),
            (None, Some(c)) => Some(c),
            (None, None) => None,
        };
        match split {
            Some(idx) => {
                let key = raw[..idx].trim().to_string();
                if key.is_empty() {
                    return Line::Raw(raw.to_string());
                }
                // Preserve the exact separator including whitespace after it
                // (e.g. `=` vs `: `) so rendering is byte-faithful.
                let bytes = raw.as_bytes();
                let mut value_start = idx + 1;
                while value_start < bytes.len()
                    && (bytes[value_start] == b' ' || bytes[value_start] == b'\t')
                {
                    value_start += 1;
                }
                let sep = raw[idx..value_start].to_string();
                let value = raw[value_start..].trim().to_string();
                Line::Entry { key, sep, value }
            }
            None => Line::Raw(raw.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#Minecraft server properties\n#Edited by hand\nmotd=A Server\nmax-players: 20\nthis-line-is-weird\n\n";

    #[test]
    fn parses_entries_with_both_separators() {
        let p = PropertiesFile::parse(SAMPLE);
        assert_eq!(p.get("motd"), Some("A Server"));
        assert_eq!(p.get("max-players"), Some("20"));
        assert_eq!(p.get("missing"), None);
    }

    #[test]
    fn set_updates_in_place_and_preserves_comments() {
        let mut p = PropertiesFile::parse(SAMPLE);
        p.set("motd", "New Motd");
        let out = p.render();
        assert!(out.contains("#Minecraft server properties"));
        assert!(out.contains("#Edited by hand"));
        assert!(out.contains("motd=New Motd"));
        // separator of untouched entry preserved
        assert!(out.contains("max-players: 20"));
        assert!(out.contains("this-line-is-weird"));
    }

    #[test]
    fn set_appends_missing() {
        let mut p = PropertiesFile::parse(SAMPLE);
        assert_eq!(p.set("server-port", "25565"), SetOutcome::Appended);
        assert_eq!(p.get("server-port"), Some("25565"));
        assert!(p.render().ends_with("server-port=25565\n"));
    }

    #[test]
    fn set_is_idempotent() {
        let mut p = PropertiesFile::parse(SAMPLE);
        assert_eq!(p.set("motd", "X"), SetOutcome::Updated);
        assert_eq!(p.set("motd", "X"), SetOutcome::Unchanged);
        assert!(p.apply(&[("motd", "X"), ("pvp", "true")])); // pvp appended
        assert!(!p.apply(&[("motd", "X"), ("pvp", "true")])); // second time: no change
        assert_eq!(p.render(), PropertiesFile::parse(&p.render()).render());
    }

    #[test]
    fn render_is_byte_faithful() {
        assert_eq!(PropertiesFile::parse(SAMPLE).render(), SAMPLE);
    }

    #[test]
    fn empty_values_and_colon_values() {
        let p = PropertiesFile::parse("a=\nb:has colon\n");
        assert_eq!(p.get("a"), Some(""));
        assert_eq!(p.get("b"), Some("has colon"));
    }

    #[test]
    fn missing_file_reads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = PropertiesFile::read(tmp.path().join("server.properties")).unwrap();
        assert_eq!(p, PropertiesFile::default());
        let mut p = p;
        p.set("eula", "true");
        p.write(tmp.path().join("server.properties")).unwrap();
        assert_eq!(
            PropertiesFile::read(tmp.path().join("server.properties"))
                .unwrap()
                .get("eula"),
            Some("true")
        );
    }
}
