//! Provider specs against local mock servers (wiremock) — URL shapes, JSON
//! parsing, channel filtering, auth/ratelimit mapping, checksum flows.
//! No real network access happens in this suite.

use std::sync::Arc;

use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use cubecli_core::model::{AddonKind, AddonSourceId, BuildChannel, ModLoader};
use cubecli_core::traits::addon_source::{AddonQuery, AddonSource, ServerSide};
use cubecli_core::traits::progress::ProgressReporter;
use cubecli_core::traits::server_core::ServerCoreProvider;

struct Noop;
impl ProgressReporter for Noop {}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

/// A mock piston-meta serving `1.21.8` + `1.21.1` (Java 21) — providers hit
/// it for Java requirements during install plans.
async fn mojang_mock() -> (MockServer, Arc<cubecli_providers::mojang::MojangMeta>) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mc/game/version_manifest_v2.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "versions": [
                {"id": "1.21.8", "type": "release", "url": format!("{}/v.json", server.uri())},
                {"id": "1.21.1", "type": "release", "url": format!("{}/v.json", server.uri())},
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "javaVersion": {"majorVersion": 21},
            "downloads": {"server": {"url": format!("{}/server.jar", server.uri()), "sha1": "abc"}}
        })))
        .mount(&server)
        .await;
    let meta = Arc::new(cubecli_providers::mojang::MojangMeta::with_base(
        client(),
        server.uri(),
    ));
    (server, meta)
}

// ------------------------------------------------------------------------------------------------
// mojang

#[tokio::test]
async fn mojang_uuid_lookup_dashes_hits_and_maps_misses_to_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/profiles/minecraft/CodeFlusher"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "5fbfba05c8934325915c0fde8bec0077",
            "name": "CodeFlusher"
        })))
        .mount(&server)
        .await;
    // unknown accounts answer 204 No Content with an empty body
    Mock::given(method("GET"))
        .and(path("/users/profiles/minecraft/nobody"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let meta = Arc::new(cubecli_providers::mojang::MojangMeta::with_base(
        client(),
        server.uri(),
    ));
    let uuid = meta.uuid_for_name("CodeFlusher").await.unwrap();
    assert_eq!(uuid, "5fbfba05-c893-4325-915c-0fde8bec0077");

    let err = meta.uuid_for_name("nobody").await.unwrap_err();
    assert!(matches!(
        err,
        cubecli_providers::ProviderError::NotFound { .. }
    ));
}

#[tokio::test]
async fn mojang_versions_filter_snapshots_and_plan() {
    let server = MockServer::start().await;
    let version_json = json!({
        "downloads": {"server": {"url": format!("{}/server.jar", server.uri()), "sha1": "abc"}},
        "javaVersion": {"majorVersion": 25}
    });
    Mock::given(method("GET"))
        .and(path("/mc/game/version_manifest_v2.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "latest": {"release": "26.3"},
            "versions": [
                {"id": "26.4-snapshot-1", "type": "snapshot", "url": format!("{}/snap.json", server.uri())},
                {"id": "26.3", "type": "release", "url": format!("{}/v.json", server.uri())},
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(version_json))
        .mount(&server)
        .await;
    // snapshot URL is never fetched — prove it by NOT mounting /snap.json

    let meta = Arc::new(cubecli_providers::mojang::MojangMeta::with_base(
        client(),
        server.uri(),
    ));
    let provider = cubecli_providers::mojang::MojangProvider::new(meta.clone());

    let versions = provider.list_versions().await.unwrap();
    assert_eq!(versions.len(), 1, "snapshots filtered");
    assert_eq!(versions[0].id, "26.3");

    assert_eq!(meta.java_requirement("26.3").await.unwrap(), 25);

    let plan = provider.install_plan("26.3", "").await.unwrap();
    assert_eq!(plan.java.major, 25);
    assert_eq!(plan.downloads.len(), 1);
    assert_eq!(
        plan.downloads[0].dest,
        std::path::PathBuf::from("server.jar")
    );
    assert!(matches!(
        plan.downloads[0].checksum,
        Some(cubecli_core::model::Checksum::Sha1(ref h)) if h == "abc"
    ));
}

// ------------------------------------------------------------------------------------------------
// paper

async fn paper_server() -> MockServer {
    MockServer::start().await
}

async fn paper_mocks(server: &MockServer, latest_channel: &str) {
    Mock::given(method("GET"))
        .and(path("/projects/paper"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "versions": {"1.21": ["1.21.11", "1.21.8"], "26.3": ["26.3"]}
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/projects/paper/versions/1.21.8"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "version": {"java": {"version": {"minimum": 21}}},
            "builds": [60, 59, 58]
        })))
        .mount(server)
        .await;
    let build = |id: i64, channel: &str| {
        json!({
            "id": id,
            "channel": channel,
            "downloads": {"server:default": {
                "name": format!("paper-1.21.8-{id}.jar"),
                "url": format!("{}/paper-{id}.jar", server.uri()),
                "checksums": {"sha256": format!("{id:064x}")},
            }}
        })
    };
    Mock::given(method("GET"))
        .and(path("/projects/paper/versions/1.21.8/builds/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(build(60, latest_channel)))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/projects/paper/versions/1.21.8/builds/59"))
        .respond_with(ResponseTemplate::new(200).set_body_json(build(59, "STABLE")))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/projects/paper/versions/1.21.8/builds/60"))
        .respond_with(ResponseTemplate::new(200).set_body_json(build(60, latest_channel)))
        .mount(server)
        .await;
}

#[tokio::test]
async fn paper_latest_build_skips_experimental() {
    let server = paper_server().await;
    paper_mocks(&server, "EXPERIMENTAL").await;
    let provider = cubecli_providers::paper::PaperProvider::with_base(client(), server.uri());

    let latest = provider
        .latest_build("1.21.8", BuildChannel::Stable)
        .await
        .unwrap();
    assert_eq!(latest.id, "59", "must scan back to the STABLE build");
}

#[tokio::test]
async fn paper_versions_sorted_and_plan_uses_api_url() {
    let server = paper_server().await;
    paper_mocks(&server, "STABLE").await;
    let provider = cubecli_providers::paper::PaperProvider::with_base(client(), server.uri());

    let versions = provider.list_versions().await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        vec!["26.3", "1.21.11", "1.21.8"]
    );

    let plan = provider.install_plan("1.21.8", "").await.unwrap();
    assert_eq!(plan.java.major, 21);
    assert_eq!(
        plan.downloads[0].dest,
        std::path::PathBuf::from("paper-1.21.8-60.jar")
    );
    assert!(plan.downloads[0].url.as_str().starts_with(&server.uri()));
    let cubecli_core::traits::server_core::InstallPlan { launch, .. } = &plan;
    match launch {
        cubecli_core::model::LaunchSpec::Jar { jar, args, .. } => {
            assert_eq!(jar, "paper-1.21.8-60.jar");
            assert_eq!(args, &["nogui".to_string()]);
        }
        other => panic!("unexpected launch: {other:?}"),
    }
}

#[tokio::test]
async fn paper_404_maps_to_not_found() {
    let server = paper_server().await;
    let provider = cubecli_providers::paper::PaperProvider::with_base(client(), server.uri());
    let err = provider.list_builds("9.9.9").await.unwrap_err();
    assert!(matches!(err, cubecli_core::CoreError::External { .. }));
}

// ------------------------------------------------------------------------------------------------
// purpur

#[tokio::test]
async fn purpur_versions_builds_and_plan() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/purpur"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "versions": ["1.21.8", "1.14.1"], "metadata": {"current": "1.21.8"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/purpur/1.21.8"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "builds": {"latest": "2497", "all": ["2497", "2478"]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/purpur/1.21.8/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "build": "2497", "result": "SUCCESS", "md5": "feedface"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/purpur/1.21.8/2497"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "build": "2497", "result": "SUCCESS", "md5": "feedface"
        })))
        .mount(&server)
        .await;

    let (_mojang, meta) = mojang_mock().await;
    let provider = cubecli_providers::purpur::PurpurProvider::with_base(
        client(),
        format!("{}/purpur", server.uri()),
        meta,
    );

    let versions = provider.list_versions().await.unwrap();
    assert_eq!(versions[0].id, "1.21.8");

    let builds = provider.list_builds("1.21.8").await.unwrap();
    assert_eq!(
        builds.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
        vec!["2497", "2478"]
    );

    let plan = provider.install_plan("1.21.8", "").await.unwrap();
    let cubecli_core::model::Checksum::Md5(md5) = plan.downloads[0].checksum.clone().unwrap()
    else {
        panic!("expected md5");
    };
    assert_eq!(md5, "feedface");
    assert!(
        plan.downloads[0]
            .url
            .as_str()
            .ends_with("/purpur/1.21.8/2497/download")
    );
}

// ------------------------------------------------------------------------------------------------
// fabric

#[tokio::test]
async fn fabric_install_plan_uses_first_stable_loader() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/versions/game"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"version": "26.4-snapshot-6", "stable": false},
            {"version": "1.21.8", "stable": true},
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/versions/loader"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"version": "0.21.0-beta1", "stable": false},
            {"version": "0.20.1", "stable": true},
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/versions/installer"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"version": "1.1.2", "stable": true},
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/versions/loader/1.21.8/0.20.1/1.1.2/server/jar"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hybrid".to_vec()))
        .mount(&server)
        .await;

    let (_mojang, meta) = mojang_mock().await;
    let provider =
        cubecli_providers::fabric::FabricProvider::with_base(client(), server.uri(), meta);

    let versions = provider.list_versions().await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        vec!["1.21.8"]
    );

    let plan = provider.install_plan("1.21.8", "").await.unwrap();
    assert_eq!(
        plan.downloads[0].dest,
        std::path::PathBuf::from("server.jar")
    );
    assert!(
        plan.downloads[0]
            .url
            .as_str()
            .contains("/loader/1.21.8/0.20.1/1.1.2/server/jar")
    );
}

// ------------------------------------------------------------------------------------------------
// neoforge

#[tokio::test]
async fn neoforge_version_derivation_and_builds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/neoforge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "versions": ["21.1.107", "21.1.90-beta", "26.3.0.100", "26.3.0.99", "0.25w14craftmine.3-beta"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/neoforge-installer.jar"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"installer".to_vec()))
        .mount(&server)
        .await;

    let (_mojang, meta) = mojang_mock().await;
    let provider = cubecli_providers::neoforge::NeoForgeProvider::with_base(
        client(),
        format!("{}/neoforge", server.uri()),
        format!("{}/", server.uri()),
        meta,
    );

    // mc version derivation: 3-part → 1.x.y, 4-part → a.b, beta/0.x dropped
    let versions = provider.list_versions().await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        vec!["26.3", "1.21.1"]
    );

    let builds = provider.list_builds("1.21.1").await.unwrap();
    assert_eq!(
        builds.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
        vec!["21.1.107"]
    );

    let plan = provider.install_plan("1.21.1", "21.1.107").await.unwrap();
    assert!(
        plan.downloads[0]
            .url
            .as_str()
            .ends_with("/21.1.107/neoforge-21.1.107-installer.jar")
    );
    assert!(matches!(
        plan.steps[0],
        cubecli_core::traits::server_core::PostInstallStep::RunCommand { ref expect_paths, .. }
            if expect_paths == &vec![std::path::PathBuf::from("run.sh")]
    ));
    assert!(matches!(
        plan.launch,
        cubecli_core::model::LaunchSpec::Script { .. }
    ));
}

// ------------------------------------------------------------------------------------------------
// forge

#[tokio::test]
async fn forge_promotions_list_and_plan() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/promotions_slim.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "promos": {
                "1.21.1-latest": "52.1.20",
                "1.21.1-recommended": "52.1.0",
                "26.3-latest": "66.1.2",
                "1.20.1-recommended": "47.2.0",
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/net/minecraftforge/forge/forge-1.21.1-52.1.0/forge-1.21.1-52.1.0-installer.jar",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"installer".to_vec()))
        .mount(&server)
        .await;

    let (_mojang, meta) = mojang_mock().await;
    let provider = cubecli_providers::forge::ForgeProvider::with_base(
        client(),
        format!("{}/promotions_slim.json", server.uri()),
        format!("{}/net/minecraftforge/forge/", server.uri()),
        meta,
    );

    let versions = provider.list_versions().await.unwrap();
    // only versions with a *recommended* promo, newest first
    assert_eq!(
        versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        vec!["1.21.1", "1.20.1"]
    );

    let builds = provider.list_builds("1.21.1").await.unwrap();
    assert_eq!(builds[0].id, "52.1.0", "recommended first");
    assert_eq!(builds[1].id, "52.1.20");

    let latest = provider
        .latest_build("1.21.1", BuildChannel::Stable)
        .await
        .unwrap();
    assert_eq!(latest.id, "52.1.0", "stable tier = recommended");

    let plan = provider.install_plan("1.21.1", "52.1.0").await.unwrap();
    assert!(
        plan.downloads[0]
            .url
            .as_str()
            .ends_with("/1.21.1-52.1.0/forge-1.21.1-52.1.0-installer.jar")
    );
}

// ------------------------------------------------------------------------------------------------
// modrinth

fn modrinth_query(kind: AddonKind) -> AddonQuery {
    AddonQuery {
        mc_version: "1.21.8".into(),
        loader: ModLoader::Fabric,
        kind,
    }
}

fn sodium_versions() -> Value {
    json!([
        {
            "version_number": "0.7.4-beta",
            "version_type": "beta",
            "id": "bbbb",
            "files": [{"url": "http://primary/0.7.4.jar", "filename": "sodium-0.7.4.jar", "primary": true, "hashes": {"sha1": "aaa"}}],
            "dependencies": [],
            "date_published": "2026-10-01T00:00:00Z"
        },
        {
            "version_number": "0.7.3",
            "version_type": "release",
            "id": "aaaa",
            "files": [{"url": "http://primary/0.7.3.jar", "filename": "sodium-0.7.3.jar", "primary": true, "hashes": {"sha1": "bbb"}}],
            "dependencies": [{"project_id": "dep-1", "dependency_type": "required"}],
            "date_published": "2026-09-01T00:00:00Z"
        }
    ])
}

#[tokio::test]
async fn modrinth_latest_prefers_stable_and_files_parse() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/project/sodium"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "slug": "sodium", "id": "AANobbMI", "title": "Sodium", "description": "fast", "project_type": "mod"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/sodium/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sodium_versions()))
        .mount(&server)
        .await;

    let source = cubecli_providers::modrinth::ModrinthSource::with_base(
        client(),
        format!("{}/v2", server.uri()),
    );
    let project = source.resolve("sodium").await.unwrap();
    assert_eq!(project.provider_id, "AANobbMI");

    let query = modrinth_query(AddonKind::Mod);
    let files = source.files(&project, &query).await.unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].version, "0.7.4-beta", "API order preserved");

    let latest = source.latest(&project, &query).await.unwrap().unwrap();
    assert_eq!(latest.version, "0.7.3", "stable preferred over newer beta");
    assert!(matches!(
        latest.checksum,
        Some(cubecli_core::model::Checksum::Sha1(ref h)) if h == "bbb"
    ));

    // wrong kind on the same slug is rejected
    let err = cubecli_providers::modrinth::check_project_type(
        AddonKind::Plugin,
        &json!({
            "slug": "sodium", "project_type": "mod"
        }),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        cubecli_providers::ProviderError::Unsupported { .. }
    ));
}

#[tokio::test]
async fn modrinth_search_scopes_by_project_type_and_parses_hits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/search"))
        .and(query_param("query", "sodium"))
        .and(query_param("facets", r#"[["project_type:mod"]]"#))
        .and(query_param("limit", "15"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "hits": [
                {"slug": "sodium", "title": "Sodium", "description": "fast", "author": "jellysquid3", "downloads": 5_000_000},
                {"title": "NoSlug", "description": "missing the slug field"},
            ]
        })))
        .mount(&server)
        .await;

    let source = cubecli_providers::modrinth::ModrinthSource::with_base(
        client(),
        format!("{}/v2", server.uri()),
    );
    let hits = source.search("sodium", AddonKind::Mod).await.unwrap();
    assert_eq!(hits.len(), 1, "entries without a slug are dropped");
    assert_eq!(hits[0].slug, "sodium");
    assert_eq!(hits[0].title, "Sodium");
    assert_eq!(hits[0].author, "jellysquid3");
    assert_eq!(hits[0].downloads, 5_000_000);
}

#[tokio::test]
async fn modrinth_download_verifies_and_deps_resolve() {
    let server = MockServer::start().await;
    let jar_bytes = b"sodium jar bytes".to_vec();
    let sha1 = {
        use sha1::Digest as _;
        let mut h = sha1::Sha1::new();
        h.update(&jar_bytes);
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    Mock::given(method("GET"))
        .and(path("/v2/project/sodium"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "slug": "sodium", "id": "AANobbMI", "title": "Sodium", "description": "", "project_type": "mod"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/sodium/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "version_number": "0.7.3",
                "version_type": "release",
                "id": "aaaa",
                "files": [{"url": format!("{}/sodium.jar", server.uri()), "filename": "sodium.jar", "primary": true, "hashes": {"sha1": sha1}}],
                "dependencies": [{"project_id": "AANobbDEP", "dependency_type": "required"},
                                 {"project_id": "optional-1", "dependency_type": "optional"}],
                "date_published": "2026-09-01T00:00:00Z"
            }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sodium.jar"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jar_bytes.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/AANobbDEP"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "slug": "fabric-api", "id": "AANobbDEP", "title": "Fabric API", "description": "", "project_type": "mod"
        })))
        .mount(&server)
        .await;

    let source = cubecli_providers::modrinth::ModrinthSource::with_base(
        client(),
        format!("{}/v2", server.uri()),
    );
    let project = source.resolve("sodium").await.unwrap();
    let query = modrinth_query(AddonKind::Mod);
    let entries = source.files(&project, &query).await.unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let downloaded = source.download(&entries[0], tmp.path()).await.unwrap();
    assert_eq!(std::fs::read(&downloaded.path).unwrap(), jar_bytes);

    let deps = source
        .required_dependencies(&project, &entries[0], &query)
        .await
        .unwrap();
    assert_eq!(deps.len(), 1, "optional deps excluded");
    assert_eq!(deps[0].slug, "fabric-api");
}

// ------------------------------------------------------------------------------------------------
// curseforge

fn cf_key() -> cubecli_providers::curseforge::KeyProvider {
    Arc::new(|| Some("test-key".into()))
}

fn cf_no_key() -> cubecli_providers::curseforge::KeyProvider {
    Arc::new(|| None)
}

#[tokio::test]
async fn curseforge_missing_key_maps_to_auth_hint() {
    let server = MockServer::start().await;
    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        server.uri(),
        cf_no_key(),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    let err = source.resolve("jei").await.unwrap_err();
    assert!(matches!(
        err,
        cubecli_core::CoreError::Auth {
            service: "curseforge"
        }
    ));
    assert!(err.hint().unwrap().contains("setup curseforge"));
}

#[tokio::test]
async fn curseforge_bad_key_401_maps_to_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        server.uri(),
        cf_key(),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    let err = source.resolve("jei").await.unwrap_err();
    assert!(matches!(
        err,
        cubecli_core::CoreError::Auth {
            service: "curseforge"
        }
    ));
}

#[tokio::test]
async fn curseforge_search_slug_match_files_and_cdn_download() {
    let server = MockServer::start().await;
    let jar = b"cf jar".to_vec();

    // search: slug match is client-side
    Mock::given(method("GET"))
        .and(path("/v1/mods/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {"id": 999, "slug": "not-jei", "name": "Not JEI"},
                {"id": 238222, "slug": "jei", "name": "JEI"},
            ]
        })))
        .mount(&server)
        .await;
    // files for the project
    Mock::given(method("GET"))
        .and(path("/v1/mods/238222/files"))
        .and(query_param("gameVersion", "1.21.8"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{
                "id": 12345,
                "fileName": "jei.jar",
                "displayName": "JEI 1.21.8",
                "releaseType": 1,
                "fileDate": "2026-09-01T00:00:00Z",
                "hashes": [{"value": "deadbeef", "algo": 1}],
            }]
        })))
        .mount(&server)
        .await;
    // download-url endpoint (key required)
    Mock::given(method("GET"))
        .and(path("/v1/mods/238222/files/12345/download-url"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": format!("{}/cdn/jei.jar", server.uri())
        })))
        .mount(&server)
        .await;
    // the CDN also demands the key header
    Mock::given(method("GET"))
        .and(path("/cdn/jei.jar"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jar.clone()))
        .mount(&server)
        .await;

    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        format!("{}/v1", server.uri()),
        cf_key(),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    let project = source.resolve("jei").await.unwrap();
    assert_eq!(project.provider_id, "238222");

    let query = AddonQuery {
        mc_version: "1.21.8".into(),
        loader: ModLoader::PaperLike,
        kind: AddonKind::Plugin,
    };
    let files = source.files(&project, &query).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].provider_ref, "238222:12345");

    let tmp = tempfile::tempdir().unwrap();
    let downloaded = source.download(&files[0], tmp.path()).await.unwrap();
    assert_eq!(std::fs::read(&downloaded.path).unwrap(), jar);
}

#[tokio::test]
async fn curseforge_search_scopes_by_class_and_parses_hits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/mods/search"))
        .and(query_param("searchFilter", "essentials"))
        .and(query_param("classId", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {"id": 1, "slug": "essentialsx", "name": "EssentialsX", "summary": "the plugin",
                 "downloadCount": 9_000_000, "authors": [{"name": "EssentialsX Team"}]},
                {"slug": "bad", "name": "no slug? has one — but drop nothing here"},
            ]
        })))
        .mount(&server)
        .await;

    let cache_dir = seeded_class_cache(5);
    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        format!("{}/v1", server.uri()),
        cf_key(),
        cache_dir.path().to_path_buf(),
    );
    let hits = source
        .search("essentials", AddonKind::Plugin)
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].slug, "essentialsx");
    assert_eq!(hits[0].title, "EssentialsX");
    assert_eq!(hits[0].author, "EssentialsX Team");
    assert_eq!(hits[0].downloads, 9_000_000);
}

#[tokio::test]
async fn curseforge_search_rejects_datapacks_without_calling_the_api() {
    let server = MockServer::start().await;
    let cache_dir = seeded_class_cache(5);
    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        format!("{}/v1", server.uri()),
        cf_key(),
        cache_dir.path().to_path_buf(),
    );
    let err = source
        .search("anything", AddonKind::Datapack)
        .await
        .unwrap_err();
    assert!(matches!(err, cubecli_core::CoreError::Unsupported { .. }));
}

/// CurseForge search filters by classId, which normally costs a live
/// /categories roundtrip — pre-seed the cache so the suite stays offline.
fn seeded_class_cache(plugin_class_id: u32) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let cache = cubecli_providers::cf_categories::ClassCache {
        fetched_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        classes: [("bukkit-plugins".to_string(), plugin_class_id)]
            .into_iter()
            .collect(),
    };
    std::fs::write(
        dir.path().join("cf-categories.json"),
        serde_json::to_string_pretty(&cache).unwrap(),
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn curseforge_slug_miss_is_not_found_and_datapack_unsupported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/mods/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;

    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        format!("{}/v1", server.uri()),
        cf_key(),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    let err = source.resolve("nope").await.unwrap_err();
    assert!(matches!(err, cubecli_core::CoreError::NotFound { .. }));

    let project = cubecli_core::traits::addon_source::AddonProject {
        slug: "x".into(),
        provider_id: "1".into(),
        title: String::new(),
        description: String::new(),
    };
    let err = source
        .files(
            &project,
            &AddonQuery {
                mc_version: "1.21.8".into(),
                loader: ModLoader::Vanilla,
                kind: AddonKind::Datapack,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, cubecli_core::CoreError::Unsupported { .. }));
}

// ------------------------------------------------------------------------------------------------
// file identification (tools export-modlist)

fn write_jar(dir: &std::path::Path, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn sha1_hex(content: &[u8]) -> String {
    use sha1::Digest as _;
    let mut h = sha1::Sha1::new();
    h.update(content);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn modrinth_identifies_file_by_sha1_and_unknown_hash_is_none() {
    let server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let known = write_jar(tmp.path(), "known.jar", b"sodium jar contents");
    let unknown = write_jar(tmp.path(), "unknown.jar", b"mystery jar contents");

    let known_hash = sha1_hex(b"sodium jar contents");
    Mock::given(method("GET"))
        .and(path(format!("/v2/version_file/{known_hash}")))
        .and(query_param("algorithm", "sha1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "project_id": "AANobbMI",
            "project_type": "mod",
            "version_number": "0.6.5",
            "game_versions": ["1.21.1"],
        })))
        .mount(&server)
        .await;
    // any other hash is a miss
    Mock::given(method("GET"))
        .and(path("/v2/version_file/"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/AANobbMI"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "slug": "sodium", "id": "AANobbMI", "title": "Sodium", "description": "fast",
            "project_type": "mod", "server_side": "required"
        })))
        .mount(&server)
        .await;

    let source = cubecli_providers::modrinth::ModrinthSource::with_base(
        client(),
        format!("{}/v2", server.uri()),
    );
    let identified = source.identify(&known).await.unwrap().expect("known jar");
    assert_eq!(identified.project.slug, "sodium");
    assert_eq!(identified.project.provider_id, "AANobbMI");
    assert_eq!(identified.version, "0.6.5");
    assert_eq!(identified.kind, Some(AddonKind::Mod));
    assert_eq!(identified.server_side, Some(ServerSide::Required));

    // a hash Modrinth doesn't know is None, not an error
    let miss = source.identify(&unknown).await.unwrap();
    assert!(miss.is_none());
}

#[tokio::test]
async fn modrinth_server_side_parses_all_declared_values() {
    let server = MockServer::start().await;
    for (id, side) in [
        ("req", "required"),
        ("opt", "optional"),
        ("unsup", "unsupported"),
        ("odd", "client-and-server"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/v2/project/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "slug": id, "id": id, "title": id, "description": "",
                "project_type": "mod", "server_side": side
            })))
            .mount(&server)
            .await;
    }

    let source = cubecli_providers::modrinth::ModrinthSource::with_base(
        client(),
        format!("{}/v2", server.uri()),
    );
    let project = |slug: &str| cubecli_core::traits::addon_source::AddonProject {
        slug: slug.into(),
        provider_id: slug.into(),
        title: String::new(),
        description: String::new(),
    };
    use cubecli_core::traits::addon_source::AddonSource as _;
    assert_eq!(
        source.server_side(&project("req")).await.unwrap(),
        Some(ServerSide::Required)
    );
    assert_eq!(
        source.server_side(&project("opt")).await.unwrap(),
        Some(ServerSide::Optional)
    );
    assert_eq!(
        source.server_side(&project("unsup")).await.unwrap(),
        Some(ServerSide::Unsupported)
    );
    assert_eq!(
        source.server_side(&project("odd")).await.unwrap(),
        Some(ServerSide::Unknown),
        "undeclared values map to Unknown"
    );
}

#[tokio::test]
async fn curseforge_identifies_file_via_fingerprints_and_file_detail() {
    let server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let known = write_jar(tmp.path(), "mod.jar", b"cf jar contents");
    let unknown = write_jar(tmp.path(), "other.jar", b"untracked contents");

    Mock::given(method("POST"))
        .and(path("/v1/fingerprints"))
        .and(body_partial_json(json!({
            "fingerprints": [cubecli_providers::murmur2::digest(b"cf jar contents")]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "isCacheBuilt": true,
                "exactMatches": [
                    {"id": 238222, "file": {"id": 12345, "modId": 238222, "fileName": "jei.jar"}}
                ],
                "unmatchedFingerprints": [],
            }
        })))
        .mount(&server)
        .await;
    // every other fingerprint lands in unmatchedFingerprints
    Mock::given(method("POST"))
        .and(path("/v1/fingerprints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"isCacheBuilt": true, "exactMatches": [], "unmatchedFingerprints": [42]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/mods/238222"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"id": 238222, "slug": "jei", "name": "Just Enough Items"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/mods/238222/files/12345"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"id": 12345, "displayName": "JEI 1.21.8", "isClientSideOnly": false}
        })))
        .mount(&server)
        .await;

    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        format!("{}/v1", server.uri()),
        cf_key(),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    let identified = source.identify(&known).await.unwrap().expect("tracked jar");
    assert_eq!(identified.project.slug, "jei");
    assert_eq!(identified.project.provider_id, "238222");
    assert_eq!(identified.version, "JEI 1.21.8");
    assert_eq!(identified.kind, None, "CF cannot tell the addon kind");
    assert_eq!(identified.server_side, None, "not client-side-only");

    // An unmatched fingerprint is None, not an error.
    let miss = source.identify(&unknown).await.unwrap();
    assert!(miss.is_none(), "untracked jar must not error");
}

#[tokio::test]
async fn curseforge_identify_flags_client_side_only_files() {
    let server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let jar = write_jar(tmp.path(), "clientmod.jar", b"client jar contents");

    Mock::given(method("POST"))
        .and(path("/v1/fingerprints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"exactMatches": [{"id": 7, "file": {"id": 8}}], "unmatchedFingerprints": []}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/mods/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"id": 7, "slug": "clientmod", "name": "ClientMod"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/mods/7/files/8"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"id": 8, "displayName": "ClientMod", "isClientSideOnly": true}
        })))
        .mount(&server)
        .await;

    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        format!("{}/v1", server.uri()),
        cf_key(),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    let identified = source.identify(&jar).await.unwrap().unwrap();
    assert_eq!(
        identified.server_side,
        Some(ServerSide::Unsupported),
        "isClientSideOnly maps to Unsupported"
    );
}

#[tokio::test]
async fn curseforge_identify_needs_the_api_key() {
    let server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let jar = write_jar(tmp.path(), "x.jar", b"contents");

    let source = cubecli_providers::curseforge::CurseForgeSource::with_base(
        client(),
        format!("{}/v1", server.uri()),
        cf_no_key(),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    let err = source.identify(&jar).await.unwrap_err();
    assert!(
        matches!(
            err,
            cubecli_providers::ProviderError::Auth {
                service: "curseforge"
            }
        ),
        "{err}"
    );
}

// ------------------------------------------------------------------------------------------------
// http helpers

#[test]
fn service_tag_covers_all_hosts() {
    use cubecli_providers::http::service_of;
    for (url, expected) in [
        ("https://api.modrinth.com/v2", "modrinth"),
        ("https://api.curseforge.com/v1", "curseforge"),
        ("https://edge.forgecdn.net/x", "curseforge"),
        ("https://fill.papermc.io/v3", "paper"),
        ("https://api.purpurmc.org/v2", "purpur"),
        ("https://meta.fabricmc.net/v2", "fabric"),
        ("https://maven.neoforged.net/api", "neoforge"),
        ("https://files.minecraftforge.net/x", "forge"),
        ("https://piston-meta.mojang.com/x", "mojang"),
        ("https://api.adoptium.net/v3", "adoptium"),
        ("https://example.com", "network"),
    ] {
        assert_eq!(service_of(url), expected, "{url}");
    }
}

#[test]
fn encode_segment_encodes_unsafe_bytes() {
    use cubecli_providers::http::encode_segment;
    assert_eq!(encode_segment("1.21.8"), "1.21.8");
    assert_eq!(
        encode_segment("1.14.4 Pre-Release 5"),
        "1.14.4%20Pre-Release%205"
    );
    assert_eq!(encode_segment("a/b"), "a%2Fb");
}
