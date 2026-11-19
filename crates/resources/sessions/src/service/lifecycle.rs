use shared::limits::required;
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, Pagination, Timestamp, Usage};

use super::egress::{FireRefusal, check_mcp_egress};
use super::events::validate_sendable;
use super::{SessionsService, guard_writable, map_store, parse_id};
use crate::events::{EventPayload, UserEvent};
use crate::store::{SessionFilter, StoreError};
use crate::types::{
    AgentRefInput, CreateSessionRequest, Session, SessionId, SessionStatus, SessionTag, Stats,
    UpdateSessionRequest,
};

impl SessionsService {
    /// `POST /v1/sessions`. `initial_events` are queued as the first input.
    pub async fn create(&self, ctx: &AuthContext, req: CreateSessionRequest) -> Result<Session, ApiError> {
        self.create_fired(ctx, req).await.map_err(FireRefusal::into_api)
    }

    /// [`Self::create`], refusing a deployment's session whose agent names an
    /// MCP host the environment blocks (MC-18) as its own case.
    pub async fn create_fired(
        &self,
        ctx: &AuthContext,
        req: CreateSessionRequest,
    ) -> Result<Session, FireRefusal> {
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
            ))
            .into());
        }
        if let Some(budget) = &req.budget {
            budget.max_minor_units()?;
        }
        let initial: Vec<EventPayload> = req.initial_events.into_iter().map(EventPayload::User).collect();
        validate_sendable("initial_events", &initial)?;
        let requested = req.id.is_some();
        if let Some(made) = self.made(ctx, req.id.as_ref(), &initial).await? {
            return Ok(made);
        }
        let mut agent = self.deps.resolve_agent(ctx, agent_ref.id(), agent_ref.version()).await?;
        if let AgentRefInput::Tagged(overrides) = agent_ref {
            agent = overrides.apply(agent, &self.catalog)?;
        }
        let environment = self.deps.resolve_environment(ctx, &environment_id).await?;
        if req.deployment_id.is_some() {
            check_mcp_egress(&agent, &environment)?;
        }
        self.deps.check_vaults(ctx, &req.vault_ids).await?;
        let now = Timestamp::now();
        let session = Session {
            agent,
            archived_at: None,
            budget: req.budget,
            created_at: now,
            environment_id,
            id: req.id.unwrap_or_else(SessionId::generate),
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
        self.queue_work(ctx, &session, &environment).await?;
        match self.store.insert(&ctx.workspace, session.clone()).await {
            // An overlapping create of the same requested id inserted first:
            // the work item is that session's, so it stays queued (DS-20).
            Err(StoreError::Conflict) if requested => {
                let made = self.made(ctx, Some(&session.id), &initial).await?;
                return made.ok_or_else(|| map_store(StoreError::NotFound).into());
            }
            Err(e) => {
                self.end_work(&ctx.workspace, &session.id).await;
                return Err(map_store(e).into());
            }
            Ok(()) => {}
        }
        self.send_initial(ctx, &session, &initial).await?;
        Ok(session)
    }

    async fn made(
        &self,
        ctx: &AuthContext,
        id: Option<&SessionId>,
        initial: &[EventPayload],
    ) -> Result<Option<Session>, ApiError> {
        let Some(id) = id else { return Ok(None) };
        let session = match self.store.get(&ctx.workspace, id).await {
            Ok(session) => session,
            Err(StoreError::NotFound) => return Ok(None),
            Err(e) => return Err(map_store(e)),
        };
        self.send_initial(ctx, &session, initial).await?;
        Ok(Some(session))
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
        let read = s.clone();
        guard_writable(&s)?;
        if req.vault_ids.is_some() {
            return Err(ApiError::invalid_request("Updating vault_ids is not yet supported."));
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
        s.change_budget(req.budget)?;
        s.updated_at = Timestamp::now();
        self.store.replace(&ctx.workspace, &read, s.clone(), &[]).await.map_err(map_store)?;
        Ok(s)
    }

    /// `POST /v1/sessions/{id}/archive` — read-only afterwards.
    pub async fn archive(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let mut s = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        let read = s.clone();
        if s.status == SessionStatus::Running {
            // Documented race: cannot archive while running.
            return Err(ApiError::conflict_invalid_request("cannot archive a running session"));
        }
        if s.archived_at.is_none() {
            // As live: an archived session is terminated (U3).
            s.archived_at = Some(Timestamp::now());
            s.status = SessionStatus::Terminated;
            s.updated_at = Timestamp::now();
            let kind = webhooks::EventKind::SessionTerminated;
            let deliveries =
                self.webhooks.deliveries(ctx.org.as_str(), ctx.workspace.as_str(), kind, s.id.as_str());
            self.store.replace(&ctx.workspace, &read, s.clone(), &deliveries).await.map_err(map_store)?;
            // Every replica's streams of the session end.
            self.publish(&ctx.workspace, &id).await;
        }
        Ok(s)
    }

    /// `DELETE /v1/sessions/{id}`: removes the session, stops its work item
    /// and deletes its events. Open streams get `session.deleted` as their
    /// last event.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let deleted = self.store.delete(&ctx.workspace, &id).await.map_err(|e| match e {
            StoreError::Conflict => ApiError::conflict_invalid_request("cannot delete a running session"),
            e => map_store(e),
        })?;
        self.publish(&ctx.workspace, &id).await;
        // The session is gone, so no API reaches events left behind: they
        // are logged, not raised.
        if let Err(e) = self.events.delete_events_of(&[(ctx.workspace.clone(), id.clone())]).await {
            tracing::error!(session = %id, error = %e, "sessions: could not delete a deleted session's events");
        }
        Ok(deleted)
    }
}
