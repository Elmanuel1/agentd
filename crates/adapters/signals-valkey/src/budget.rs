//! The account-wide model budget on Valkey: per provider and minute, one
//! counter of requests and one of input tokens, each expiring after two
//! minutes, shared by every loop worker.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use engine::model::budget::{Admit, ModelBudget};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{Client, RedisError};
use shared::BackendError;

/// The longest a step waits on the budget before it defers.
const WAIT: Duration = Duration::from_millis(200);
/// A minute's counters outlive it, so a late request still finds them.
const KEEP: i64 = 120;

/// [`ModelBudget`] over Valkey.
#[derive(Clone)]
pub struct ValkeyBudget {
    conn: ConnectionManager,
    requests: Option<u64>,
    tokens: Option<u64>,
}

impl std::fmt::Debug for ValkeyBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeyBudget")
            .field("requests", &self.requests)
            .field("tokens", &self.tokens)
            .finish()
    }
}

impl ValkeyBudget {
    /// A budget of `requests` and `tokens` per provider and minute, either
    /// unlimited when `None`, on the Valkey at `url`.
    pub fn connect(url: &str, requests: Option<u64>, tokens: Option<u64>) -> Result<Self, RedisError> {
        let conn =
            ConnectionManager::new_lazy_with_config(Client::open(url)?, ConnectionManagerConfig::new())?;
        Ok(Self { conn, requests, tokens })
    }
}

/// This minute, and the seconds left in it.
fn minute() -> (u64, i64) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    (now / 60, i64::try_from(60 - now % 60).unwrap_or(60))
}

fn key(provider: &str, minute: u64, what: &str) -> String {
    format!("agentd:budget:{provider}:{minute}:{what}")
}

#[async_trait]
impl ModelBudget for ValkeyBudget {
    async fn admit(&self, provider: &str) -> Result<Admit, BackendError> {
        let (minute, left) = minute();
        let (requests, tokens) = (key(provider, minute, "requests"), key(provider, minute, "tokens"));
        let mut pipe = redis::pipe();
        pipe.atomic().incr(&requests, 1).expire(&requests, KEEP).ignore().get(&tokens);
        let mut conn = self.conn.clone();
        let counted = tokio::time::timeout(WAIT, pipe.query_async::<(u64, Option<u64>)>(&mut conn)).await;
        let (made, used) = counted.map_err(BackendError::new)?.map_err(BackendError::new)?;
        let over = self.requests.is_some_and(|max| made > max)
            || self.tokens.is_some_and(|max| used.unwrap_or(0) >= max);
        Ok(if over { Admit::Later(left) } else { Admit::Yes })
    }

    async fn spend(&self, provider: &str, input_tokens: u64) -> Result<(), BackendError> {
        let (minute, _) = minute();
        let tokens = key(provider, minute, "tokens");
        let mut pipe = redis::pipe();
        pipe.atomic().incr(&tokens, input_tokens).ignore().expire(&tokens, KEEP).ignore();
        let mut conn = self.conn.clone();
        let spent = tokio::time::timeout(WAIT, pipe.query_async::<()>(&mut conn)).await;
        spent.map_err(BackendError::new)?.map_err(BackendError::new)
    }
}
