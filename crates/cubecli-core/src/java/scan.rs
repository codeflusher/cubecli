//! Filesystem scan for installed JDKs.
//!
//! Resolution order: cubecli-owned (`~/.cubecli/jdks/<feature>`) → sdkman
//! (`~/.sdkman/candidates/java/*`) → system PATH (warned as an unrecommended
//! fallback). This is sync filesystem/exec work — cheap and callable anywhere.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::model::JavaRequirement;
use crate::traits::java::{JavaInstall, JavaLocator, JavaOrigin};

pub const SDKMAN_JAVA_DIR: &str = ".sdkman/candidates/java";

pub struct FsJavaLocator {
    owned_root: PathBuf,
}

impl FsJavaLocator {
    pub fn new(owned_root: PathBuf) -> Self {
        Self { owned_root }
    }

    pub fn discover() -> Option<Self> {
        dirs::home_dir().map(|home| Self {
            owned_root: home.join(".cubecli/jdks"),
        })
    }

    fn scan_owned(&self) -> Vec<JavaInstall> {
        scan_numeric_dirs(&self.owned_root, JavaOrigin::Owned)
    }

    fn scan_sdkman(&self) -> Vec<JavaInstall> {
        let Some(home) = dirs::home_dir() else {
            return vec![];
        };
        let dir = home.join(SDKMAN_JAVA_DIR);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return vec![];
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                // "21.0.5-tem", "17.0.10.hs-adpt", "current" (skip)
                let feature = name
                    .split(['.', '-'])
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())?;
                let java_bin = detect_java_bin(&e.path())?;
                let home_dir = java_bin.parent()?.parent()?.to_path_buf();
                Some(JavaInstall {
                    feature,
                    origin: JavaOrigin::Sdkman,
                    home: home_dir,
                    java_bin,
                })
            })
            .collect()
    }

    fn scan_system(&self) -> Vec<JavaInstall> {
        let Some(which) = which_java() else {
            return vec![];
        };
        let Some(feature) = detect_feature(&which) else {
            return vec![];
        };
        let home = java_home(&which).unwrap_or_else(|| {
            which
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/"))
        });
        vec![JavaInstall {
            feature,
            origin: JavaOrigin::System,
            home,
            java_bin: which,
        }]
    }
}

impl JavaLocator for FsJavaLocator {
    fn locate(&self, requirement: JavaRequirement) -> Result<JavaInstall> {
        let mut candidates: Vec<JavaInstall> = self
            .all()
            .into_iter()
            .filter(|j| j.feature == requirement.major)
            .collect();
        candidates.sort_by_key(|j| match j.origin {
            JavaOrigin::Owned => 0,
            JavaOrigin::Sdkman => 1,
            JavaOrigin::System => 2,
        });
        candidates.into_iter().next().ok_or(CoreError::JavaMissing {
            feature: requirement.major,
        })
    }

    fn all(&self) -> Vec<JavaInstall> {
        let mut all = Vec::new();
        all.extend(self.scan_owned());
        all.extend(self.scan_sdkman());
        all.extend(self.scan_system());
        // dedupe by (feature, origin), first wins
        let mut seen = std::collections::HashSet::new();
        all.retain(|j| seen.insert((j.feature, j.origin)));
        all
    }
}

fn scan_numeric_dirs(root: &Path, origin: JavaOrigin) -> Vec<JavaInstall> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return vec![];
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let feature = e.file_name().to_string_lossy().parse::<u32>().ok()?;
            let java_bin = detect_java_bin(&e.path())?;
            let home = java_bin.parent()?.parent()?.to_path_buf();
            Some(JavaInstall {
                feature,
                origin,
                home,
                java_bin,
            })
        })
        .collect()
}

/// `dir` is a JAVA_HOME candidate: try the macOS layout (Contents/Home) first.
fn detect_java_bin(dir: &Path) -> Option<PathBuf> {
    let candidates = [dir.join("Contents/Home/bin/java"), dir.join("bin/java")];
    candidates.into_iter().find(|p| p.exists())
}

/// Locate `java` on PATH (portable-enough `which`).
fn which_java() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("java"))
        .find(|candidate| candidate.is_file())
}

/// `java -version` stderr: `openjdk version "21.0.5"` / `1.8.0_412`.
fn detect_feature(java_bin: &Path) -> Option<u32> {
    let output = std::process::Command::new(java_bin)
        .arg("-version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    let line = text.lines().find(|l| l.contains("version"))?;
    let start = line.find('"')? + 1;
    let rest = &line[start..];
    let end = rest.find('"')?;
    let version = &rest[..end];
    let mut parts = version.split('.');
    let first = parts.next()?;
    if first == "1" {
        parts.next()?.parse().ok()
    } else {
        first.parse().ok()
    }
}

/// `java -XshowSettings:properties` → `java.home` value (best-effort).
fn java_home(java_bin: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new(java_bin)
        .arg("-XshowSettings:properties")
        .arg("-version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    text.lines()
        .find_map(|l| l.trim().strip_prefix("java.home = "))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use std::sync::{Mutex, MutexGuard};

    /// Tests below mutate the process-wide PATH — serialize them.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn owned_scan_finds_mac_and_linux_layouts() {
        let tmp = tempfile::tempdir().unwrap();
        let mac = tmp.path().join("21/Contents/Home/bin");
        std::fs::create_dir_all(mac).unwrap();
        std::fs::write(tmp.path().join("21/Contents/Home/bin/java"), "").unwrap();
        let linux = tmp.path().join("17/bin");
        std::fs::create_dir_all(&linux).unwrap();
        std::fs::write(linux.join("java"), "").unwrap();

        let locator = FsJavaLocator::new(tmp.path().to_path_buf());
        let mut owned: Vec<u32> = locator
            .all()
            .into_iter()
            .filter(|j| j.origin == JavaOrigin::Owned)
            .map(|j| j.feature)
            .collect();
        owned.sort_unstable();
        assert_eq!(owned, vec![17, 21]);

        let jdk21 = locator.locate(JavaRequirement { major: 21 }).unwrap();
        assert_eq!(jdk21.origin, JavaOrigin::Owned);
        assert!(jdk21.home.ends_with("Contents/Home"));
        assert!(locator.locate(JavaRequirement { major: 99 }).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn detect_feature_prefers_owned_over_system() {
        let _guard: MutexGuard<'_, ()> = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("21/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("java"), "").unwrap();

        // fake "system" java on PATH that reports 21
        let fake_dir = tempfile::tempdir().unwrap();
        let fake = fake_dir.path().join("java");
        std::fs::write(&fake, "#!/bin/sh\necho 'openjdk version \"21.0.5\"' >&2\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let original = std::env::var("PATH").ok();
        unsafe { std::env::set_var("PATH", fake_dir.path()) };

        let locator = FsJavaLocator::new(tmp.path().to_path_buf());
        let found = locator.locate(JavaRequirement { major: 21 }).unwrap();
        assert_eq!(found.origin, JavaOrigin::Owned);

        match original {
            Some(p) => unsafe { std::env::set_var("PATH", p) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }
}
