//! Reads a provider's list (list prices, the models agents may name) at
//! start and once a day, for as long as the process runs. A read that fails,
//! or finds nothing, keeps the list last installed and is tried again a
//! minute later.

use std::future::Future;
use std::time::Duration;

use serde_json::Value;

/// How long after a good read the list is read again.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long after a failed or empty read the list is read again.
const AGAIN: Duration = Duration::from_secs(60);
/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The largest response read; OpenRouter's model list is about 1 MB.
const MAX_BYTES: usize = 16 << 20;

/// What a read found, which is installed only when it has entries.
pub(super) trait Listed: Send + 'static {
    fn is_empty(&self) -> bool;
}

impl Listed for Vec<String> {
    fn is_empty(&self) -> bool {
        <[String]>::is_empty(self)
    }
}

/// Read `list` with `read` now and on the schedule above, and hand each read
/// that has entries to `install`.
pub(super) fn spawn<T, F>(
    list: &'static str,
    read: impl Fn(reqwest::Client) -> F + Send + 'static,
    install: impl Fn(T) + Send + 'static,
) -> tokio::task::JoinHandle<()>
where
    T: Listed,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    tokio::spawn(async move {
        let http = match reqwest::Client::builder().timeout(TIMEOUT).build() {
            Ok(http) => http,
            Err(e) => {
                tracing::warn!(list, error = %e, "no HTTP client; the list stays as installed");
                return;
            }
        };
        loop {
            let wait = match read(http.clone()).await {
                Ok(found) if !found.is_empty() => {
                    tracing::debug!(list, "the list read");
                    install(found);
                    EVERY
                }
                Ok(_) => {
                    tracing::warn!(list, "the list is empty; it stays as installed");
                    AGAIN
                }
                Err(e) => {
                    tracing::warn!(list, error = %e, "the list cannot be read; it stays as installed");
                    AGAIN
                }
            };
            tokio::time::sleep(wait).await;
        }
    })
}

/// The JSON body of `req`'s response, refused over [`MAX_BYTES`].
pub(super) async fn json(req: reqwest::RequestBuilder) -> Result<Value, String> {
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > MAX_BYTES {
        return Err(format!("the response is over {MAX_BYTES} bytes"));
    }
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_bad_read_is_tried_again_after_a_minute_and_a_good_one_after_a_day() {
        let (calls, installed) = (Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(Vec::new())));
        let (counted, kept) = (calls.clone(), installed.clone());
        let read = move |_: reqwest::Client| {
            let n = counted.fetch_add(1, Ordering::SeqCst);
            async move {
                match n {
                    0 => Err("down".to_owned()),
                    1 => Ok(Vec::new()),
                    _ => Ok(vec![format!("m{n}")]),
                }
            }
        };
        drop(spawn("test", read, move |found| kept.lock().unwrap().push(found)));
        tokio::time::sleep(AGAIN * 2 + Duration::from_secs(1)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 3, "a failed and an empty read are each tried again");
        assert_eq!(*installed.lock().unwrap(), [vec!["m2".to_owned()]], "only the read with entries");
        tokio::time::sleep(EVERY - AGAIN).await;
        assert_eq!(calls.load(Ordering::SeqCst), 3, "a good read waits a day");
        tokio::time::sleep(AGAIN).await;
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }
}
