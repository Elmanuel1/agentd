//! Request counts per organization and bucket in a fixed 60 s window that
//! starts at the window's first request (L2, DEP-2), shared by every replica.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use crate::BackendError;
use crate::ports::BoxFuture;

/// The window every bucket counts in.
pub const WINDOW: Duration = Duration::from_secs(60);

/// A request counted in its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counted {
    /// The window's requests, this one included.
    pub count: u64,
    /// What is left of the window.
    pub resets_in: Duration,
}

/// Where requests are counted.
pub trait RateCounter: Send + Sync {
    /// Count one request of `org` in `bucket`.
    fn hit<'a>(&'a self, org: &'a str, bucket: &'a str) -> BoxFuture<'a, Result<Counted, BackendError>>;
}

/// What an adapter implements to provide [`RateCounter`]; the method is documented there.
pub trait NativeRateCounter: Send + Sync {
    fn hit(&self, org: &str, bucket: &str) -> impl Future<Output = Result<Counted, BackendError>> + Send;
}

impl<T: NativeRateCounter> RateCounter for T {
    fn hit<'a>(&'a self, org: &'a str, bucket: &'a str) -> BoxFuture<'a, Result<Counted, BackendError>> {
        Box::pin(NativeRateCounter::hit(self, org, bucket))
    }
}

/// [`RateCounter`] inside one process, for tests.
#[derive(Debug, Default)]
pub struct LocalRateCounter {
    windows: Mutex<HashMap<(String, String), (Instant, u64)>>,
}

impl NativeRateCounter for LocalRateCounter {
    async fn hit(&self, org: &str, bucket: &str) -> Result<Counted, BackendError> {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let window = windows.entry((org.to_owned(), bucket.to_owned())).or_insert((now, 0));
        if now >= window.0 + WINDOW {
            *window = (now, 0);
        }
        window.1 += 1;
        Ok(Counted { count: window.1, resets_in: (window.0 + WINDOW).saturating_duration_since(now) })
    }
}
