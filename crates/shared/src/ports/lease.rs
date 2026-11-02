//! The lease port: one holder at a time for a key, across every replica.
//! A session's agent loop runs only under its session's lease, so two
//! replicas never run the same session's turn at once.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::BackendError;

/// Held while the holder works; dropping it releases the lease.
pub struct LeaseGuard {
    _release: Box<dyn Send + Sync>,
}

impl LeaseGuard {
    /// Wrap whatever releases the lease when dropped.
    pub fn new(release: impl Send + Sync + 'static) -> Self {
        Self { _release: Box::new(release) }
    }
}

impl std::fmt::Debug for LeaseGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LeaseGuard")
    }
}

/// Exclusive leases by key.
#[async_trait]
pub trait Leases: Send + Sync {
    /// Take the lease on `key`, or `None` while another holder has it. A
    /// holder that dies loses its lease with its connection.
    async fn try_acquire(&self, key: &str) -> Result<Option<LeaseGuard>, BackendError>;
}

/// Leases within one process (tests and single-replica runs).
#[derive(Debug, Default, Clone)]
pub struct LocalLeases {
    held: Arc<Mutex<HashSet<String>>>,
}

struct LocalRelease {
    held: Arc<Mutex<HashSet<String>>>,
    key: String,
}

impl Drop for LocalRelease {
    fn drop(&mut self) {
        if let Ok(mut held) = self.held.lock() {
            held.remove(&self.key);
        }
    }
}

#[async_trait]
impl Leases for LocalLeases {
    async fn try_acquire(&self, key: &str) -> Result<Option<LeaseGuard>, BackendError> {
        let mut held =
            self.held.lock().map_err(|e| BackendError::new(std::io::Error::other(e.to_string())))?;
        if !held.insert(key.to_owned()) {
            return Ok(None);
        }
        Ok(Some(LeaseGuard::new(LocalRelease { held: self.held.clone(), key: key.to_owned() })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_lease_has_one_holder_until_it_is_dropped() {
        let leases = LocalLeases::default();
        let first = leases.try_acquire("sesn_1").await.unwrap();
        assert!(first.is_some());
        assert!(leases.try_acquire("sesn_1").await.unwrap().is_none());
        assert!(leases.try_acquire("sesn_2").await.unwrap().is_some());
        drop(first);
        assert!(leases.try_acquire("sesn_1").await.unwrap().is_some());
    }
}
