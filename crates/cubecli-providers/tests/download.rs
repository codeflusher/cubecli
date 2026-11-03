//! Download engine tests against a local mock server.

use std::path::Path;

use cubecli_core::model::Checksum;
use cubecli_core::traits::progress::ProgressReporter;
use cubecli_providers::download::download_to;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Noop;

impl ProgressReporter for Noop {}

const CONTENT: &[u8] = b"hello server jar bytes";
const SHA1: &str = "0e6c3db2d7670ae6cbec9b3d8b66d6e39e4f6e2f";

fn sha1_of(bytes: &[u8]) -> String {
    use sha1::Digest as _;
    let mut h = sha1::Sha1::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn downloads_verify_and_rename_atomically() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("server.jar");
    let url: url::Url = format!("{}/server.jar", server.uri()).parse().unwrap();

    download_to(
        &reqwest::Client::new(),
        &url,
        &dest,
        Some(&Checksum::Sha1(sha1_of(CONTENT))),
        &Noop,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), CONTENT);
    // no leftover .part files
    let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn checksum_mismatch_removes_part_and_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"corrupted".to_vec()))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("server.jar");
    let url: url::Url = format!("{}/f", server.uri()).parse().unwrap();

    let err = download_to(
        &reqwest::Client::new(),
        &url,
        &dest,
        Some(&Checksum::Sha1(SHA1.into())),
        &Noop,
    )
    .await
    .unwrap_err();

    assert!(matches!(
        err,
        cubecli_providers::ProviderError::ChecksumMismatch { .. }
    ));
    assert!(!dest.exists(), "no truncated file at dest");
    assert!(
        !any_part_files(tmp.path()),
        ".part file must be cleaned up after mismatch"
    );
}

#[tokio::test]
async fn http_error_surfaces() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let url: url::Url = format!("{}/missing", server.uri()).parse().unwrap();
    let err = download_to(
        &reqwest::Client::new(),
        &url,
        &tmp.path().join("x"),
        None,
        &Noop,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, cubecli_providers::ProviderError::Http(_)));
}

fn any_part_files(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().contains(".part"))
}
