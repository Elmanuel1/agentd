//! The worker's side of the work API, with its retry and lease rules taken
//! from the SDK's environment worker.

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde_json::Value;

/// The beta every work call names.
const BETA: &str = "managed-agents-2026-04-01";

/// Where the worker reports and who it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// agentd's API, as the sandbox reaches it.
    pub api: String,
    /// The environment.
    pub environment: String,
    /// The work item.
    pub work: String,
    /// The work token.
    pub token: String,
}

impl Config {
    /// Read the worker's settings; the token comes from the work secret.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let need = |k: &str| get(k).filter(|v| !v.is_empty()).ok_or(format!("{k} is required"));
        let secret = need("AGENTD_WORK_SECRET")?;
        let padded = secret.trim_end_matches('=');
        let raw = URL_SAFE_NO_PAD.decode(padded).map_err(|e| format!("AGENTD_WORK_SECRET: {e}"))?;
        let payload: Value = serde_json::from_slice(&raw).map_err(|e| format!("AGENTD_WORK_SECRET: {e}"))?;
        let token = payload
            .get("sessions_token")
            .and_then(Value::as_str)
            .ok_or("AGENTD_WORK_SECRET: no sessions_token")?;
        Ok(Self {
            api: need("AGENTD_API_URL")?.trim_end_matches('/').to_owned(),
            environment: need("AGENTD_ENVIRONMENT_ID")?,
            work: need("AGENTD_WORK_ID")?,
            token: token.to_owned(),
        })
    }
}

/// Why the worker stopped holding its work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseEnd {
    /// agentd is stopping the work: the worker stops it in turn.
    Stopping,
    /// The lease is gone, so another holder may own the work now: the worker
    /// leaves it alone.
    Lost(String),
}

/// The lease's current heartbeat from a 412 answer.
fn current_heartbeat(body: &Value) -> Option<String> {
    let state = body.pointer("/error/current_state").or_else(|| body.pointer("/current_state"))?;
    state.get("last_heartbeat").and_then(Value::as_str).filter(|h| !h.is_empty()).map(str::to_owned)
}

/// Calls the work API.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    config: Config,
}

impl Client {
    /// A client for `config`.
    pub fn new(config: Config) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().unwrap_or_default();
        Self { http, config }
    }

    fn url(&self, tail: &str) -> String {
        let c = &self.config;
        format!("{}/v1/environments/{}/work/{}{tail}", c.api, c.environment, c.work)
    }

    fn post(&self, tail: &str) -> reqwest::RequestBuilder {
        self.with_headers(self.http.post(self.url(tail)))
    }

    fn with_headers(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.config.token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", BETA)
    }

    /// Acknowledge the work, retrying until agentd answers.
    pub async fn ack_until_done(&self) {
        let mut wait = Duration::from_millis(500);
        loop {
            match self.post("/ack").send().await {
                Ok(r) if r.status().is_success() => return,
                Ok(r) => eprintln!("agentd-worker: ack: HTTP {}", r.status()),
                Err(e) => eprintln!("agentd-worker: ack: {e}"),
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(30));
        }
    }

    /// Heartbeat until the lease ends; why it ended.
    pub async fn keep_lease(&self) -> LeaseEnd {
        let mut expected = "NO_HEARTBEAT".to_owned();
        let (mut interval, mut ttl) = (Duration::from_secs(30), Duration::from_secs(90));
        let mut last_ok = tokio::time::Instant::now();
        // Whether the last beat may have reached agentd without its answer
        // reaching us: then a 412 is our own stale `expected`, not a takeover.
        let mut unknown = false;
        loop {
            let sent =
                self.post("/heartbeat").query(&[("expected_last_heartbeat", expected.as_str())]).send().await;
            match sent {
                Ok(r) if r.status() == reqwest::StatusCode::PRECONDITION_FAILED => {
                    let current = r.json::<Value>().await.ok().and_then(|v| current_heartbeat(&v));
                    match current {
                        Some(current) if unknown => {
                            (expected, unknown) = (current, false);
                            continue;
                        }
                        _ => return LeaseEnd::Lost("the lease was lost".into()),
                    }
                }
                Ok(r)
                    if r.status().is_client_error()
                        && r.status() != reqwest::StatusCode::TOO_MANY_REQUESTS =>
                {
                    return LeaseEnd::Lost(format!("heartbeat refused: HTTP {}", r.status()));
                }
                Ok(r) if r.status().is_success() => {
                    unknown = false;
                    let v: Value = r.json().await.unwrap_or_default();
                    let state = v.get("state").and_then(Value::as_str).unwrap_or_default();
                    if matches!(state, "stopping" | "stopped") {
                        return LeaseEnd::Stopping;
                    }
                    if v.get("lease_extended") == Some(&Value::Bool(false)) {
                        return LeaseEnd::Lost("the lease was not extended".into());
                    }
                    expected = v.get("last_heartbeat").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let secs = v.get("ttl_seconds").and_then(Value::as_u64).unwrap_or(90).max(1);
                    ttl = Duration::from_secs(secs);
                    interval = Duration::from_secs_f64((secs as f64 / 2.0).clamp(1.0, 30.0));
                    last_ok = tokio::time::Instant::now();
                }
                Ok(r) => {
                    unknown = true;
                    eprintln!("agentd-worker: heartbeat: HTTP {}", r.status());
                }
                Err(e) => {
                    unknown = true;
                    eprintln!("agentd-worker: heartbeat: {e}");
                }
            }
            if last_ok.elapsed() > ttl {
                return LeaseEnd::Lost("the lease ran out".into());
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// Force-stop the work on the way out.
    pub async fn stop(&self) {
        let _ = self.post("/stop").json(&serde_json::json!({"force": true})).send().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_comes_from_the_work_secret() {
        let secret = URL_SAFE_NO_PAD.encode(r#"{"sessions_token":"agentd-wt-x.y"}"#);
        let env = |k: &str| match k {
            "AGENTD_WORK_SECRET" => Some(secret.clone()),
            "AGENTD_API_URL" => Some("http://agentd:8080/".into()),
            "AGENTD_ENVIRONMENT_ID" => Some("env_1".into()),
            "AGENTD_WORK_ID" => Some("work_1".into()),
            _ => None,
        };
        let c = Config::from_env(env).unwrap();
        assert_eq!((c.token.as_str(), c.api.as_str()), ("agentd-wt-x.y", "http://agentd:8080"));
        assert!(Config::from_env(|_| None).is_err());
    }
}
