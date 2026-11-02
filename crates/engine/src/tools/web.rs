//! `web_fetch` and `web_search`, which agentd runs itself (as the hosted
//! toolset does), under the environment's network policy. Fetches to private
//! and loopback addresses are refused, so the tool cannot reach agentd's own
//! network.

use std::net::IpAddr;
use std::time::Duration;

use serde_json::Value;

use super::ToolOutcome;

/// Most redirects one fetch follows.
const MAX_REDIRECTS: usize = 5;

/// Most bytes of a fetched page returned.
const MAX_BODY: usize = 1_000_000;

/// The HTTP client the web tools share.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder().user_agent("agentd").build().unwrap_or_default()
}

/// The web tools for one session.
#[derive(Debug, Clone, Default)]
pub struct WebTools {
    http: Option<reqwest::Client>,
    /// Hosts a `limited` environment allows; `None` when unrestricted.
    allowed_hosts: Option<Vec<String>>,
    /// A Brave Search API key, when web search is configured.
    search_key: Option<String>,
}

impl WebTools {
    /// Web tools under `allowed_hosts` (`None`: unrestricted), searching with
    /// `search_key` when set.
    pub fn new(
        http: reqwest::Client,
        allowed_hosts: Option<Vec<String>>,
        search_key: Option<String>,
    ) -> Self {
        Self { http: Some(http), allowed_hosts, search_key }
    }

    /// Whether `host` passes the environment's allow-list: a bare host matches
    /// itself only; `*.example.com` matches its subdomains, not the apex.
    pub fn host_allowed(&self, host: &str) -> bool {
        let Some(allowed) = &self.allowed_hosts else {
            return true;
        };
        allowed.iter().any(|rule| match rule.strip_prefix("*.") {
            Some(apex) => host.len() > apex.len() + 1 && host.ends_with(&format!(".{apex}")),
            None => rule.eq_ignore_ascii_case(host),
        })
    }

    pub(super) async fn fetch(&self, input: &Value) -> ToolOutcome {
        let Some(raw) = crate::json::at(input, "/url").as_str() else {
            return ToolOutcome::err("web_fetch: `url` is required");
        };
        let url = match reqwest::Url::parse(raw) {
            Ok(u) if matches!(u.scheme(), "http" | "https") => u,
            _ => return ToolOutcome::err(format!("web_fetch: {raw} is not an http(s) URL")),
        };
        if self.http.is_none() {
            return ToolOutcome::err("web_fetch is not available on this server");
        }
        let mut url = url;
        let mut hops = 0;
        let resp = loop {
            let host = url.host_str().unwrap_or_default().to_owned();
            if !self.host_allowed(&host) {
                return ToolOutcome::err(format!(
                    "web_fetch: {host} is not in the environment's allowed hosts"
                ));
            }
            // Connect to the addresses just checked, so a second lookup cannot
            // swap in an internal one, and check every redirect the same way.
            let addrs = match public_addrs(&host, url.port_or_known_default().unwrap_or(443)).await {
                Ok(a) => a,
                Err(e) => return ToolOutcome::err(format!("web_fetch: {e}")),
            };
            let pinned = reqwest::Client::builder()
                .user_agent("agentd")
                .redirect(reqwest::redirect::Policy::none())
                .resolve_to_addrs(&host, &addrs)
                .timeout(Duration::from_secs(30))
                .build();
            let sent = match pinned {
                Ok(c) => c.get(url.clone()).send().await,
                Err(e) => return ToolOutcome::err(format!("web_fetch: {e}")),
            };
            let resp = match sent {
                Ok(r) => r,
                Err(e) => return ToolOutcome::err(format!("web_fetch: {e}")),
            };
            let next = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| url.join(l).ok());
            match next {
                Some(next) if resp.status().is_redirection() && hops < MAX_REDIRECTS => {
                    hops += 1;
                    url = next;
                }
                _ => break resp,
            }
        };
        let status = resp.status();
        let html = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.contains("html"));
        let body = match resp.bytes().await {
            Ok(b) => String::from_utf8_lossy(b.get(..MAX_BODY.min(b.len())).unwrap_or(&b)).into_owned(),
            Err(e) => return ToolOutcome::err(format!("web_fetch: {e}")),
        };
        let text = if html { strip_tags(&body) } else { body };
        if status.is_success() {
            ToolOutcome::ok(text)
        } else {
            ToolOutcome::err(format!("web_fetch: HTTP {status}\n{text}"))
        }
    }

    pub(super) async fn search(&self, input: &Value) -> ToolOutcome {
        let (Some(http), Some(key)) = (&self.http, &self.search_key) else {
            return ToolOutcome::err("web_search is not configured on this server");
        };
        let Some(query) = crate::json::at(input, "/query").as_str() else {
            return ToolOutcome::err("web_search: `query` is required");
        };
        let resp = http
            .get("https://api.search.brave.com/res/v1/web/search")
            .query(&[("q", query)])
            .header("X-Subscription-Token", key)
            .timeout(Duration::from_secs(30))
            .send()
            .await;
        let v: Value = match resp {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(e) => return ToolOutcome::err(format!("web_search: {e}")),
        };
        let lines: Vec<String> = crate::json::at(&v, "/web/results")
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                format!(
                    "{}\n{}\n{}",
                    crate::json::at(r, "/title").as_str().unwrap_or(""),
                    crate::json::at(r, "/url").as_str().unwrap_or(""),
                    crate::json::at(r, "/description").as_str().unwrap_or("")
                )
            })
            .collect();
        ToolOutcome::ok(if lines.is_empty() { "No results".to_owned() } else { lines.join("\n\n") })
    }
}

/// `host`'s addresses, refused when any is loopback, private, link-local or
/// otherwise internal.
async fn public_addrs(host: &str, port: u16) -> Result<Vec<std::net::SocketAddr>, String> {
    let addrs: Vec<_> =
        tokio::net::lookup_host((host, port)).await.map_err(|e| format!("{host}: {e}"))?.collect();
    for addr in &addrs {
        let internal = match addr.ip() {
            IpAddr::V4(ip) => {
                ip.is_private()
                    || ip.is_loopback()
                    || ip.is_link_local()
                    || ip.is_unspecified()
                    || ip.is_broadcast()
            }
            IpAddr::V6(ip) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || (ip.segments()[0] & 0xfe00) == 0xfc00
                    || (ip.segments()[0] & 0xffc0) == 0xfe80
            }
        };
        if internal {
            return Err(format!("{host} resolves to an internal address"));
        }
    }
    Ok(addrs)
}

/// The visible text of an HTML page, roughly: tags, scripts and styles out.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut skip_until: Option<&str> = None;
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while let Some(c) = html.get(i..).and_then(|s| s.chars().next()) {
        if let Some(end) = skip_until {
            if lower.get(i..).is_some_and(|s| s.starts_with(end)) {
                skip_until = None;
            }
        } else if c == '<' {
            in_tag = true;
            if lower.get(i..).is_some_and(|s| s.starts_with("<script")) {
                skip_until = Some("</script");
            } else if lower.get(i..).is_some_and(|s| s.starts_with("<style")) {
                skip_until = Some("</style");
            }
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
        i += c.len_utf8();
    }
    out.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests;
