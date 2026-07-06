//! Session business logic: lifecycle, event ingest, the status state
//! machine, live streaming, and the engine contract.
//!
//! State machine (documented + probe-consistent):
//! ```text
//! rescheduling → running ↔ idle → terminated
//! ```
//! - events can be sent while `running` or `idle`; queued in order,
//! - `terminated` is absorbing; archived sessions are read-only,
//! - `session.status_idle` carries a `stop_reason`; `requires_action` is
//!   NOT terminal for drain loops.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::limits::{required, validate_range_u32, values};
use agentd_core::{ApiError, AuthContext, Pagination, Timestamp, Usage, WorkspaceId};
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::events::{Event, EventPayload, StopReason};
use crate::store::{SessionsStore, StoreError};
use crate::types::{
    CreateSessionRequest, OutcomeEvaluation, OutcomeId, Session, SessionId, SessionResource,
    SessionResourceId, SessionResourceInput, SessionStatus, SessionTag, Stats,
    UpdateSessionRequest,
};

use agentd_agents::AgentSnapshot;

/// Resolution + validation dependencies the sessions service needs from
/// other entities — a port, so tests fake it and `agentd-api` wires it to
/// the real services (keeps the crate graph acyclic).
#[async_trait]
pub trait SessionDeps: Send + Sync {
    /// Resolve an agent reference (id + optional pinned version) to a frozen
    /// snapshot. Errors if missing or archived.
    async fn resolve_agent(
        &self,
        ctx: &AuthContext,
        agent_id: &str,
        version: Option<u64>,
    ) -> Result<AgentSnapshot, ApiError>;
    /// Verify the environment exists (org-scoped) and is not archived.
    async fn check_environment(&self, ctx: &AuthContext, env_id: &str) -> Result<(), ApiError>;
}

/// What the engine (task-side loop) is allowed to do to a session — the
/// sessions↔engine seam.
#[async_trait]
pub trait EngineContract: Send + Sync {
    /// Append an agent/span event and broadcast it.
    async fn emit(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        payload: EventPayload,
    ) -> Result<Event, ApiError>;
    /// Transition status (emits the matching `session.status_*` event).
    async fn set_status(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        status: SessionStatus,
        stop_reason: Option<StopReason>,
    ) -> Result<(), ApiError>;
    /// Pull queued (unprocessed) user events, stamping them processed.
    async fn drain_user_events(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> Result<Vec<Event>, ApiError>;
    /// Accumulate usage onto the session.
    async fn add_usage(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        usage: &Usage,
    ) -> Result<(), ApiError>;
    /// Record an outcome evaluation onto the session object.
    async fn record_outcome(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        eval: OutcomeEvaluation,
    ) -> Result<(), ApiError>;
}

/// Sessions service.
pub struct SessionsService {
    store: Arc<dyn SessionsStore>,
    deps: Arc<dyn SessionDeps>,
    /// Live SSE fan-out. Stream has NO replay — reconnect = list + dedupe.
    channels: Mutex<HashMap<SessionId, broadcast::Sender<Event>>>,
}

impl std::fmt::Debug for SessionsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionsService").finish_non_exhaustive()
    }
}

impl SessionsService {
    /// Build with a store and dependency port.
    pub fn new(store: Arc<dyn SessionsStore>, deps: Arc<dyn SessionDeps>) -> Self {
        Self {
            store,
            deps,
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// `POST /v1/sessions`.
    pub async fn create(
        &self,
        ctx: &AuthContext,
        req: CreateSessionRequest,
    ) -> Result<Session, ApiError> {
        let agent_ref = required("agent", req.agent)?;
        let environment_id = required("environment_id", req.environment_id)?;
        // Sessions do NOT cap metadata entries (live divergence) but key and
        // value length rules still apply.
        req.metadata
            .validate(agentd_core::metadata::MetadataPolicy::unlimited_entries())?;
        let agent = self
            .deps
            .resolve_agent(ctx, agent_ref.id(), agent_ref.version())
            .await?;
        self.deps.check_environment(ctx, &environment_id).await?;
        let resources = req
            .resources
            .into_iter()
            .map(assign_resource_id)
            .collect::<Vec<_>>();
        let now = Timestamp::now();
        let session = Session {
            agent,
            archived_at: None,
            created_at: now,
            environment_id,
            id: SessionId::generate(),
            metadata: req.metadata,
            outcome_evaluations: vec![],
            resources,
            stats: Stats::default(),
            status: SessionStatus::Idle,
            title: req.title,
            tag: SessionTag::Session,
            updated_at: now,
            usage: Usage::default(),
            vault_ids: req.vault_ids,
        };
        self.store
            .insert(&ctx.workspace, session.clone())
            .await
            .map_err(map_store)?;
        Ok(session)
    }

    /// `GET /v1/sessions/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/sessions`.
    pub async fn list(
        &self,
        ctx: &AuthContext,
        page: &Pagination,
    ) -> Result<Vec<Session>, ApiError> {
        self.store.list(&ctx.workspace, page).await.map_err(map_store)
    }

    /// `POST /v1/sessions/{id}` — title/metadata only here.
    pub async fn update(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: UpdateSessionRequest,
    ) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let mut s = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        guard_writable(&s)?;
        if let Some(t) = req.title {
            s.title = Some(t);
        }
        if let Some(m) = req.metadata {
            m.validate(agentd_core::metadata::MetadataPolicy::unlimited_entries())?;
            s.metadata = m;
        }
        s.updated_at = Timestamp::now();
        self.store
            .replace(&ctx.workspace, s.clone())
            .await
            .map_err(map_store)?;
        Ok(s)
    }

    /// `POST /v1/sessions/{id}/archive` — read-only afterwards.
    pub async fn archive(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let mut s = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if s.status == SessionStatus::Running {
            // Documented race: cannot archive while running.
            return Err(ApiError::conflict_invalid_request(
                "cannot archive a running session",
            ));
        }
        if s.archived_at.is_none() {
            s.archived_at = Some(Timestamp::now());
            s.updated_at = Timestamp::now();
            self.store
                .replace(&ctx.workspace, s.clone())
                .await
                .map_err(map_store)?;
            // Drop the broadcast sender — an archived session takes no new
            // events, so keeping its channel would leak the entry (and its
            // ring buffer) for the process lifetime.
            self.channels.lock().remove(&id);
        }
        Ok(s)
    }

    /// `DELETE /v1/sessions/{id}` — removes session, events, checkpoints.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let s = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if s.status == SessionStatus::Running {
            return Err(ApiError::conflict_invalid_request(
                "cannot delete a running session",
            ));
        }
        self.channels.lock().remove(&id);
        self.store
            .delete(&ctx.workspace, &id)
            .await
            .map_err(map_store)
    }

    // ── events ────────────────────────────────────────────────────────────

    /// `POST /v1/sessions/{id}/events` — queue client events in order.
    pub async fn send_events(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        payloads: Vec<EventPayload>,
    ) -> Result<Vec<Event>, ApiError> {
        let id = parse_id(raw_id)?;
        let session = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        guard_writable(&session)?;
        if session.status == SessionStatus::Terminated {
            return Err(ApiError::conflict_invalid_request(
                "cannot send events to a terminated session",
            ));
        }
        // Validate each payload with the live field-path error convention.
        for (i, p) in payloads.iter().enumerate() {
            if let EventPayload::UserDefineOutcome {
                max_iterations: Some(n),
                ..
            } = p
            {
                validate_range_u32(
                    &format!("events.{i}.user_define_outcome.max_iterations"),
                    *n,
                    values::OUTCOME_MAX_ITERATIONS_MIN,
                    values::OUTCOME_MAX_ITERATIONS_MAX,
                )?;
            }
            if !p.is_user_event() {
                return Err(ApiError::invalid_request(format!(
                    "events.{i}: only user.* and system.message events can be sent"
                )));
            }
        }
        let mut out = Vec::with_capacity(payloads.len());
        for p in payloads {
            // define_outcome gets its server-assigned outcome id on echo.
            let p = match p {
                EventPayload::UserDefineOutcome {
                    description,
                    rubric,
                    max_iterations,
                    outcome_id: _,
                } => EventPayload::UserDefineOutcome {
                    description,
                    rubric,
                    max_iterations,
                    outcome_id: Some(OutcomeId::generate()),
                },
                other => other,
            };
            let ev = Event::new(p);
            self.store
                .append_event(&ctx.workspace, &id, ev.clone())
                .await
                .map_err(map_store)?;
            self.broadcast(&id, &ev);
            out.push(ev);
        }
        Ok(out)
    }

    /// `GET /v1/sessions/{id}/events` — full history (reconnect/dedupe
    /// source; the SSE stream has no replay).
    pub async fn list_events(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vec<Event>, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.events(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// Subscribe to live events (`GET /v1/sessions/{id}/events/stream`).
    pub async fn subscribe(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<broadcast::Receiver<Event>, ApiError> {
        let id = parse_id(raw_id)?;
        // Existence check under the caller's workspace.
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        Ok(self.channel(&id).subscribe())
    }

    fn channel(&self, id: &SessionId) -> broadcast::Sender<Event> {
        self.channels
            .lock()
            .entry(id.clone())
            .or_insert_with(|| broadcast::channel(1024).0)
            .clone()
    }

    fn broadcast(&self, id: &SessionId, ev: &Event) {
        // No receiver = no error; stream is fire-and-forget.
        let _ = self.channel(id).send(ev.clone());
    }
}

/// The engine contract is implemented by the same service (single source of
/// truth for the event log + broadcast), consumed by `agentd-engine`.
#[async_trait]
impl EngineContract for SessionsService {
    async fn emit(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        payload: EventPayload,
    ) -> Result<Event, ApiError> {
        let mut ev = Event::new(payload);
        // Agent-side events are processed by construction.
        ev.processed_at = Some(Timestamp::now());
        self.store
            .append_event(ws, session, ev.clone())
            .await
            .map_err(map_store)?;
        self.broadcast(session, &ev);
        Ok(ev)
    }

    async fn set_status(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        status: SessionStatus,
        stop_reason: Option<StopReason>,
    ) -> Result<(), ApiError> {
        let mut s = self.store.get(ws, session).await.map_err(map_store)?;
        // Terminated is absorbing.
        if s.status == SessionStatus::Terminated {
            return Err(ApiError::conflict_invalid_request(
                "session is terminated",
            ));
        }
        s.status = status;
        s.updated_at = Timestamp::now();
        self.store.replace(ws, s).await.map_err(map_store)?;
        let payload = match status {
            SessionStatus::Idle => EventPayload::SessionStatusIdle {
                stop_reason: stop_reason.unwrap_or(StopReason::EndTurn {}),
            },
            SessionStatus::Running => EventPayload::SessionStatusRunning {},
            SessionStatus::Rescheduling => EventPayload::SessionStatusRescheduled {},
            SessionStatus::Terminated => EventPayload::SessionStatusTerminated {},
        };
        self.emit(ws, session, payload).await?;
        Ok(())
    }

    async fn drain_user_events(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> Result<Vec<Event>, ApiError> {
        let events = self.store.events(ws, session).await.map_err(map_store)?;
        let mut drained = Vec::new();
        for mut ev in events {
            if ev.payload.is_user_event() && ev.processed_at.is_none() {
                ev.processed_at = Some(Timestamp::now());
                self.store
                    .replace_event(ws, session, ev.clone())
                    .await
                    .map_err(map_store)?;
                // The processed form is re-broadcast (documented: client
                // events appear twice — queued then processed).
                self.broadcast(session, &ev);
                drained.push(ev);
            }
        }
        Ok(drained)
    }

    async fn add_usage(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        usage: &Usage,
    ) -> Result<(), ApiError> {
        let mut s = self.store.get(ws, session).await.map_err(map_store)?;
        s.usage.add(usage);
        s.updated_at = Timestamp::now();
        self.store.replace(ws, s).await.map_err(map_store)
    }

    async fn record_outcome(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        eval: OutcomeEvaluation,
    ) -> Result<(), ApiError> {
        let mut s = self.store.get(ws, session).await.map_err(map_store)?;
        // Replace the entry for this outcome or append.
        match s
            .outcome_evaluations
            .iter_mut()
            .find(|e| e.outcome_id == eval.outcome_id)
        {
            Some(slot) => *slot = eval,
            None => s.outcome_evaluations.push(eval),
        }
        s.updated_at = Timestamp::now();
        self.store.replace(ws, s).await.map_err(map_store)
    }
}

fn assign_resource_id(input: SessionResourceInput) -> SessionResource {
    let id = Some(SessionResourceId::generate());
    match input {
        SessionResourceInput::File {
            file_id,
            mount_path,
        } => SessionResource::File {
            id,
            file_id,
            mount_path: mount_path.unwrap_or_else(|| "/workspace/file".into()),
        },
        SessionResourceInput::GithubRepository {
            url,
            mount_path,
            checkout,
            ..
        } => SessionResource::GithubRepository {
            id,
            url,
            mount_path,
            checkout,
        },
        SessionResourceInput::MemoryStore {
            memory_store_id,
            access,
            instructions,
        } => SessionResource::MemoryStore {
            id,
            memory_store_id,
            access,
            instructions,
        },
    }
}

fn guard_writable(s: &Session) -> Result<(), ApiError> {
    if s.archived_at.is_some() {
        return Err(ApiError::conflict_invalid_request(
            "session is archived and read-only",
        ));
    }
    Ok(())
}

fn parse_id(raw: &str) -> Result<SessionId, ApiError> {
    SessionId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("session not found"),
        StoreError::Backend(m) => ApiError::internal(m),
    }
}
