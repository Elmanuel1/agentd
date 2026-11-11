//! One session's tail on this replica: it reads the log after a signal or
//! its fallback timer, and sends each event to the session's subscribers.

use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use shared::{Timestamp, WorkspaceId};

use super::{listed, Tail, Tails};
use crate::events::{Event, SessionEvent};
use crate::store::{Head, SessionEventLog};
use crate::types::SessionId;

/// Events one read of the log takes.
const PAGE: usize = 1000;
/// A tail reads the log this often with no signal, plus up to [`JITTER_MS`].
const FALLBACK: Duration = Duration::from_secs(5);
const JITTER_MS: u64 = 1000;

/// One session's tail on this replica.
pub(super) struct Follow {
    pub(super) events: Arc<dyn SessionEventLog>,
    pub(super) tails: Tails,
    pub(super) ws: WorkspaceId,
    pub(super) id: SessionId,
    pub(super) tail: Tail,
    /// The session was not archived when its first stream here opened.
    pub(super) ends_on_archive: bool,
}

impl Follow {
    pub(super) async fn run(self, head: Head) {
        // Client events not processed yet: each is sent once it is.
        let (mut cursor, mut pending) = (head.seq, head.queued);
        loop {
            let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..=JITTER_MS));
            tokio::select! {
                () = self.tail.wake.notified() => {}
                () = tokio::time::sleep(FALLBACK + jitter) => {}
            }
            {
                // The last subscriber left, or took the tail with it.
                let mut tails = self.tails.lock();
                if !listed(&tails, &self.id, &self.tail.wake) {
                    return;
                }
                if self.tail.tx.receiver_count() == 0 {
                    tails.remove(&self.id);
                    return;
                }
            }
            loop {
                let read = match self.events.tail(&self.ws, &self.id, cursor, &pending, PAGE).await {
                    Ok(read) => read,
                    Err(e) => {
                        tracing::warn!(session = %self.id, error = %e, "sessions: could not read the log");
                        break;
                    }
                };
                let Some(read) = read else {
                    let mut last = Event::new(SessionEvent::Deleted {}.into());
                    last.processed_at = Some(Timestamp::now());
                    let _ = self.tail.tx.send(last);
                    return self.close();
                };
                let fresh = read.events.iter().filter(|(seq, _)| *seq > cursor).count();
                for (seq, ev) in read.events {
                    self.send(&mut cursor, &mut pending, seq, ev);
                }
                if read.archived && self.ends_on_archive {
                    return self.close();
                }
                if fresh < PAGE {
                    break;
                }
            }
        }
    }

    fn send(&self, cursor: &mut i64, pending: &mut Vec<i64>, seq: i64, ev: Event) {
        let seen = seq <= *cursor;
        if seen && ev.processed_at.is_none() {
            return;
        }
        if seen {
            pending.retain(|p| *p != seq);
            let _ = self.tail.tx.send(ev);
            return;
        }
        *cursor = seq;
        if !ev.payload.is_user_event() {
            let _ = self.tail.tx.send(ev);
            return;
        }
        // A client event is sent queued, then again once processed.
        if ev.processed_at.is_none() {
            pending.push(seq);
            let _ = self.tail.tx.send(ev);
            return;
        }
        let queued = Event { processed_at: None, ..ev.clone() };
        let _ = self.tail.tx.send(queued);
        let _ = self.tail.tx.send(ev);
    }

    /// End every stream of the session here: they end once the tail's
    /// senders are gone.
    fn close(self) {
        let mut tails = self.tails.lock();
        if listed(&tails, &self.id, &self.tail.wake) {
            tails.remove(&self.id);
        }
    }
}
