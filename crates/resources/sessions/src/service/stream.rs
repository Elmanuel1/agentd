//! Live streams. Per replica, one tail per streamed session reads its log
//! when a signal names the session or its fallback timer fires, and sends
//! each event to the session's subscribers here. The log is the one source,
//! so every replica's streams see the same events in the same order.

use std::collections::HashMap;
use std::sync::{Arc, Once, Weak};

use parking_lot::Mutex;
use shared::{ApiError, AuthContext, ErrorType, WorkspaceId};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, Notify};

use super::{map_store, parse_id, SessionsService};
use crate::events::Event;
use crate::signals::{LocalSignals, Signal, Signals};
use crate::store::DatabaseLoad;
use crate::types::SessionId;

mod follow;

use self::follow::Follow;

/// Events a subscriber holds before it falls behind and its stream ends.
const BUFFER: usize = 1024;

/// A session's tail as its subscribers reach it.
#[derive(Debug, Clone)]
struct Tail {
    tx: broadcast::Sender<Event>,
    wake: Arc<Notify>,
}

type Tails = Arc<Mutex<HashMap<SessionId, Tail>>>;

/// How this replica learns of changes and whether it takes new streams.
pub struct Streams {
    signals: Arc<dyn Signals>,
    load: Arc<dyn DatabaseLoad>,
    tails: Tails,
    listening: Once,
}

impl Streams {
    /// Signals from `signals`; new streams refused while `load` says so.
    pub fn new(signals: Arc<dyn Signals>, load: Arc<dyn DatabaseLoad>) -> Self {
        Self { signals, load, tails: Tails::default(), listening: Once::new() }
    }
}

impl std::fmt::Debug for Streams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Streams").finish_non_exhaustive()
    }
}

impl Default for Streams {
    /// One replica's signals, never refusing a stream.
    fn default() -> Self {
        Self::new(Arc::new(LocalSignals::default()), Arc::new(Unloaded))
    }
}

struct Unloaded;

impl DatabaseLoad for Unloaded {
    fn overloaded(&self) -> bool {
        false
    }
}

impl SessionsService {
    /// Subscribe to live events (`GET /v1/sessions/{id}/events/stream`);
    /// `environment` as for [`Self::send_events`]. While the database is
    /// overloaded a new stream is refused and the open ones keep streaming.
    pub async fn subscribe(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        environment: Option<&str>,
    ) -> Result<Subscription, ApiError> {
        let id = parse_id(raw_id)?;
        if self.streams.load.overloaded() {
            return Err(ApiError::of(ErrorType::Overloaded, "the database is overloaded; retry the stream"));
        }
        let session = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?.seen_from(environment)?;
        self.streams.listening.call_once(|| {
            let rx = self.streams.signals.subscribe();
            tokio::spawn(wake_tails(rx, Arc::downgrade(&self.streams.tails)));
        });
        let tails = &self.streams.tails;
        // Listed before its head is read, so a signal meanwhile leaves the
        // tail a wake-up.
        let (tail, rx, new) = {
            let mut map = tails.lock();
            let new = !map.contains_key(&id);
            let tail = map
                .entry(id.clone())
                .or_insert_with(|| Tail { tx: broadcast::channel(BUFFER).0, wake: Arc::new(Notify::new()) })
                .clone();
            let rx = tail.tx.subscribe();
            (tail, rx, new)
        };
        let sub = Subscription { rx, wake: tail.wake.clone(), tails: tails.clone(), id: id.clone() };
        if !new {
            return Ok(sub);
        }
        // The stream has no replay: it starts after the latest event.
        let head = match self.events.head(&ctx.workspace, &id).await {
            Ok(head) => head,
            Err(e) => {
                // No tail runs for it: the next stream lists its own.
                let mut map = tails.lock();
                if listed(&map, &id, &tail.wake) {
                    map.remove(&id);
                }
                return Err(map_store(e));
            }
        };
        let follow = Follow {
            events: self.events.clone(),
            tails: tails.clone(),
            ws: ctx.workspace.clone(),
            id,
            tail,
            ends_on_archive: session.archived_at.is_none(),
        };
        tokio::spawn(follow.run(head));
        Ok(sub)
    }

    /// Tell every replica the session changed. A signal that cannot be sent
    /// is logged: the fallback timer reads the log anyway.
    pub(super) async fn publish(&self, ws: &WorkspaceId, id: &SessionId) {
        let signal = Signal { workspace: ws.clone(), session: id.clone() };
        if let Err(e) = self.streams.signals.publish(&signal).await {
            tracing::warn!(session = %id, error = %e, "sessions: could not signal the session's streams");
        }
    }
}

/// `wake` is the tail listed for `id`, not one that replaced it.
fn listed(tails: &HashMap<SessionId, Tail>, id: &SessionId, wake: &Arc<Notify>) -> bool {
    tails.get(id).is_some_and(|t| Arc::ptr_eq(&t.wake, wake))
}

async fn wake_tails(mut rx: broadcast::Receiver<Signal>, tails: Weak<Mutex<HashMap<SessionId, Tail>>>) {
    loop {
        let signal = rx.recv().await;
        let Some(tails) = tails.upgrade() else { return };
        let tails = tails.lock();
        match signal {
            Ok(s) => tails.get(&s.session).iter().for_each(|t| t.wake.notify_one()),
            // Signals were missed: every tail reads once more.
            Err(RecvError::Lagged(_)) => tails.values().for_each(|t| t.wake.notify_one()),
            Err(RecvError::Closed) => return,
        }
    }
}

/// One listener on a session's events; the session's tail goes when its
/// last listener does.
#[derive(Debug)]
pub struct Subscription {
    rx: broadcast::Receiver<Event>,
    wake: Arc<Notify>,
    tails: Tails,
    id: SessionId,
}

impl Subscription {
    /// The next event; an error ends the stream (archived, or fell behind).
    pub async fn recv(&mut self) -> Result<Event, RecvError> {
        self.rx.recv().await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut tails = self.tails.lock();
        // This receiver is still counted while it drops.
        let last = listed(&tails, &self.id, &self.wake)
            && tails.get(&self.id).is_some_and(|t| t.tx.receiver_count() <= 1);
        if last {
            tails.remove(&self.id);
            self.wake.notify_one();
        }
    }
}
