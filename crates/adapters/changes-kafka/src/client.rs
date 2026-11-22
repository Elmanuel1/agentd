//! How every consumer reaches the cluster: librdkafka's own properties
//! (`security.protocol`, `sasl.*`, `ssl.*`), and for SASL/OAUTHBEARER the
//! token librdkafka asks for before the last one expires. librdkafka asks from
//! whichever thread polls, a runtime worker among them, so it is handed the
//! token already fetched, and a task fetches the next: nothing waits on the
//! runtime from inside the poll.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rdkafka::client::{ClientContext, OAuthToken};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{ConsumerContext, StreamConsumer};
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use shared::BackendError;
use shared::ports::credentials::{BearerToken, BearerTokenSource};
use tokio::runtime::Handle;

/// The consumer every topic is read with.
pub(crate) type KafkaConsumer = StreamConsumer<Context>;

/// The properties every consumer sets, and the OAUTHBEARER token source.
#[derive(Clone, Default)]
pub struct KafkaClient {
    properties: Vec<(String, String)>,
    bearer: Option<Arc<Tokens>>,
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
        self.bearer = Some(Arc::new(Tokens { source, last: Mutex::new(None) }));
        self
    }

    /// A token in hand before a consumer starts, so librdkafka's first ask is answered.
    pub(crate) async fn ready(&self) -> Result<(), BackendError> {
        match &self.bearer {
            Some(tokens) if tokens.current().is_none() => tokens.fetch().await,
            _ => Ok(()),
        }
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

/// The OAUTHBEARER source and the last token it gave.
#[derive(Debug)]
pub(crate) struct Tokens {
    source: Arc<dyn BearerTokenSource>,
    last: Mutex<Option<BearerToken>>,
}

impl Tokens {
    /// The last token, while it has not expired.
    fn current(&self) -> Option<BearerToken> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        let last = self.last.lock().unwrap_or_else(PoisonError::into_inner).clone();
        last.filter(|t| u128::try_from(t.expires_at_ms).unwrap_or_default() > now)
    }

    async fn fetch(&self) -> Result<(), BackendError> {
        let token = self.source.token().await?;
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(token);
        Ok(())
    }
}

/// The consumers' context: it answers librdkafka's token refreshes.
pub(crate) struct Context {
    bearer: Option<Arc<Tokens>>,
    runtime: Handle,
}

impl ClientContext for Context {
    const ENABLE_REFRESH_OAUTH_TOKEN: bool = true;

    /// A subscribed topic that does not exist yet is a table with no change
    /// yet, which the reader waits out, not a failure.
    fn error(&self, error: KafkaError, reason: &str) {
        match error {
            KafkaError::Global(RDKafkaErrorCode::UnknownTopicOrPartition) => {
                tracing::debug!(%error, reason, "changes: librdkafka: a subscribed topic does not exist yet")
            }
            _ => tracing::error!(%error, reason, "changes: librdkafka error"),
        }
    }

    /// librdkafka asks again at 80% of the lifetime it is given, and after a
    /// refused ask, so the token fetched now is in hand by the next ask, also
    /// when the last fetch failed and the cached token ran out.
    fn generate_oauth_token(&self, _: Option<&str>) -> Result<OAuthToken, Box<dyn std::error::Error>> {
        let tokens = self.bearer.clone().ok_or("no OAUTHBEARER token source is configured")?;
        let fetching = tokens.clone();
        self.runtime.spawn(async move {
            if let Err(e) = fetching.fetch().await {
                tracing::warn!(error = %e, "changes: the next OAUTHBEARER token could not be fetched");
            }
        });
        let token = tokens.current().ok_or("no OAUTHBEARER token yet")?;
        Ok(OAuthToken {
            token: token.value,
            principal_name: token.principal,
            lifetime_ms: token.expires_at_ms,
        })
    }
}

impl ConsumerContext for Context {}

#[cfg(test)]
mod tests;
