//! Modlist export/import pipelines against in-memory mock sources — no
//! network. Mirrors the mock style of `pipeline.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use url::Url;

use cubecli_core::error::CoreError;
use cubecli_core::meta::ProjectMeta;
use cubecli_core::model::{AddonKind, AddonSourceId, BuildChannel, CoreKind, InstalledAddon};
use cubecli_core::paths::ProjectLayout;
use cubecli_core::pipeline::modlist::{
    SourceLookup, export_from_project, export_from_scan, import_modlist,
};
use cubecli_core::project::ProjectHandle;
use cubecli_core::traits::addon_source::{
    AddonFileEntry, AddonProject, AddonQuery, AddonSearchHit, AddonSource, DownloadedAddon,
    IdentifiedFile, ServerSide,
};

// ------------------------------------------------------------------------------------------------
// mocks

/// What `identify_file` says about a jar (keyed by file name).
#[derive(Clone)]
enum Identify {
    /// Hash not in the source's database.
    Unknown,
    /// Lookup failed (network, auth…) — must not abort the export.
    Error(&'static str),
    Mod {
        slug: &'static str,
        version: &'static str,
        server_side: Option<ServerSide>,
        kind: Option<AddonKind>,
    },
}

struct MockSource {
    id: AddonSourceId,
    identify: HashMap<String, Identify>,
    /// slug → declared server-side support (`None` = keep the map miss as "no metadata").
    server_side: HashMap<String, ServerSide>,
    /// Versions `files()` offers (default: a single stable `1.2.3`).
    versions: Vec<String>,
    /// Slugs `required_dependencies` reports for any file.
    deps: Vec<String>,
}

impl MockSource {
    fn new(id: AddonSourceId) -> Self {
        Self {
            id,
            identify: HashMap::new(),
            server_side: HashMap::new(),
            versions: vec!["1.2.3".into()],
            deps: Vec::new(),
        }
    }

    fn with_identify(mut self, file: &str, outcome: Identify) -> Self {
        self.identify.insert(file.into(), outcome);
        self
    }

    fn with_server_side(mut self, slug: &str, side: ServerSide) -> Self {
        self.server_side.insert(slug.into(), side);
        self
    }

    fn with_deps(mut self, slugs: &[&str]) -> Self {
        self.deps = slugs.iter().map(|s| s.to_string()).collect();
        self
    }
}

fn mod_identified(slug: &str, version: &str) -> Identify {
    Identify::Mod {
        slug: leaky(slug),
        version: leaky(version),
        server_side: None,
        kind: Some(AddonKind::Mod),
    }
}

/// Test maps need 'static keys/outcomes; the strings are process-lifetime
/// constants in practice — leak is fine for a test binary.
fn leaky(s: &str) -> &'static str {
    String::leak(s.to_string())
}

#[async_trait]
impl AddonSource for MockSource {
    fn id(&self) -> AddonSourceId {
        self.id
    }

    async fn resolve(&self, slug: &str) -> cubecli_core::Result<AddonProject> {
        if slug == "ghost" {
            return Err(CoreError::not_found("mock", format!("slug `{slug}`")));
        }
        Ok(AddonProject {
            slug: slug.into(),
            provider_id: format!("id-{slug}"),
            title: slug.into(),
            description: String::new(),
        })
    }

    async fn search(
        &self,
        query: &str,
        _kind: AddonKind,
    ) -> cubecli_core::Result<Vec<AddonSearchHit>> {
        Ok(vec![AddonSearchHit {
            slug: query.into(),
            title: query.into(),
            description: String::new(),
            author: String::new(),
            downloads: 1,
        }])
    }

    async fn files(
        &self,
        _project: &AddonProject,
        _query: &AddonQuery,
    ) -> cubecli_core::Result<Vec<AddonFileEntry>> {
        Ok(self
            .versions
            .iter()
            .map(|version| AddonFileEntry {
                version: version.clone(),
                file_name: format!("{}-{}.jar", _project.slug, version),
                url: Url::parse("http://mock/mod.jar").unwrap(),
                checksum: None,
                released_at: None,
                channel: Some(BuildChannel::Stable),
                provider_ref: String::new(),
            })
            .collect())
    }

    async fn latest(
        &self,
        project: &AddonProject,
        query: &AddonQuery,
    ) -> cubecli_core::Result<Option<AddonFileEntry>> {
        Ok(self.files(project, query).await?.into_iter().next())
    }

    async fn download(
        &self,
        file: &AddonFileEntry,
        dest_dir: &Path,
    ) -> cubecli_core::Result<DownloadedAddon> {
        let path = dest_dir.join(&file.file_name);
        std::fs::write(&path, b"mod bytes").unwrap();
        Ok(DownloadedAddon {
            path,
            file_name: file.file_name.clone(),
        })
    }

    async fn required_dependencies(
        &self,
        _project: &AddonProject,
        _file: &AddonFileEntry,
        _query: &AddonQuery,
    ) -> cubecli_core::Result<Vec<AddonProject>> {
        Ok(self
            .deps
            .iter()
            .map(|slug| AddonProject {
                slug: slug.clone(),
                provider_id: format!("id-{slug}"),
                title: slug.clone(),
                description: String::new(),
            })
            .collect())
    }

    async fn identify_file(&self, path: &Path) -> cubecli_core::Result<Option<IdentifiedFile>> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        match self.identify.get(name) {
            None => Ok(None),
            Some(Identify::Unknown) => Ok(None),
            Some(Identify::Error(message)) => {
                Err(CoreError::external("mock", (*message).to_string()))
            }
            Some(Identify::Mod {
                slug,
                version,
                server_side,
                kind,
            }) => Ok(Some(IdentifiedFile {
                project: AddonProject {
                    slug: (*slug).into(),
                    provider_id: format!("id-{slug}"),
                    title: (*slug).into(),
                    description: String::new(),
                },
                version: (*version).into(),
                server_side: *server_side,
                kind: *kind,
            })),
        }
    }

    async fn server_side(
        &self,
        project: &AddonProject,
    ) -> cubecli_core::Result<Option<ServerSide>> {
        Ok(self.server_side.get(&project.slug).copied())
    }
}

struct MockLookup {
    sources: HashMap<AddonSourceId, Arc<dyn AddonSource>>,
}

impl SourceLookup for MockLookup {
    fn lookup(&self, id: AddonSourceId) -> cubecli_core::Result<Arc<dyn AddonSource>> {
        self.sources
            .get(&id)
            .cloned()
            .ok_or_else(|| CoreError::not_found("mock", format!("source {id}")))
    }
}

// ------------------------------------------------------------------------------------------------
// helpers

fn write_jar(root: &Path, relative: &str) -> PathBuf {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"jar bytes").unwrap();
    path
}

fn installed_mod(slug: &str, source: AddonSourceId) -> InstalledAddon {
    InstalledAddon {
        slug: slug.into(),
        source_id: source,
        provider_id: format!("id-{slug}"),
        kind: AddonKind::Mod,
        version: "1.0.0".into(),
        file: format!("{slug}.jar"),
        dir: "mods".into(),
        sha1: None,
        installed_at: Utc::now(),
        deps: vec![],
    }
}

fn project_with(
    core: CoreKind,
    installed: Vec<InstalledAddon>,
) -> (tempfile::TempDir, ProjectHandle) {
    let tmp = tempfile::tempdir().unwrap();
    let layout = ProjectLayout::new(tmp.path());
    let mut meta = ProjectMeta::default();
    meta.server.core = core;
    meta.server.mc_version = "1.21.8".into();
    meta.addons.installed = installed;
    meta.save(layout.meta_file()).unwrap();
    let handle = ProjectHandle::open(layout).unwrap();
    (tmp, handle)
}

fn no_progress(_index: usize, _total: usize, _jar: &Path) {}

// ------------------------------------------------------------------------------------------------
// scan export (tools export-modlist)

#[tokio::test]
async fn scan_export_recognizes_unrecognized_and_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    write_jar(tmp.path(), "a-sodium.jar");
    write_jar(tmp.path(), "mods/z-sodium-copy.jar");
    write_jar(tmp.path(), "mystery.jar");
    write_jar(tmp.path(), "resource-pack.jar");
    write_jar(tmp.path(), "broken.jar");

    let source = MockSource::new(AddonSourceId::Modrinth)
        .with_identify("a-sodium.jar", mod_identified("sodium", "0.6.5"))
        .with_identify("z-sodium-copy.jar", mod_identified("sodium", "0.6.5"))
        .with_identify(
            "resource-pack.jar",
            Identify::Mod {
                slug: "fancy-pack",
                version: "2.0",
                server_side: None,
                kind: Some(AddonKind::ServerResourcePack),
            },
        )
        .with_identify("broken.jar", Identify::Error("kaboom"));

    let report = export_from_scan(tmp.path(), &source, false, &no_progress)
        .await
        .unwrap();

    assert_eq!(
        report.entries,
        vec![cubecli_core::modlist::ModlistEntry {
            slug: "sodium".into(),
            version: Some("0.6.5".into())
        }]
    );
    assert!(report.excluded.is_empty());

    let by_label: HashMap<&str, &str> = report
        .not_found
        .iter()
        .map(|nf| (nf.label.as_str(), nf.reason.as_str()))
        .collect();
    assert_eq!(by_label["mystery.jar"], "not recognized on modrinth");
    assert!(by_label["resource-pack.jar"].contains("is a server-resource-pack"));
    assert!(by_label["mods/z-sodium-copy.jar"].contains("duplicate of `sodium`"));
    assert!(by_label["broken.jar"].contains("kaboom"));
    assert_eq!(report.not_found.len(), 4);
}

#[tokio::test]
async fn scan_export_server_only_drops_client_only() {
    let tmp = tempfile::tempdir().unwrap();
    write_jar(tmp.path(), "client.jar");
    write_jar(tmp.path(), "server.jar");
    write_jar(tmp.path(), "unknown-side.jar");

    let source = MockSource::new(AddonSourceId::Modrinth)
        .with_identify(
            "client.jar",
            Identify::Mod {
                slug: "optifine",
                version: "1.0",
                server_side: Some(ServerSide::Unsupported),
                kind: Some(AddonKind::Mod),
            },
        )
        .with_identify(
            "server.jar",
            Identify::Mod {
                slug: "lithium",
                version: "0.14",
                server_side: Some(ServerSide::Required),
                kind: Some(AddonKind::Mod),
            },
        )
        .with_identify("unknown-side.jar", mod_identified("xaero", "1.0"));

    let kept = export_from_scan(tmp.path(), &source, false, &no_progress)
        .await
        .unwrap();
    assert_eq!(kept.entries.len(), 3); // unknown metadata never filters

    let filtered = export_from_scan(tmp.path(), &source, true, &no_progress)
        .await
        .unwrap();
    assert_eq!(
        filtered
            .entries
            .iter()
            .map(|e| e.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["lithium", "xaero"]
    );
    assert_eq!(
        filtered.excluded,
        vec![("optifine".into(), ServerSide::Unsupported)]
    );
}

#[tokio::test]
async fn scan_export_no_jars_is_not_found_error() {
    let tmp = tempfile::tempdir().unwrap();
    let err = export_from_scan(
        tmp.path(),
        &MockSource::new(AddonSourceId::Modrinth),
        false,
        &no_progress,
    )
    .await
    .err()
    .expect("scan of an empty dir must fail");
    assert!(matches!(err, CoreError::NotFound { .. }), "{err}");
    assert!(err.to_string().contains("no .jar files"));
}

#[tokio::test]
async fn scan_export_counts_progress_for_every_jar() {
    let tmp = tempfile::tempdir().unwrap();
    write_jar(tmp.path(), "one.jar");
    write_jar(tmp.path(), "two.jar");
    let seen = std::sync::Mutex::new(Vec::new());
    let on_progress = |index: usize, total: usize, jar: &Path| {
        seen.lock().unwrap().push((
            index,
            total,
            jar.file_name().unwrap().to_str().unwrap().to_string(),
        ));
    };
    export_from_scan(
        tmp.path(),
        &MockSource::new(AddonSourceId::Modrinth),
        false,
        &on_progress,
    )
    .await
    .unwrap();
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[(0, 2, "one.jar".to_string()), (1, 2, "two.jar".to_string())]
    );
}

// ------------------------------------------------------------------------------------------------
// project export (project export-modlist)

fn lookup_with(sources: Vec<Arc<dyn AddonSource>>) -> MockLookup {
    let mut map = HashMap::new();
    for source in sources {
        map.insert(source.id(), source as Arc<dyn AddonSource>);
    }
    MockLookup { sources: map }
}

#[tokio::test]
async fn project_export_mods_only_sorted_with_distinct_sources() {
    let installed = vec![
        installed_mod("zmod", AddonSourceId::Modrinth),
        installed_mod("amod", AddonSourceId::CurseForge),
        InstalledAddon {
            slug: "essentialsx".into(),
            source_id: AddonSourceId::Modrinth,
            provider_id: "id-p".into(),
            kind: AddonKind::Plugin, // must not leak into a modlist
            version: "2.0".into(),
            file: "e.jar".into(),
            dir: "plugins".into(),
            sha1: None,
            installed_at: Utc::now(),
            deps: vec![],
        },
    ];
    let (_tmp, handle) = project_with(CoreKind::Fabric, installed);

    let export = export_from_project(handle.meta(), &lookup_with(vec![]), false, None)
        .await
        .unwrap();
    assert_eq!(
        export
            .entries
            .iter()
            .map(|e| e.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["amod", "zmod"]
    );
    assert_eq!(
        export.distinct_sources,
        vec![AddonSourceId::Modrinth, AddonSourceId::CurseForge]
    );
    assert!(export.excluded.is_empty());
    assert!(export.metadata_unknown.is_empty());
}

#[tokio::test]
async fn project_export_server_only_uses_record_source_and_override() {
    let installed = vec![
        installed_mod("keep", AddonSourceId::Modrinth),
        installed_mod("drop", AddonSourceId::Modrinth),
        installed_mod("blind", AddonSourceId::CurseForge),
    ];
    let (_tmp, handle) = project_with(CoreKind::Fabric, installed);

    let mut modrinth = MockSource::new(AddonSourceId::Modrinth);
    modrinth
        .server_side
        .insert("keep".into(), ServerSide::Required);
    modrinth
        .server_side
        .insert("drop".into(), ServerSide::Unsupported);
    let curseforge = MockSource::new(AddonSourceId::CurseForge); // no metadata → kept
    let lookup = lookup_with(vec![Arc::new(modrinth), Arc::new(curseforge)]);

    // Default: each record checks against its own source.
    let export = export_from_project(handle.meta(), &lookup, true, None)
        .await
        .unwrap();
    assert_eq!(
        export
            .entries
            .iter()
            .map(|e| e.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["blind", "keep"]
    );
    assert_eq!(
        export.excluded,
        vec![("drop".into(), ServerSide::Unsupported)]
    );
    assert_eq!(export.metadata_unknown, vec!["blind".to_string()]);

    // Override: every record is checked against curseforge (no metadata there).
    let export = export_from_project(
        handle.meta(),
        &lookup,
        true,
        Some(AddonSourceId::CurseForge),
    )
    .await
    .unwrap();
    assert_eq!(export.entries.len(), 3);
    assert_eq!(export.metadata_unknown.len(), 3);
    assert!(export.excluded.is_empty());
}

// ------------------------------------------------------------------------------------------------
// import (project import-modlist)

fn auto_accept(_dep: &AddonProject) -> bool {
    true
}

#[tokio::test]
async fn import_installs_pin_falls_back_to_latest_with_warning() {
    let (_tmp, mut handle) = project_with(CoreKind::Fabric, vec![]);
    let source = MockSource::new(AddonSourceId::Modrinth); // only offers 1.2.3

    let entries = vec![
        cubecli_core::modlist::ModlistEntry {
            slug: "sodium".into(),
            version: Some("9.9.9".into()), // pin the mock can't satisfy
        },
        cubecli_core::modlist::ModlistEntry {
            slug: "lithium".into(),
            version: Some("1.2.3".into()), // exact hit
        },
    ];
    let report = import_modlist(&mut handle, &source, &entries, &auto_accept).await;

    assert!(report.not_found.is_empty(), "{:?}", report.not_found);
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("9.9.9"));
    assert!(report.warnings[0].contains("1.2.3"));
    let installed_slugs: Vec<&str> = report.installed.iter().map(|r| r.slug.as_str()).collect();
    assert_eq!(installed_slugs, vec!["sodium", "lithium"]);
    assert!(
        handle
            .meta()
            .addons
            .installed
            .iter()
            .all(|r| r.version == "1.2.3")
    );
}

#[tokio::test]
async fn import_collects_not_found_and_keeps_going() {
    let (_tmp, mut handle) = project_with(CoreKind::Fabric, vec![]);
    let source = MockSource::new(AddonSourceId::Modrinth);

    let entries = vec![
        cubecli_core::modlist::ModlistEntry {
            slug: "ghost".into(),
            version: None,
        },
        cubecli_core::modlist::ModlistEntry {
            slug: "sodium".into(),
            version: Some("1.2.3".into()),
        },
    ];
    let report = import_modlist(&mut handle, &source, &entries, &auto_accept).await;

    assert_eq!(
        report.not_found,
        vec![(
            "ghost".to_string(),
            "mock: not found: slug `ghost`".to_string()
        )]
    );
    assert_eq!(report.installed.len(), 1);
    assert_eq!(report.installed[0].slug, "sodium");
}

#[tokio::test]
async fn import_skips_already_installed() {
    let (_tmp, mut handle) = project_with(CoreKind::Fabric, vec![]);
    let source = MockSource::new(AddonSourceId::Modrinth);

    let first = import_modlist(
        &mut handle,
        &source,
        &[cubecli_core::modlist::ModlistEntry {
            slug: "sodium".into(),
            version: Some("1.2.3".into()),
        }],
        &auto_accept,
    )
    .await;
    assert_eq!(first.installed.len(), 1);

    let again = import_modlist(
        &mut handle,
        &source,
        &[
            cubecli_core::modlist::ModlistEntry {
                slug: "sodium".into(),
                version: Some("1.2.3".into()), // same pin → skip
            },
            cubecli_core::modlist::ModlistEntry {
                slug: "sodium".into(),
                version: None, // unpinned + installed → skip
            },
        ],
        &auto_accept,
    )
    .await;
    assert!(again.installed.is_empty());
    assert_eq!(
        again.skipped,
        vec![
            ("sodium".to_string(), "already installed".to_string()),
            ("sodium".to_string(), "already installed".to_string())
        ]
    );
}

#[tokio::test]
async fn import_auto_accepts_required_dependencies() {
    let (_tmp, mut handle) = project_with(CoreKind::Fabric, vec![]);
    let source = MockSource::new(AddonSourceId::Modrinth).with_deps(&["fabric-api"]);

    let report = import_modlist(
        &mut handle,
        &source,
        &[cubecli_core::modlist::ModlistEntry {
            slug: "sodium".into(),
            version: Some("1.2.3".into()),
        }],
        &auto_accept,
    )
    .await;

    let slugs: Vec<&str> = report.installed.iter().map(|r| r.slug.as_str()).collect();
    // install_addon returns the requested addon first, then dependency records.
    assert_eq!(slugs, vec!["sodium", "fabric-api"]);
    assert_eq!(handle.meta().addons.installed.len(), 2);
}

#[tokio::test]
async fn import_reports_incompatible_core_offline() {
    let (_tmp, mut handle) = project_with(CoreKind::Paper, vec![]); // paper loads no mods
    let source = MockSource::new(AddonSourceId::Modrinth);

    let report = import_modlist(
        &mut handle,
        &source,
        &[cubecli_core::modlist::ModlistEntry {
            slug: "sodium".into(),
            version: Some("1.2.3".into()),
        }],
        &auto_accept,
    )
    .await;

    assert!(report.installed.is_empty());
    assert_eq!(report.not_found.len(), 1);
    assert!(report.not_found[0].1.contains("not supported"));
}
