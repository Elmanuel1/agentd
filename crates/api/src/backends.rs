//! The composition root: one backend per port, selected by [`Config`].
//! Adding a backend is a new config value and one match arm here; services
//! and routes see only the ports.

use std::sync::Arc;

use blobs_s3::{BlobError, S3BlobStore};

mod feed;
mod postgres;
mod scheduler;
mod secrets;
mod stores;
mod webhooks;

pub use postgres::FireSources;
pub use stores::Stores;

use crate::telemetry::ports::Observed;
use crate::telemetry::scheduler::SchedulerMetrics;
use crate::telemetry::{self, Metrics};
use api_keys::{AdminKeysService, ApiKeysService, CachedKeys, EnvironmentKeysService, KeyRevokes, REVOKED};
use sessions::signals::Signals;
use sessions::store::DatabaseLoad;
use shared::hash::KeyRing;
use shared::ports::blobs::BlobStore;
use shared::ports::broadcast::Broadcast;
use shared::ports::fire::FireRegistry;
use shared::ports::keys::{AdminKeySource, EnvironmentKeySource, KeySource, PepperRing};
use shared::ports::secrets::SecretError;
use signals_valkey::{Valkey, ValkeyBroadcast, ValkeyRateCounter, ValkeySignals};
use store_postgres::{PgError, PgFireClaims, PgHistoryLog, ScheduleOutbox};

use crate::config::{BlobsConfig, Config, Role, StoreConfig, WorkConfig};
use crate::shutdown::Shutdown;

/// One backend per port.
pub struct Backends {
    /// The entity stores.
    pub stores: Stores,
    /// Workspace key resolution.
    pub keys: Arc<dyn KeySource>,
    /// Admin key resolution.
    pub admin_keys: Arc<dyn AdminKeySource>,
    /// Environment key resolution, agentd's runtime key among them.
    pub environment_keys: Arc<dyn EnvironmentKeySource>,
    /// The keys work tokens and placeholders are signed with.
    pub system: SystemKeys,
    /// Where the schedulers deliver fires; the modules register their handlers here.
    pub fires: Arc<FireRegistry>,
    /// What delivers fires, started once the handlers are registered; none
    /// in a rig that fires nothing.
    pub fire_sources: Option<FireSources>,
    /// Model ids agents may name beyond the built-in ones (`AGENTD_MODELS`).
    pub extra_models: Vec<String>,
    /// The instruments every boundary records into.
    pub metrics: Arc<Metrics>,
    /// How replicas tell each other a session changed.
    pub signals: Arc<dyn Signals>,
    /// The database's load, which new streams wait on.
    pub load: Arc<dyn DatabaseLoad>,
    /// What this replica runs; a `loop` replica serves no route.
    pub role: Role,
    /// Where agentd itself may connect for a tenant: MCP servers, token endpoints.
    pub guard: shared::net::Guard,
    /// Each organization's webhook endpoints.
    pub webhooks: Arc<::webhooks::Webhooks>,
    /// What stops at SIGTERM, and the work that drains after it.
    pub shutdown: Shutdown,
    /// What the edge applies to every module.
    pub edge: crate::edge::EdgeSettings,
}

/// The deployment's own keys, from its configuration.
#[derive(Clone)]
pub struct SystemKeys {
    /// Signs and verifies work tokens.
    pub work_tokens: KeyRing,
    /// Signs and verifies `environment_variable` placeholders.
    pub placeholders: KeyRing,
    /// agentd's API as a work secret's `api_base_url` names it.
    pub api_base_url: String,
}

impl From<&WorkConfig> for SystemKeys {
    fn from(work: &WorkConfig) -> Self {
        Self {
            work_tokens: work.work_tokens.clone(),
            placeholders: work.placeholders.clone(),
            api_base_url: work.api_base_url.clone(),
        }
    }
}

impl std::fmt::Debug for SystemKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemKeys").field("api_base_url", &self.api_base_url).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Backends {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backends").field("fires", &self.fires).finish_non_exhaustive()
    }
}

/// A backend that could not be reached at boot.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// The record store.
    #[error("store: {0}")]
    Store(#[from] PgError),
    /// The blob store.
    #[error("blobs: {0}")]
    Blobs(#[from] BlobError),
    /// The secret store, or its KMS key.
    #[error("secrets: {0}")]
    Secrets(#[from] SecretError),
    /// The Valkey the replicas signal through.
    #[error("signals: {0}")]
    Signals(String),
    /// The loop's model providers or budget.
    #[error("loop: {0}")]
    Loop(String),
    /// The scheduler sink.
    #[error("scheduler: {0}")]
    Scheduler(String),
}

impl Backends {
    /// Connect the backends `config` selects. The scheduler and its outbox
    /// relay start here.
    pub async fn from_config(config: &Config) -> Result<Self, BootError> {
        let metrics = Arc::new(Metrics::global());
        let role = config.agent_loop.role;
        let blobs: Arc<dyn BlobStore> = match &config.blobs {
            BlobsConfig::S3(s3) => Arc::new(S3BlobStore::connect(s3).await?),
        };
        let blobs: Arc<dyn BlobStore> = Arc::new(Observed::new(blobs, metrics.clone()));
        let secrets = secrets::Secrets::connect(&config.secrets, &metrics).await?;
        let peppers = Arc::new(PepperRing::from_ring(&config.keys.peppers));
        let streams = &config.streams;
        let valkey = Valkey::open(&streams.valkey_url, streams.valkey_cluster)
            .map_err(|e| BootError::Signals(e.to_string()))?;
        let broadcast =
            Arc::new(ValkeyBroadcast::connect(&valkey).map_err(|e| BootError::Signals(e.to_string()))?);
        let mut shutdown = Shutdown::default();
        let counter = ValkeyRateCounter::connect(&valkey).map_err(|e| BootError::Signals(e.to_string()))?;
        let limits = crate::edge::rate::Limits::new(Arc::new(counter));
        let (stores, load, keys, fires, sources) = match &config.store {
            StoreConfig::Postgres { url, read_url, pool } => {
                let retention = config.streams.retention.filter(|_| role.serves_api());
                let hooks = config.webhooks.clone();
                let outside = postgres::Outside { blobs, secrets };
                let pg =
                    postgres::connect(url, read_url.as_deref(), *pool, retention, &metrics, outside).await?;
                let queue = PgFireClaims::new(pg.backend.clone());
                let meter = opentelemetry::global::meter("agentd");
                let scheduling = SchedulerMetrics::new(&meter);
                let delivered = scheduling.clone();
                let registry = FireRegistry::new(Arc::new(queue.clone()));
                let fires = Arc::new(registry.counting_enqueued(move |n| delivered.enqueued(n)));
                let sink = scheduler::Sink::connect(&config.scheduler, &pg.backend).await?;
                let feed = feed::ChangeFeed::from_config(streams, &valkey);
                let instance = format!("agentd-{}", uuid::Uuid::new_v4());
                let sending = telemetry::webhooks::WebhookMetrics::on(&meter, &instance);
                let wake = webhooks::start(hooks, &pg.backend, broadcast.clone(), sending, &mut shutdown);
                drop(telemetry::feed::watch(&meter, pg.backend.diesel.clone(), &streams.changes_slot));
                let log = Arc::new(PgHistoryLog::load(pg.backend.clone()).await?);
                let mut handlers = history::recorders(log, telemetry::feed::apply_delay(&meter));
                handlers.push(Arc::new(ScheduleOutbox::new(sink.port())));
                handlers.push(wake);
                handlers.push(Arc::new(KeyRevokes::new(broadcast.clone())));
                drop(feed.spawn(handlers));
                let keys = ApiKeysService::new(pg.stores.api_keys.clone(), peppers.clone());
                let keys: Arc<dyn KeySource> = Arc::new(Observed::new(Arc::new(keys), metrics.clone()));
                let keys = Arc::new(CachedKeys::new(keys, Arc::new(telemetry::edge::key_cache)));
                drop(keys.clone().listen(broadcast.subscribe(REVOKED)));
                (pg.stores, pg.load, keys, fires, FireSources::new(sink, feed, queue, scheduling))
            }
        };
        let signals: Arc<dyn Signals> =
            Arc::new(ValkeySignals::connect(&valkey).map_err(|e| BootError::Signals(e.to_string()))?);
        let admin = Arc::new(AdminKeysService::new(stores.admin_keys.clone(), peppers.clone()));
        let admin_keys: Arc<dyn AdminKeySource> = Arc::new(Observed::new(admin, metrics.clone()));
        let environment_keys = EnvironmentKeysService::new(stores.environment_keys.clone(), peppers)
            .with_runtime_keys(&config.keys.runtime);
        let system = SystemKeys::from(&config.work);
        if role.runs_loop() {
            let stop = shutdown.signal();
            shutdown.drain(crate::agent_loop::spawn(config, &stores, signals.clone(), &system, stop).await?);
        }
        let extra_models = config.extra_models.clone();
        let environment_keys: Arc<dyn EnvironmentKeySource> = Arc::new(environment_keys);
        Ok(Self {
            stores,
            keys,
            admin_keys,
            environment_keys,
            system,
            fires,
            fire_sources: Some(sources),
            extra_models,
            metrics,
            signals,
            load,
            role,
            guard: shared::net::Guard::default(),
            webhooks: config.webhooks.clone(),
            shutdown,
            edge: crate::edge::EdgeSettings { request_timeout: config.request_timeout, limits: Some(limits) },
        })
    }
}
