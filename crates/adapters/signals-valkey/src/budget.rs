//! The account-wide model budget on Valkey: per provider and minute, one
//! counter of requests and one of input tokens, each expiring after two
//! minutes, shared by every loop worker. A step over the budget waits for the
//! next minute plus a random 0 to 60 s, so the waiting steps do not all
//! come back at its first second (SH-53).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use engine::model::budget::{Admit, NativeModelBudget};
use rand::RngExt;
use redis::RedisError;
use shared::BackendError;

use crate::valkey::{Shared, Valkey};

/// The longest a step waits on the budget before it defers.
const WAIT: Duration = Duration::from_millis(200);
/// A minute's counters outlive it, so a late request still finds them.
const KEEP: i64 = 120;
/// The most a deferred step's wait is spread past the window's end, in seconds.
const SPREAD: i64 = 60;

/// [`ModelBudget`](engine::model::budget::ModelBudget) over Valkey.
#[derive(Clone)]
pub struct ValkeyBudget {
    conn: Shared,
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
    /// unlimited when `None`, on `valkey`.
    pub fn connect(valkey: &Valkey, requests: Option<u64>, tokens: Option<u64>) -> Result<Self, RedisError> {
        Ok(Self { conn: valkey.shared()?, requests, tokens })
    }
}

/// This minute, and the seconds left in it.
fn minute() -> (u64, i64) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    (now / 60, i64::try_from(60 - now % 60).unwrap_or(60))
}

/// Wait out the `left` seconds of the window, and up to a minute more.
fn later(left: i64) -> Admit {
    Admit::Later(left + rand::rng().random_range(0..=SPREAD))
}

/// A minute's counters share a hash tag, so on a cluster one shard holds both
/// and a transaction may touch them together.
fn key(provider: &str, minute: u64, what: &str) -> String {
    format!("agentd:budget:{{{provider}:{minute}}}:{what}")
}

impl NativeModelBudget for ValkeyBudget {
    async fn admit(&self, provider: &str) -> Result<Admit, BackendError> {
        let (minute, left) = minute();
        let (requests, tokens) = (key(provider, minute, "requests"), key(provider, minute, "tokens"));
        let mut pipe = redis::pipe();
        pipe.atomic().incr(&requests, 1).expire(&requests, KEEP).ignore().get(&tokens);
        let counted = tokio::time::timeout(WAIT, async {
            pipe.query_async::<(u64, Option<u64>)>(&mut self.conn.get().await?).await
        })
        .await;
        let (made, used) = counted.map_err(BackendError::new)?.map_err(BackendError::new)?;
        let over = self.requests.is_some_and(|max| made > max)
            || self.tokens.is_some_and(|max| used.unwrap_or(0) >= max);
        Ok(match over {
            true => later(left),
            false => Admit::Yes,
        })
    }

    async fn spend(&self, provider: &str, input_tokens: u64) -> Result<(), BackendError> {
        let (minute, _) = minute();
        let tokens = key(provider, minute, "tokens");
        let mut pipe = redis::pipe();
        pipe.atomic().incr(&tokens, input_tokens).ignore().expire(&tokens, KEEP).ignore();
        let spent =
            tokio::time::timeout(WAIT, async { pipe.query_async::<()>(&mut self.conn.get().await?).await })
                .await;
        spent.map_err(BackendError::new)?.map_err(BackendError::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_third_request_in_a_minute_over_a_budget_of_two_waits_for_the_next_minute() {
        for valkey in crate::test_valkeys() {
            over_budget_waits(&valkey).await;
        }
    }

    async fn over_budget_waits(valkey: &Valkey) {
        let provider = format!("test-{}-{}", std::process::id(), uuid::Uuid::new_v4());
        let budget = ValkeyBudget::connect(valkey, Some(2), None).unwrap();
        assert_eq!(budget.admit(&provider).await.unwrap(), Admit::Yes);
        assert_eq!(budget.admit(&provider).await.unwrap(), Admit::Yes);
        assert!(matches!(budget.admit(&provider).await.unwrap(), Admit::Later(s) if (1..=120).contains(&s)));
        let tokens = ValkeyBudget::connect(valkey, None, Some(100)).unwrap();
        let other = format!("{provider}-tokens");
        assert_eq!(tokens.admit(&other).await.unwrap(), Admit::Yes);
        tokens.spend(&other, 100).await.unwrap();
        assert!(
            matches!(tokens.admit(&other).await.unwrap(), Admit::Later(_)),
            "the minute's tokens are spent"
        );
    }

    #[test]
    fn an_over_budget_wait_is_spread_over_the_next_minute() {
        let waits: Vec<i64> = (0..1_000)
            .map(|_| match later(10) {
                Admit::Later(s) => s,
                Admit::Yes => 0,
            })
            .collect();
        assert!(waits.iter().all(|w| (10..=70).contains(w)));
        let (low, high) = (waits.iter().min().unwrap(), waits.iter().max().unwrap());
        assert!(*low < 20 && *high > 60, "spread from {low} to {high}");
    }

    #[tokio::test]
    async fn with_valkey_down_the_budget_answers_an_error_so_the_step_waits() {
        let down = Valkey::open("redis://127.0.0.1:1", false).unwrap();
        let budget = ValkeyBudget::connect(&down, Some(2), None).unwrap();
        assert!(budget.admit("anthropic").await.is_err());
    }
}
