//! Event retention: the events of sessions archived longer ago than the
//! retention period are deleted, by one replica at a time.

use std::time::Duration;

use async_trait::async_trait;
use futures::future::BoxFuture;
use shared::{Timestamp, WorkspaceId};

use crate::store::{SessionEventLog, StoreError};
use crate::types::SessionId;

/// Sessions a page of the walk takes.
const PAGE: usize = 500;
/// A later run walks back this far before the cutoff, so a run missed for
/// less than this is caught by the next.
const HORIZON: Duration = Duration::from_secs(60 * 60);
/// How often each replica tries to run.
pub const EVERY: Duration = Duration::from_secs(5 * 60);
/// The lock one run holds across replicas.
const LOCK: &str = "event_retention";

/// A place in the walk of archived sessions: after `archived_at`, then
/// after `session_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchivedCursor {
    /// An `archived_at`, in its wire form.
    pub archived_at: String,
    /// A session id within that `archived_at`.
    pub session_id: String,
}

/// An archived session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedSession {
    /// Its workspace.
    pub workspace: WorkspaceId,
    /// The session.
    pub session: SessionId,
    /// When it was archived, in its wire form.
    pub archived_at: String,
}

/// The archived sessions, in the order the walk takes them.
#[async_trait]
pub trait ArchivedSessions: Send + Sync {
    /// Up to `limit` sessions after `after` and archived before `to`.
    async fn archived_between(
        &self,
        after: &ArchivedCursor,
        to: Timestamp,
        limit: usize,
    ) -> Result<Vec<ArchivedSession>, StoreError>;
}

/// A lock every replica sees.
#[async_trait]
pub trait RunLock: Send + Sync {
    /// Run `job` only while this process alone holds `name`; `false`, with
    /// `job` not run, while another holds it. A job that fails or panics
    /// still frees the lock.
    async fn exclusive(
        &self,
        name: &str,
        job: BoxFuture<'_, Result<(), StoreError>>,
    ) -> Result<bool, StoreError>;
}

/// The retention runs of one replica.
pub struct Retention {
    sessions: Box<dyn ArchivedSessions>,
    events: Box<dyn SessionEventLog>,
    lock: Box<dyn RunLock>,
    period: Duration,
}

impl std::fmt::Debug for Retention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Retention").field("period", &self.period).finish_non_exhaustive()
    }
}

impl Retention {
    /// Delete the events of sessions archived longer ago than `period`.
    pub fn new(
        sessions: Box<dyn ArchivedSessions>,
        events: Box<dyn SessionEventLog>,
        lock: Box<dyn RunLock>,
        period: Duration,
    ) -> Self {
        Self { sessions, events, lock, period }
    }

    /// One run at `now`: from the oldest archived session when `first`, else
    /// from [`HORIZON`] before the cutoff; `false` when another replica holds
    /// the run.
    pub async fn run(&self, first: bool, now: Timestamp) -> Result<bool, StoreError> {
        let cutoff = now.earlier_by(self.period);
        let start = match first {
            true => ArchivedCursor::default(),
            false => ArchivedCursor {
                archived_at: cutoff.earlier_by(HORIZON).to_wire(),
                session_id: String::new(),
            },
        };
        self.lock.exclusive(LOCK, Box::pin(self.walk(start, cutoff))).await
    }

    async fn walk(&self, mut after: ArchivedCursor, to: Timestamp) -> Result<(), StoreError> {
        loop {
            let page = self.sessions.archived_between(&after, to, PAGE).await?;
            let Some(last) = page.last() else {
                return Ok(());
            };
            after = ArchivedCursor {
                archived_at: last.archived_at.clone(),
                session_id: last.session.to_string(),
            };
            let ids: Vec<_> = page.iter().map(|s| (s.workspace.clone(), s.session.clone())).collect();
            self.events.delete_events_of(&ids).await?;
            if page.len() < PAGE {
                return Ok(());
            }
        }
    }

    /// Try a run every [`EVERY`]; `done` hears each run this replica completes.
    pub fn spawn(self, done: impl Fn(Timestamp) + Send + 'static) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut first = true;
            let mut every = tokio::time::interval(EVERY);
            loop {
                every.tick().await;
                match self.run(first, Timestamp::now()).await {
                    Ok(true) => {
                        first = false;
                        done(Timestamp::now());
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!(error = %e, "sessions: the event retention run failed"),
                }
            }
        })
    }
}
