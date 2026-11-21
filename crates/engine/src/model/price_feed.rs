//! Keeps this replica's list prices current from a model list
//! (`AGENTD_MODEL_PRICES_URL`): read at start and every five minutes. A read
//! that fails keeps the prices last read, or the built-in table before any.

use std::time::Duration;

use shared::prices::{Feed, install};

/// How often the list is read again.
const EVERY: Duration = Duration::from_secs(300);
/// How long one read may take.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The largest list read; the public list is about 1 MB.
const MAX_BYTES: usize = 16 << 20;

/// Read the list at `url` now and every five minutes, for as long as the
/// process runs.
pub fn spawn(url: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let http = match reqwest::Client::builder().timeout(TIMEOUT).build() {
            Ok(http) => http,
            Err(e) => {
                tracing::warn!(error = %e, "prices: no HTTP client; the built-in list prices stay");
                return;
            }
        };
        loop {
            match read(&http, &url).await {
                Ok(feed) if !feed.is_empty() => {
                    tracing::debug!(url = %url, "prices: list prices read");
                    install(feed);
                }
                Ok(_) => {
                    tracing::warn!(url = %url, "prices: the list prices no Anthropic model; prices stay")
                }
                Err(e) => {
                    tracing::warn!(url = %url, error = %e, "prices: the list cannot be read; prices stay")
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    })
}

async fn read(http: &reqwest::Client, url: &str) -> Result<Feed, String> {
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > MAX_BYTES {
        return Err(format!("the list is over {MAX_BYTES} bytes"));
    }
    let list: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(Feed::parse(&list))
}
