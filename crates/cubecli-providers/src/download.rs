//! Streaming downloads: `dest.part` → verify → atomic rename.

use std::path::Path;

use async_trait::async_trait;
use url::Url;

use crate::error::{ProviderError, Result};

/// Download `url` to `dest`, verifying `checksum` when present. Writes to
/// `dest.part` first so a failed download never leaves a truncated file
/// where the server expects a complete one.
pub async fn download_to(
    client: &reqwest::Client,
    url: &Url,
    dest: &Path,
    checksum: Option<&cubecli_core::model::Checksum>,
    progress: &dyn cubecli_core::traits::ProgressReporter,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ProviderError::io(parent, e))?;
    }
    let part = dest.with_extension(format!("part{}", std::process::id()));

    let response = client
        .get(url.clone())
        .timeout(std::time::Duration::from_secs(30 * 60))
        .send()
        .await
        .map_err(|e| ProviderError::Http(e.to_string()))?;
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::RateLimited {
            service: crate::http::service_of(url.as_str()),
            retry_after: None,
        });
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth {
            service: crate::http::service_of(url.as_str()),
        });
    }
    if !status.is_success() {
        return Err(ProviderError::Http(format!("GET {url} returned {status}")));
    }

    let total = response.content_length();
    progress.started(&dest.display().to_string(), total);

    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| ProviderError::io(&part, e))?;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &bytes)
            .await
            .map_err(|e| ProviderError::io(&part, e))?;
        progress.progressed(bytes.len() as u64);
    }
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .map_err(|e| ProviderError::io(&part, e))?;
    drop(file);
    progress.finished(&dest.display().to_string());

    if let Some(expected) = checksum {
        let actual = tokio::task::spawn_blocking({
            let part = part.clone();
            let algorithm = expected.algorithm().to_string();
            move || crate::checksum::digest_file(&algorithm, &part)
        })
        .await
        .map_err(|e| ProviderError::Other(e.to_string()))??;
        if !expected.matches(&actual) {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(ProviderError::ChecksumMismatch {
                file: dest.display().to_string(),
                algorithm: expected.algorithm(),
                expected: expected.expected_hex().to_string(),
                actual,
            });
        }
    }

    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| ProviderError::io(dest, e))?;
    Ok(())
}

/// The [`cubecli_core::pipeline::FileFetcher`] implementation backed by the
/// shared reqwest client.
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl cubecli_core::pipeline::FileFetcher for HttpFetcher {
    async fn fetch(
        &self,
        url: &Url,
        checksum: Option<&cubecli_core::model::Checksum>,
        dest: &Path,
        progress: &dyn cubecli_core::traits::ProgressReporter,
    ) -> cubecli_core::Result<()> {
        download_to(&self.client, url, dest, checksum, progress)
            .await
            .map_err(Into::into)
    }
}
