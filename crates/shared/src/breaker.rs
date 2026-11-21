//! A circuit breaker per dependency, kept by each replica in memory (D4): 5
//! failures in a row open it for 30 s, or longer when the dependency asked to
//! be left alone that long; then one call goes through, and its answer closes
//! the breaker or opens it again.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

/// Failures in a row that open a breaker.
pub const FAILURES: u32 = 5;
/// How long an opened breaker stays open, at least.
pub const OPEN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Closed(u32),
    Open(Instant),
    /// One call is through since this instant; a probe that never reports is
    /// written off after [`OPEN`], so the breaker cannot stay shut for good.
    Probing(Instant),
}

/// Whether a call may go now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Call.
    Go,
    /// Wait until then: the breaker is open.
    Until(Instant),
    /// Another call is the probe; wait for its answer.
    Probing,
}

/// Breakers by dependency.
#[derive(Debug)]
pub struct Breakers<K> {
    states: Mutex<HashMap<K, State>>,
}

impl<K> Default for Breakers<K> {
    fn default() -> Self {
        Self { states: Mutex::new(HashMap::new()) }
    }
}

impl<K: Eq + Hash + Clone> Breakers<K> {
    /// Whether a call to `key` may go now; a `Go` after the open time makes this call the probe.
    pub fn pass(&self, key: &K) -> Pass {
        let mut states = self.states.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(state) = states.get_mut(key) else { return Pass::Go };
        let now = Instant::now();
        match *state {
            State::Closed(_) => Pass::Go,
            State::Open(until) if now < until => Pass::Until(until),
            State::Probing(since) if now < since + OPEN => Pass::Probing,
            State::Open(_) | State::Probing(_) => {
                *state = State::Probing(now);
                Pass::Go
            }
        }
    }

    /// The keys a call would not go to now.
    pub fn held(&self) -> Vec<K> {
        let states = self.states.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let held = |s: &State| match *s {
            State::Closed(_) => false,
            State::Open(until) => now < until,
            State::Probing(since) => now < since + OPEN,
        };
        states.iter().filter(|(_, s)| held(s)).map(|(k, _)| k.clone()).collect()
    }

    /// A call to `key` succeeded: it closes.
    pub fn succeeded(&self, key: &K) {
        self.states.lock().unwrap_or_else(PoisonError::into_inner).remove(key);
    }

    /// A call to `key` failed, and the dependency asked for `hold` without
    /// calls when it said; `true` when this failure opened the breaker.
    pub fn failed(&self, key: &K, hold: Option<Duration>) -> bool {
        let mut states = self.states.lock().unwrap_or_else(PoisonError::into_inner);
        let state = states.entry(key.clone()).or_insert(State::Closed(0));
        let failures = match *state {
            State::Closed(n) => n + 1,
            State::Open(_) | State::Probing(_) => FAILURES,
        };
        if failures < FAILURES {
            *state = State::Closed(failures);
            return false;
        }
        let open = hold.map_or(OPEN, |h| h.max(OPEN));
        let opened = !matches!(*state, State::Open(_));
        // A retry-after past what an Instant holds is nonsense; it gets the default hold.
        *state = State::Open(Instant::now().checked_add(open).unwrap_or_else(|| Instant::now() + OPEN));
        opened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn five_failures_open_it_for_30_s_then_one_call_goes_and_its_answer_decides() {
        let b = Breakers::default();
        for _ in 0..4 {
            assert!(!b.failed(&"e", None));
        }
        assert_eq!(b.pass(&"e"), Pass::Go, "four failures keep it closed");
        assert!(b.failed(&"e", None), "the fifth opens it");
        assert!(matches!(b.pass(&"e"), Pass::Until(_)));
        assert_eq!(b.held(), ["e"]);
        tokio::time::advance(OPEN).await;
        assert_eq!(b.pass(&"e"), Pass::Go, "the probe");
        assert_eq!(b.pass(&"e"), Pass::Probing, "one call at a time");
        assert!(b.failed(&"e", None), "a failed probe opens it again");
        tokio::time::advance(OPEN).await;
        assert_eq!(b.pass(&"e"), Pass::Go);
        b.succeeded(&"e");
        assert_eq!((b.pass(&"e"), b.held()), (Pass::Go, vec![]), "a success closes it");
    }

    #[tokio::test(start_paused = true)]
    async fn a_longer_hold_keeps_it_open_and_a_lost_probe_is_written_off() {
        let b = Breakers::default();
        for _ in 0..FAILURES {
            b.failed(&"p", Some(Duration::from_secs(90)));
        }
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(matches!(b.pass(&"p"), Pass::Until(_)), "held for the 90 s asked");
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(b.pass(&"p"), Pass::Go);
        tokio::time::advance(OPEN).await;
        assert_eq!(b.pass(&"p"), Pass::Go, "a probe that never answered is written off");
    }

    #[tokio::test(start_paused = true)]
    async fn a_hold_past_what_an_instant_holds_gets_the_default_hold() {
        let b = Breakers::default();
        for _ in 0..FAILURES {
            b.failed(&"p", Some(Duration::MAX));
        }
        assert!(matches!(b.pass(&"p"), Pass::Until(_)));
        tokio::time::advance(OPEN).await;
        assert_eq!(b.pass(&"p"), Pass::Go);
    }
}
