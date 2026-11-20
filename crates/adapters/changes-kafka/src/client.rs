//! How every consumer reaches the cluster: librdkafka's own properties
//! (`security.protocol`, `sasl.*`, `ssl.*`), and for SASL/OAUTHBEARER the
//! source librdkafka asks for each token before the last one expires.

use std::sync::Arc;

use rdkafka::client::{ClientContext, OAuthToken};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{ConsumerContext, StreamConsumer};
use shared::ports::credentials::BearerTokenSource;
use tokio::runtime::Handle;

/// The consumer every topic is read with.
pub(crate) type KafkaConsumer = StreamConsumer<Context>;

/// The properties every consumer sets, and the OAUTHBEARER token source.
#[derive(Clone, Default)]
pub struct KafkaClient {
    properties: Vec<(String, String)>,
    bearer: Option<Arc<dyn BearerTokenSource>>,
}

/// Shows every property but `sasl.password`'s value.
impl std::fmt::Debug for KafkaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown: Vec<(&str, &str)> = self
            .properties
            .iter()
            .map(|(k, v)| (k.as_str(), if k == "sasl.password" { "***" } else { v.as_str() }))
            .collect();
        f.debug_struct("KafkaClient").field("properties", &shown).field("bearer", &self.bearer).finish()
    }
}

impl KafkaClient {
    /// librdkafka `properties`, by their own names.
    pub fn new(properties: Vec<(String, String)>) -> Self {
        Self { properties, bearer: None }
    }

    /// SASL/OAUTHBEARER over TLS, with each token from `source`.
    #[must_use]
    pub fn with_bearer(mut self, source: Arc<dyn BearerTokenSource>) -> Self {
        self.properties.retain(|(k, _)| k != "security.protocol" && k != "sasl.mechanism");
        self.properties.push(("security.protocol".into(), "SASL_SSL".into()));
        self.properties.push(("sasl.mechanism".into(), "OAUTHBEARER".into()));
        self.bearer = Some(source);
        self
    }

    pub(crate) fn apply(&self, config: &mut ClientConfig) {
        for (key, value) in &self.properties {
            config.set(key, value);
        }
    }

    pub(crate) fn context(&self) -> Context {
        Context { bearer: self.bearer.clone(), runtime: Handle::current() }
    }
}

/// The consumers' context: it answers librdkafka's token refreshes.
pub(crate) struct Context {
    bearer: Option<Arc<dyn BearerTokenSource>>,
    runtime: Handle,
}

impl ClientContext for Context {
    const ENABLE_REFRESH_OAUTH_TOKEN: bool = true;

    /// librdkafka asks from whichever thread polls, a runtime worker among
    /// them, so the token is fetched on a thread of its own. Precondition: a
    /// multi-threaded runtime, whose other workers drive the fetch's IO while
    /// this one waits.
    fn generate_oauth_token(&self, _: Option<&str>) -> Result<OAuthToken, Box<dyn std::error::Error>> {
        let source = self.bearer.clone().ok_or("no OAUTHBEARER token source is configured")?;
        let runtime = self.runtime.clone();
        let token = std::thread::scope(|s| s.spawn(move || runtime.block_on(source.token())).join())
            .map_err(|_| "the OAUTHBEARER token fetch panicked")??;
        Ok(OAuthToken {
            token: token.value,
            principal_name: token.principal,
            lifetime_ms: token.expires_at_ms,
        })
    }
}

impl ConsumerContext for Context {}
