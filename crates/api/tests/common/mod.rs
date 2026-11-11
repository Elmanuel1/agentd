//! The work API rig: the composed router over live Postgres and LocalStack's
//! object store, real environment keys with a runtime key, and a work service
//! on the same stores to queue items as the loop will. Each rig has its own
//! org and workspaces. Gated by `AGENTD_PG_TESTS=1` and `AGENTD_AWS_ENDPOINT`.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use api::session_grants::SessionReach;
use api::{build_router, AppState, Backends, Stores, SystemKeys};
use api_keys::EnvironmentKeysService;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use blobs_s3::{BlobStore, S3BlobStore, S3Config};
use http_body_util::BodyExt;
use sessions::signals::{LocalSignals, Signals};
use sessions::steps::LoopWorkspaces;
use sessions::store::DatabaseLoad;
use shared::ports::fire::FireRegistry;
use shared::ports::keys::material::{self, KeyKind};
use shared::ports::keys::KeyPepper;
use shared::ports::secrets::{ExposeSecret, SecretString};
use shared::ports::{FakeRuntime, FakeScheduler};
use shared::{AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::PgBackend;
use test_support::{MemorySecretStore, StaticAdminKeys, StaticKeySource};
use tower::ServiceExt;
use vaults::egress::VaultCredentials;
use work::{CloudVariables, Placeholders, WorkService, WorkTokens};

pub const KEY: &str = "test-key-work-a";
pub const KEY_B: &str = "test-key-work-b";
pub const WORK_KEY: [u8; 32] = [1; 32];
pub const EGRESS_KEY: [u8; 32] = [2; 32];
/// Live's 403 for an environment key off its routes.
pub const SCOPE: &str = "OAuth token does not meet scope requirement any_of(org:sessions:claude_code, \
                         user:developer, user:sessions:claude_code, workspace:developer)";

pub fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1") && std::env::var("AGENTD_AWS_ENDPOINT").is_ok()
}

pub fn stamp() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!("{}{}", Timestamp::now().as_datetime().timestamp_micros(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A database load that never refuses a stream.
pub struct Unloaded;

impl DatabaseLoad for Unloaded {
    fn overloaded(&self) -> bool {
        false
    }
}

/// How one app replica streams: its signals and the load it reads.
pub struct Streaming {
    pub signals: Arc<dyn Signals>,
    pub load: Arc<dyn DatabaseLoad>,
    /// The workspaces whose sessions the loop serves.
    pub on_loop: LoopWorkspaces,
}

impl Default for Streaming {
    /// Signals in this app alone, never overloaded.
    fn default() -> Self {
        Self {
            signals: Arc::new(LocalSignals::default()),
            load: Arc::new(Unloaded),
            on_loop: LoopWorkspaces::default(),
        }
    }
}

fn app(
    stores: &Stores,
    keys: Arc<StaticKeySource>,
    env_keys: &EnvironmentKeysService,
    sandbox: bool,
    streaming: Streaming,
) -> Router {
    let backends = Backends {
        stores: stores.clone(),
        keys,
        admin_keys: Arc::new(StaticAdminKeys::default()),
        environment_keys: Arc::new(env_keys.clone()),
        system: SystemKeys {
            work_token_key: WORK_KEY.to_vec(),
            egress_key: EGRESS_KEY.to_vec(),
            api_base_url: "http://agentd.test".into(),
        },
        scheduler: Arc::new(FakeScheduler::new()),
        fires: Arc::new(FireRegistry::new()),
        runtime: Some(Arc::new(FakeRuntime::new())),
        sandbox: sandbox.then(api::config::SandboxConfig::default),
        extra_models: Vec::new(),
        metrics: Arc::new(api::telemetry::Metrics::global()),
        signals: streaming.signals,
        load: streaming.load,
        on_loop: streaming.on_loop,
    };
    build_router(AppState::build(backends).unwrap())
}

/// How a request authenticates.
#[derive(Clone, Copy)]
pub enum Auth<'a> {
    /// `x-api-key`.
    Key(&'a str),
    /// `Authorization: Bearer`.
    Bearer(&'a str),
}

pub struct Rig {
    pub app: Router,
    pub stores: Stores,
    pub keys: Arc<StaticKeySource>,
    pub sandbox: bool,
    pub env_keys: EnvironmentKeysService,
    pub runtime_key: SecretString,
    pub work: Arc<WorkService>,
    pub secrets: Arc<MemorySecretStore>,
    pub org: String,
    pub ws: String,
    pub ws_b: String,
}

impl Rig {
    /// Sessions get no sandbox, so no hosted work item: the tests queue items.
    pub async fn new() -> Option<Self> {
        Self::with(false).await
    }

    /// Each session's sandbox starts on a fake runtime with its hosted work item.
    pub async fn with_sandbox() -> Option<Self> {
        Self::with(true).await
    }

    async fn with(sandbox: bool) -> Option<Self> {
        if !enabled() {
            return None;
        }
        let url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
        let backend = PgBackend::connect(&url).await.unwrap();
        let s = stamp();
        let blobs: Arc<dyn BlobStore> = Arc::new(
            S3BlobStore::connect(&S3Config {
                endpoint: std::env::var("AGENTD_AWS_ENDPOINT").ok(),
                path_style: true,
                create_bucket: true,
                ..S3Config::new(format!("agentdwork{s}"))
            })
            .await
            .unwrap(),
        );
        let (org, ws, ws_b) = (format!("org_w{s}"), format!("wrkspc_a{s}"), format!("wrkspc_b{s}"));
        let mut keys = StaticKeySource::default();
        keys.register(KEY, &org, &ws, "apikey_01a");
        keys.register(KEY_B, &org, &ws_b, "apikey_01b");
        let secrets = Arc::new(MemorySecretStore::default());
        let stores = Stores::postgres(backend, blobs, secrets.clone());
        let runtime_key = material::generate(KeyKind::Environment);
        let env_keys =
            EnvironmentKeysService::new(stores.environment_keys.clone(), Arc::new(KeyPepper::new(&[9; 32])))
                .with_runtime_key(&runtime_key);
        let keys = Arc::new(keys);
        let app = app(&stores, keys.clone(), &env_keys, sandbox, Streaming::default());
        let reach = Arc::new(SessionReach {
            sessions: stores.sessions.clone(),
            vaults: VaultCredentials::new(stores.vault_credentials.clone()),
        });
        let cloud = CloudVariables { source: reach, placeholders: Placeholders::new(EGRESS_KEY.to_vec()) };
        let work = Arc::new(
            WorkService::new(stores.work.clone(), WorkTokens::new(WORK_KEY.to_vec()), "http://agentd.test")
                .with_cloud_variables(cloud),
        );
        Some(Self { app, stores, keys, env_keys, runtime_key, work, secrets, org, ws, ws_b, sandbox })
    }

    /// A fresh app over the same database and keys, as after a restart.
    pub fn restarted(&self) -> Router {
        app(&self.stores, self.keys.clone(), &self.env_keys, self.sandbox, Streaming::default())
    }

    /// Another replica over `stores`, streaming as `streaming` says.
    pub fn replica(&self, stores: &Stores, streaming: Streaming) -> Router {
        app(stores, self.keys.clone(), &self.env_keys, self.sandbox, streaming)
    }

    pub fn ctx(&self) -> AuthContext {
        AuthContext {
            org: OrgId::new(&self.org),
            workspace: WorkspaceId::new(&self.ws),
            api_key_id: "apikey_01a".into(),
        }
    }

    pub fn runtime(&self) -> &str {
        self.runtime_key.expose_secret()
    }

    /// A key for `environment` in the rig's workspace.
    pub async fn environment_key(&self, environment: &str) -> String {
        let (_, key) = self.env_keys.create(&self.org, &self.ws, environment, "worker").await.unwrap();
        key.expose_secret().to_owned()
    }

    pub async fn call(
        &self,
        method: &str,
        uri: &str,
        auth: Auth<'_>,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        self.send(request(method, uri, auth, body)).await
    }

    pub async fn send(&self, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        send_to(&self.app, req).await
    }
}

/// A request as the SDKs send it.
pub fn request(method: &str, uri: &str, auth: Auth<'_>, body: Option<serde_json::Value>) -> Request<Body> {
    let beta = if uri.starts_with("/v1/memory_stores") {
        "agent-memory-2026-07-22"
    } else {
        "managed-agents-2026-04-01"
    };
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", beta);
    let b = match auth {
        Auth::Key(k) => b.header("x-api-key", k),
        Auth::Bearer(k) => b.header("authorization", format!("Bearer {k}")),
    };
    b.body(body.map_or_else(Body::empty, |v| Body::from(v.to_string()))).unwrap()
}

/// Send `req` to `app`: its status and JSON body (or text).
pub async fn send_to(app: &Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    {
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, v)
    }
}

impl Rig {
    /// `POST /v1/files` as `auth`, `scope_id` set when given.
    pub async fn upload(
        &self,
        auth: Auth<'_>,
        name: &str,
        bytes: &str,
        scope_id: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let boundary = "agentdboundary";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: text/plain\r\n\r\n{bytes}\r\n"
        );
        if let Some(scope) = scope_id {
            body += &format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"scope_id\"\r\n\r\n{scope}\r\n"
            );
        }
        body += &format!("--{boundary}--\r\n");
        let mut b = Request::builder()
            .method("POST")
            .uri("/v1/files")
            .header("content-type", format!("multipart/form-data; boundary={boundary}"))
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "files-api-2025-04-14");
        b = match auth {
            Auth::Key(k) => b.header("x-api-key", k),
            Auth::Bearer(k) => b.header("authorization", format!("Bearer {k}")),
        };
        self.send(b.body(Body::from(body)).unwrap()).await
    }

    /// A `self_hosted` or `cloud` environment and an agent, by the rig's workspace key.
    pub async fn environment(&self, kind: &str) -> String {
        let body = serde_json::json!({"name": format!("w-{}", stamp()), "config": {"type": kind}});
        let (st, env) = self.call("POST", "/v1/environments", Auth::Key(KEY), Some(body)).await;
        assert_eq!(st, StatusCode::OK, "{env}");
        env["id"].as_str().unwrap().to_owned()
    }

    /// A session of a fresh agent in `environment`, with `extra` merged into the create body.
    pub async fn session(&self, environment: &str, extra: serde_json::Value) -> String {
        let agent_body = serde_json::json!({"name": format!("a-{}", stamp()), "model": "claude-sonnet-4-6"});
        let (_, agent) = self.call("POST", "/v1/agents", Auth::Key(KEY), Some(agent_body)).await;
        let mut body = serde_json::json!({"agent": agent["id"], "environment_id": environment});
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        let (st, s) = self.call("POST", "/v1/sessions", Auth::Key(KEY), Some(body)).await;
        assert_eq!(st, StatusCode::OK, "{s}");
        s["id"].as_str().unwrap().to_owned()
    }

    /// The work token of `session`, as its work secret carries it.
    pub fn token(&self, session: &str) -> String {
        WorkTokens::new(WORK_KEY.to_vec()).mint(&work::WorkClaims {
            session: session.into(),
            ws: self.ws.clone(),
            org: self.org.clone(),
        })
    }
}
