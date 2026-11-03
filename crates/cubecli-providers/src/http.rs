//! Shared HTTP client: identifying User-Agent (mandatory for Modrinth,
//! polite everywhere else), rustls, generous redirect chain (Adoptium hops
//! through GitHub).

use std::time::Duration;

use crate::error::{ProviderError, Result};

pub fn user_agent() -> String {
    format!(
        "cubecli/{} (minecraft server manager)",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(user_agent())
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .expect("static client config")
}

/// GET a JSON document with retry on transient failures.
pub async fn get_json(client: &reqwest::Client, url: &str) -> Result<serde_json::Value> {
    let body = get_text_with_retry(client, url).await?;
    serde_json::from_str(&body)
        .map_err(|e| ProviderError::Other(format!("invalid JSON from {url}: {e}")))
}

/// GET text with up to 3 attempts (429/5xx/connection errors), honoring Retry-After.
pub async fn get_text_with_retry(client: &reqwest::Client, url: &str) -> Result<String> {
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let result = send_get(client, url).await;
        match result {
            Ok(body) => return Ok(body),
            Err(ProviderError::RateLimited {
                service,
                retry_after,
            }) if attempts < 3 => {
                let wait = retry_after.unwrap_or(Duration::from_secs(2u64.pow(attempts)));
                tracing::warn!(service, ?wait, "rate limited, retrying");
                tokio::time::sleep(wait).await;
            }
            Err(ProviderError::Http(m)) if attempts < 3 && is_transient(&m) => {
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempts))).await;
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_transient(message: &str) -> bool {
    message.contains("status 5") || message.contains("connection") || message.contains("timed out")
}

async fn send_get(client: &reqwest::Client, url: &str) -> Result<String> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| ProviderError::Http(e.to_string()))?;

    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs);

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth {
            service: service_of(url),
        });
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::RateLimited {
            service: service_of(url),
            retry_after,
        });
    }
    if !status.is_success() {
        return Err(ProviderError::Http(format!("GET {url} returned {status}")));
    }
    response
        .text()
        .await
        .map_err(|e| ProviderError::Http(e.to_string()))
}

/// Percent-encode a URL path segment (version ids can contain spaces).
/// RFC 3986 unreserved characters pass through; everything else becomes %XX
/// (form-encoding's `+` for spaces would corrupt paths).
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for &b in segment.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Best-effort service tag from the host — drives hints and rate-limit text.
pub fn service_of(url: &str) -> &'static str {
    if url.contains("modrinth.com") {
        "modrinth"
    } else if url.contains("curseforge.com") || url.contains("forgecdn.net") {
        "curseforge"
    } else if url.contains("papermc.io") {
        "paper"
    } else if url.contains("purpurmc.org") {
        "purpur"
    } else if url.contains("fabricmc.net") {
        "fabric"
    } else if url.contains("neoforged.net") {
        "neoforge"
    } else if url.contains("minecraftforge.net") {
        "forge"
    } else if url.contains("mojang.com") {
        "mojang"
    } else if url.contains("adoptium.net") || url.contains("github.com") {
        "adoptium"
    } else {
        "network"
    }
}
