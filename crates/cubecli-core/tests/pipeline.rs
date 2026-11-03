//! Provision + addon pipelines against in-memory mocks — no network, no
//! real processes beyond shell echo commands.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use url::Url;

use cubecli_core::model::{
    AddonKind, AddonSourceId, BuildChannel, Checksum, CoreKind, JavaRequirement, LaunchSpec,
    ModLoader,
};
use cubecli_core::paths::ProjectLayout;
use cubecli_core::pipeline::ProvisionOptions;
use cubecli_core::pipeline::addon_install::{
    AddonRequest, install_addon, parse_request, uninstall_addon,
};
use cubecli_core::pipeline::provision::provision;
use cubecli_core::project::ProjectHandle;
use cubecli_core::traits::addon_source::{
    AddonFileEntry, AddonProject, AddonQuery, AddonSearchHit, AddonSource, DownloadedAddon,
};
use cubecli_core::traits::java::{JavaInstall, JavaLocator, JavaOrigin, JavaProvisioner};
use cubecli_core::traits::progress::ProgressReporter;
use cubecli_core::traits::server_core::{DownloadSpec, InstallPlan, PostInstallStep};

// ------------------------------------------------------------------------------------------------
// mocks

struct MockFetcher {
    fail_first: AtomicUsize,
}

#[async_trait]
impl cubecli_core::pipeline::FileFetcher for MockFetcher {
    async fn fetch(
        &self,
        _url: &Url,
        checksum: Option<&Checksum>,
        dest: &Path,
        _progress: &dyn ProgressReporter,
    ) -> cubecli_core::Result<()> {
        // Fail at most once (swap avoids the wrap-around a plain fetch_sub
        // would hit after reaching zero).
        if self.fail_first.swap(0, Ordering::SeqCst) > 0 {
            return Err(cubecli_core::CoreError::External {
                service: "mock",
                message: "transient failure".into(),
            });
        }
        let content = b"downloaded jar";
        // prove checksum verification runs in the pipeline
        if let Some(expected) = checksum {
            use sha1::Digest as _;
            let mut h = sha1::Sha1::new();
            h.update(content);
            let hex = h
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            if !expected.matches(&hex) {
                return Err(cubecli_core::CoreError::ChecksumMismatch {
                    file: dest.display().to_string(),
                    algorithm: expected.algorithm(),
                    expected: expected.expected_hex().into(),
                    actual: hex,
                });
            }
        }
        std::fs::write(dest, content).unwrap();
        Ok(())
    }
}

struct AlwaysJava;

impl JavaLocator for AlwaysJava {
    fn locate(&self, req: JavaRequirement) -> cubecli_core::Result<JavaInstall> {
        Ok(JavaInstall {
            feature: req.major,
            origin: JavaOrigin::Owned,
            home: PathBuf::from("/mock/jdk"),
            java_bin: PathBuf::from("/mock/jdk/bin/java"),
        })
    }

    fn all(&self) -> Vec<JavaInstall> {
        vec![]
    }
}

struct NoJavaInstaller;

#[async_trait]
impl JavaProvisioner for NoJavaInstaller {
    async fn ensure(
        &self,
        _req: JavaRequirement,
        _p: &dyn ProgressReporter,
    ) -> cubecli_core::Result<JavaInstall> {
        panic!("installer must not run when the locator resolves Java");
    }
}

struct MockAddonSource {
    downloads: AtomicUsize,
}

#[async_trait]
impl AddonSource for MockAddonSource {
    fn id(&self) -> AddonSourceId {
        AddonSourceId::Modrinth
    }

    async fn resolve(&self, slug: &str) -> cubecli_core::Result<AddonProject> {
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
        _p: &AddonProject,
        _q: &AddonQuery,
    ) -> cubecli_core::Result<Vec<AddonFileEntry>> {
        Ok(vec![AddonFileEntry {
            version: "1.2.3".into(),
            file_name: "mod.jar".into(),
            url: Url::parse("http://mock/mod.jar").unwrap(),
            checksum: None,
            released_at: None,
            channel: Some(BuildChannel::Stable),
            provider_ref: String::new(),
        }])
    }

    async fn latest(
        &self,
        _p: &AddonProject,
        _q: &AddonQuery,
    ) -> cubecli_core::Result<Option<AddonFileEntry>> {
        Ok(self.files(_p, _q).await?.into_iter().next())
    }

    async fn download(
        &self,
        file: &AddonFileEntry,
        dest_dir: &Path,
    ) -> cubecli_core::Result<DownloadedAddon> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        let path = dest_dir.join(&file.file_name);
        std::fs::write(&path, b"mod bytes").unwrap();
        Ok(DownloadedAddon {
            path,
            file_name: file.file_name.clone(),
        })
    }

    async fn required_dependencies(
        &self,
        _p: &AddonProject,
        _f: &AddonFileEntry,
        _q: &AddonQuery,
    ) -> cubecli_core::Result<Vec<AddonProject>> {
        Ok(vec![AddonProject {
            slug: "fabric-api".into(),
            provider_id: "id-fabric-api".into(),
            title: "Fabric API".into(),
            description: String::new(),
        }])
    }
}

// ------------------------------------------------------------------------------------------------
// provision pipeline

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::Digest as _;
    let mut h = sha1::Sha1::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn provision_downloads_runs_steps_and_records_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = ProjectLayout::new(tmp.path());
    layout.ensure_state_dirs().unwrap();

    let jar = b"downloaded jar";
    let plan = InstallPlan {
        java: JavaRequirement { major: 21 },
        downloads: vec![DownloadSpec {
            url: Url::parse("http://mock/server.jar").unwrap(),
            checksum: Some(Checksum::Sha1(sha1_hex(jar))),
            dest: PathBuf::from("server.jar"),
            executable: false,
        }],
        steps: vec![
            PostInstallStep::RunCommand {
                argv: vec!["sh".into(), "-c".into(), "echo step-ran".into()],
                needs_java: false,
                timeout: std::time::Duration::from_secs(30),
                expect_paths: vec![PathBuf::from("marker.txt")],
            },
            PostInstallStep::WriteTextFile {
                path: PathBuf::from("marker.txt"),
                contents: "ok\n".into(),
            },
        ],
        launch: LaunchSpec::Jar {
            jar: "server.jar".into(),
            jvm_args: vec!["-Xmx1G".into()],
            args: vec!["nogui".into()],
        },
    };

    // The RunCommand expects marker.txt BEFORE the WriteTextFile creates it —
    // assert the pipeline's expect_paths fail-fast first.
    let fetcher = MockFetcher {
        fail_first: AtomicUsize::new(0),
    };
    let options = ProvisionOptions::new(&fetcher, &AlwaysJava, &NoJavaInstaller);
    let err = provision(&layout, &plan, &options).await.unwrap_err();
    assert!(err.to_string().contains("marker.txt is missing"), "{err}");

    // Correct order: write first, then a command that requires it.
    let plan = InstallPlan {
        steps: vec![
            plan.steps[1].clone(),
            PostInstallStep::RunCommand {
                argv: vec!["sh".into(), "-c".into(), "echo failing; exit 3".into()],
                needs_java: false,
                timeout: std::time::Duration::from_secs(30),
                expect_paths: vec![],
            },
        ],
        ..plan.clone()
    };
    // java -jar nonexistent.jar fails → installer failure surfaces with output
    let err = provision(&layout, &plan, &options).await.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("installer"),
        "{err}"
    );

    // Happy path: checksum-verified download + text file + report.
    let plan = InstallPlan {
        steps: vec![plan.steps[0].clone()],
        ..plan.clone()
    };
    let report = provision(&layout, &plan, &options).await.unwrap();
    assert_eq!(
        std::fs::read(layout.root().join("server.jar")).unwrap(),
        jar
    );
    let marker = std::fs::read_to_string(layout.root().join("marker.txt")).unwrap();
    assert!(marker.ends_with("ok\n"), "{marker:?}");
    assert_eq!(report.java.feature, 21);
    assert!(matches!(report.launch, LaunchSpec::Jar { .. }));
}

#[tokio::test]
async fn provision_checksum_mismatch_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = ProjectLayout::new(tmp.path());
    layout.ensure_state_dirs().unwrap();

    let plan = InstallPlan {
        java: JavaRequirement { major: 21 },
        downloads: vec![DownloadSpec {
            url: Url::parse("http://mock/server.jar").unwrap(),
            checksum: Some(Checksum::Sha1(
                "0000000000000000000000000000000000000000".into(),
            )),
            dest: PathBuf::from("server.jar"),
            executable: false,
        }],
        steps: vec![],
        launch: LaunchSpec::Jar {
            jar: "server.jar".into(),
            jvm_args: vec![],
            args: vec![],
        },
    };
    let fetcher = MockFetcher {
        fail_first: AtomicUsize::new(0),
    };
    let options = ProvisionOptions::new(&fetcher, &AlwaysJava, &NoJavaInstaller);
    let err = provision(&layout, &plan, &options).await.unwrap_err();
    assert!(matches!(
        err,
        cubecli_core::CoreError::ChecksumMismatch { .. }
    ));
    assert!(
        !layout.root().join("server.jar").exists(),
        "no truncated artifact"
    );
}

#[tokio::test]
async fn provision_missing_java_uses_installer() {
    struct MissingJava;
    impl JavaLocator for MissingJava {
        fn locate(&self, _r: JavaRequirement) -> cubecli_core::Result<JavaInstall> {
            Err(cubecli_core::CoreError::JavaMissing { feature: 21 })
        }
        fn all(&self) -> Vec<JavaInstall> {
            vec![]
        }
    }
    struct InstallingProvisioner;
    #[async_trait]
    impl JavaProvisioner for InstallingProvisioner {
        async fn ensure(
            &self,
            req: JavaRequirement,
            _p: &dyn ProgressReporter,
        ) -> cubecli_core::Result<JavaInstall> {
            Ok(JavaInstall {
                feature: req.major,
                origin: JavaOrigin::Owned,
                home: PathBuf::from("/fresh/jdk"),
                java_bin: PathBuf::from("/fresh/jdk/bin/java"),
            })
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let layout = ProjectLayout::new(tmp.path());
    layout.ensure_state_dirs().unwrap();
    let plan = InstallPlan {
        java: JavaRequirement { major: 21 },
        downloads: vec![],
        steps: vec![],
        launch: LaunchSpec::Jar {
            jar: "server.jar".into(),
            jvm_args: vec![],
            args: vec![],
        },
    };
    let fetcher = MockFetcher {
        fail_first: AtomicUsize::new(0),
    };
    let options = ProvisionOptions::new(&fetcher, &MissingJava, &InstallingProvisioner);
    let report = provision(&layout, &plan, &options).await.unwrap();
    assert_eq!(report.java.home, PathBuf::from("/fresh/jdk"));
}

// ------------------------------------------------------------------------------------------------
// addon pipeline

fn fabric_project() -> (tempfile::TempDir, ProjectHandle) {
    let tmp = tempfile::tempdir().unwrap();
    let layout = ProjectLayout::new(tmp.path());
    layout.ensure_state_dirs().unwrap();
    let mut meta = cubecli_core::ProjectMeta::default();
    meta.server.core = CoreKind::Fabric;
    meta.server.mc_version = "1.21.8".into();
    meta.server.eula_accepted = true;
    meta.save(layout.meta_file()).unwrap();
    (tmp, ProjectHandle::open(layout).unwrap())
}

fn query(kind: AddonKind) -> AddonQuery {
    AddonQuery {
        mc_version: "1.21.8".into(),
        loader: ModLoader::Fabric,
        kind,
    }
}

#[tokio::test]
async fn addon_install_records_deps_and_replaces() {
    let (_tmp, mut project) = fabric_project();
    let source = MockAddonSource {
        downloads: AtomicUsize::new(0),
    };

    let request = AddonRequest {
        kind: AddonKind::Mod,
        slug: "sodium".into(),
        version: None,
    };
    let records = install_addon(&mut project, &source, &request, &|_| true, 0)
        .await
        .unwrap();
    assert_eq!(records.len(), 2, "addon + its dependency");
    assert_eq!(records[0].slug, "sodium");
    assert_eq!(records[0].deps, vec!["fabric-api".to_string()]);
    assert!(project.layout().mods_dir().join("mod.jar").is_file());
    assert_eq!(project.meta().addons.installed.len(), 2);

    // Reinstalling replaces the file, never duplicates the record.
    let records2 = install_addon(&mut project, &source, &request, &|_| true, 0)
        .await
        .unwrap();
    eprintln!(
        "DEBUG installed: {:?}",
        project
            .meta()
            .addons
            .installed
            .iter()
            .map(|a| (&a.slug, &a.deps))
            .collect::<Vec<_>>()
    );
    eprintln!(
        "DEBUG records2: {:?}",
        records2.iter().map(|a| &a.slug).collect::<Vec<_>>()
    );
    assert_eq!(records2.len(), 1, "dependency already installed");
    assert_eq!(project.meta().addons.installed.len(), 2);
    assert_eq!(
        source.downloads.load(Ordering::SeqCst),
        3,
        "2 first install + 1 reinstall"
    );

    // Declined dependency on a fresh slug is skipped.
    let request = AddonRequest {
        kind: AddonKind::Mod,
        slug: "lithium".into(),
        version: None,
    };
    install_addon(&mut project, &source, &request, &|_| false, 0)
        .await
        .unwrap();
    assert_eq!(project.meta().addons.installed.len(), 3, "lithium only");
    assert!(project.meta().find_installed("fabric-api").is_some());
}

#[tokio::test]
async fn addon_install_enforces_compat_and_uninstall_cleans_up() {
    let (_tmp, mut project) = fabric_project();
    let source = MockAddonSource {
        downloads: AtomicUsize::new(0),
    };

    // plugin on fabric → hard error
    let request = AddonRequest {
        kind: AddonKind::Plugin,
        slug: "essentialsx".into(),
        version: None,
    };
    let err = install_addon(&mut project, &source, &request, &|_| true, 0)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        cubecli_core::CoreError::IncompatibleAddon { .. }
    ));

    // install then uninstall removes file + record
    let request = AddonRequest {
        kind: AddonKind::Mod,
        slug: "sodium".into(),
        version: None,
    };
    install_addon(&mut project, &source, &request, &|_| false, 0)
        .await
        .unwrap();
    let removed = uninstall_addon(&mut project, "SODIUM").unwrap();
    assert_eq!(removed.slug, "sodium");
    assert!(!project.layout().mods_dir().join("mod.jar").exists());
    assert!(project.meta().find_installed("sodium").is_none());
    assert!(uninstall_addon(&mut project, "sodium").is_err());
}

#[test]
fn request_parser() {
    assert!(parse_request("mod:sodium@1.0").is_ok());
    assert!(parse_request("mod:sodium").is_ok());
    assert!(parse_request("sodium").is_err());
    assert!(parse_request("shader:x").is_err());
    assert!(parse_request("mod:").is_err());
    let _ = Arc::new(query(AddonKind::Mod)); // keep imports honest
}
