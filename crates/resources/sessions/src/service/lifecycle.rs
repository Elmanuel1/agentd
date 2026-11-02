use shared::limits::required;
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, Pagination, Timestamp, Usage};

use super::events::validate_sendable;
use super::{guard_writable, map_store, parse_id, SessionsService};
use crate::events::{Event, EventPayload, SessionEvent, UserEvent};
use crate::store::SessionFilter;
use crate::types::{
    AgentRefInput, CreateSessionRequest, Session, SessionId, SessionStatus, SessionTag, Stats,
    UpdateSessionRequest,
};

impl SessionsService {
    /// `POST /v1/sessions`. `initial_events` are queued as the first input.
    pub async fn create(&self, ctx: &AuthContext, req: CreateSessionRequest) -> Result<Session, ApiError> {
        let agent_ref = required("agent", req.agent)?;
        let environment_id = required("environment_id", req.environment_id)?;
        // Sessions do NOT cap metadata entries (live divergence) but key and
        // value length rules still apply.
        req.metadata.validate(MetadataPolicy::unlimited_entries())?;
        if let Some(i) = req
            .initial_events
            .iter()
            .position(|e| !matches!(e, UserEvent::Message { .. } | UserEvent::DefineOutcome { .. }))
        {
            return Err(ApiError::invalid_request(format!(
                "initial_events.{i}: only user.message and user.define_outcome events are allowed"
            )));
        }
        let initial: Vec<EventPayload> = req.initial_events.into_iter().map(EventPayload::User).collect();
        validate_sendable("initial_events", &initial)?;
        let mut agent = self.deps.resolve_agent(ctx, agent_ref.id(), agent_ref.version()).await?;
        if let AgentRefInput::Tagged(overrides) = agent_ref {
            agent = overrides.apply(agent, &self.catalog)?;
        }
        self.deps.check_environment(ctx, &environment_id).await?;
        let now = Timestamp::now();
        let session = Session {
            agent,
            archived_at: None,
            budget: req.budget,
            created_at: now,
            environment_id,
            id: SessionId::generate(),
            metadata: req.metadata,
            outcome_evaluations: vec![],
            resources: req.resources.into_iter().map(|r| r.into_resource(now)).collect(),
            stats: Stats::default(),
            status: SessionStatus::Idle,
            title: req.title,
            tag: SessionTag::Session,
            updated_at: now,
            usage: Usage::default(),
            vault_ids: req.vault_ids,
            deployment_id: req.deployment_id,
        };
        self.store.insert(&ctx.workspace, session.clone()).await.map_err(map_store)?;
        if !initial.is_empty() {
            self.send_events(ctx, session.id.as_str(), initial).await?;
        }
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
        filter: &SessionFilter,
        page: &Pagination,
    ) -> Result<Vec<Session>, ApiError> {
        self.store.list(&ctx.workspace, filter, page).await.map_err(map_store)
    }

    /// `POST /v1/sessions/{id}`. A field left out keeps its value; the agent's
    /// tools and MCP servers change only while the session is not running.
    pub async fn update(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: UpdateSessionRequest,
    ) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let mut s = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        guard_writable(&s)?;
        if req.vault_ids.is_some() {
            return Err(ApiError::invalid_request("vault_ids: updating vault_ids is not yet supported"));
        }
        s.title = req.title.apply_to_option(s.title);
        s.metadata = s.metadata.merged(req.metadata);
        s.metadata.validate(MetadataPolicy::unlimited_entries())?;
        if let Some(agent) = req.agent {
            if s.status == SessionStatus::Running {
                return Err(ApiError::conflict_invalid_request(
                    "cannot update the agent of a running session",
                ));
            }
            s.agent = agent.apply(s.agent)?;
        }
        s.budget = req.budget.apply_to_option(s.budget);
        s.updated_at = Timestamp::now();
        self.store.replace(&ctx.workspace, s.clone()).await.map_err(map_store)?;
        Ok(s)
    }

    /// `POST /v1/sessions/{id}/archive` — read-only afterwards.
    pub async fn archive(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let mut s = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if s.status == SessionStatus::Running {
            // Documented race: cannot archive while running.
            return Err(ApiError::conflict_invalid_request("cannot archive a running session"));
        }
        if s.archived_at.is_none() {
            s.archived_at = Some(Timestamp::now());
            s.updated_at = Timestamp::now();
            self.store.replace(&ctx.workspace, s.clone()).await.map_err(map_store)?;
            // Drop the broadcast sender — an archived session takes no new
            // events, so keeping its channel would leak the entry (and its
            // ring buffer) for the process lifetime.
            self.channels.lock().remove(&id);
        }
        Ok(s)
    }

    /// `DELETE /v1/sessions/{id}`: removes the session and its events. Open
    /// streams get `session.deleted` as their last event.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let s = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if s.status == SessionStatus::Running {
            return Err(ApiError::conflict_invalid_request("cannot delete a running session"));
        }
        // Log rows first so a crash never orphans events.
        self.events.delete_events(&ctx.workspace, &id).await.map_err(map_store)?;
        let deleted = self.store.delete(&ctx.workspace, &id).await.map_err(map_store)?;
        let mut last = Event::new(SessionEvent::Deleted {}.into());
        last.processed_at = Some(Timestamp::now());
        if let Some(tx) = self.channels.lock().remove(&id) {
            let _ = tx.send(last);
        }
        Ok(deleted)
    }
}
