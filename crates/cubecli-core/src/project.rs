//! A discovered cubecli project: its layout plus loaded metadata.

use std::path::Path;

use crate::error::{CoreError, Result};
use crate::meta::ProjectMeta;
use crate::paths::{META_FILE, ProjectLayout};

#[derive(Debug, Clone)]
pub struct ProjectHandle {
    layout: ProjectLayout,
    meta: ProjectMeta,
}

impl ProjectHandle {
    /// Find the project containing `start` by walking up parents until a
    /// `.cubecli.meta` appears. The error lists every directory searched.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let start = start.as_ref();
        let absolute = if start.is_absolute() {
            start.to_path_buf()
        } else {
            let cwd = std::env::current_dir().map_err(|e| CoreError::io(".", e))?;
            cwd.join(start)
        };

        let mut dir: &Path = &absolute;
        let mut searched = Vec::new();
        loop {
            searched.push(dir.to_path_buf());
            if dir.join(META_FILE).is_file() {
                return Self::open(ProjectLayout::new(dir));
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }
        Err(CoreError::NotAProject { searched })
    }

    /// Open a project whose root is known. Errors when the meta file is missing.
    pub fn open(layout: ProjectLayout) -> Result<Self> {
        let meta = ProjectMeta::load(layout.meta_file())?;
        Ok(Self { layout, meta })
    }

    pub fn layout(&self) -> &ProjectLayout {
        &self.layout
    }

    pub fn root(&self) -> &Path {
        self.layout.root()
    }

    pub fn meta(&self) -> &ProjectMeta {
        &self.meta
    }

    pub fn meta_mut(&mut self) -> &mut ProjectMeta {
        &mut self.meta
    }

    pub fn save(&self) -> Result<()> {
        self.meta.save(self.layout.meta_file())
    }

    pub fn reload(&mut self) -> Result<()> {
        self.meta = ProjectMeta::load(self.layout.meta_file())?;
        Ok(())
    }
}

impl AsRef<ProjectMeta> for ProjectHandle {
    fn as_ref(&self) -> &ProjectMeta {
        &self.meta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_from_nested_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("server");
        std::fs::create_dir_all(root.join("world/region")).unwrap();
        ProjectMeta::default().save(root.join(META_FILE)).unwrap();

        let found = ProjectHandle::discover(root.join("world/region")).unwrap();
        assert_eq!(found.root(), root.as_path());
        assert_eq!(found.meta().project.slug, "server");
    }

    #[test]
    fn not_a_project_lists_searched_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        let err = ProjectHandle::discover(&nested).unwrap_err();
        let CoreError::NotAProject { searched } = &err else {
            panic!("wrong error: {err}")
        };
        assert!(searched.contains(&nested));
        assert!(searched.contains(&tmp.path().join("a")));
        assert!(err.hint().unwrap().contains("create"));
    }

    #[test]
    fn open_requires_meta() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ProjectHandle::open(ProjectLayout::new(tmp.path())).is_err());
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(tmp.path());
        layout.ensure_state_dirs().unwrap();
        ProjectMeta::default().save(layout.meta_file()).unwrap();

        let mut handle = ProjectHandle::open(layout).unwrap();
        handle.meta_mut().runtime.pvp = false;
        handle.save().unwrap();
        handle.reload().unwrap();
        assert!(!handle.meta().runtime.pvp);
    }
}
