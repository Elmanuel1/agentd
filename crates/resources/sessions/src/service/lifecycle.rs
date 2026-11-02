use shared::limits::required;
use shared::{ApiError, AuthContext, Pagination, Timestamp, Usage};

use super::{guard_writable, map_store, parse_id, SessionsService};
use crate::types::{
    CreateSessionRequest, Session, SessionId, SessionResource, SessionResourceId, SessionResourceInput,
    SessionStatus, SessionTag, Stats, UpdateSessionRequest,
};

impl SessionsService {
    /// `POST /v1/sessions`.
    pub async fn create(&self, ctx: &AuthContext, req: CreateSessionRequest) -> Result<Session, ApiError> {
        let agent_ref = required("agent", req.agent)?;
        let environment_id = required("environment_id", req.environment_id)?;
        // Sessions do NOT cap metadata entries (live divergence) but key and
        // value length rules still apply.
        req.metadata.validate(shared::metadata::MetadataPolicy::unlimited_entries())?;
        let agent = self.deps.resolve_agent(ctx, agent_ref.id(), agent_ref.version()).await?;
        self.deps.check_environment(ctx, &environment_id).await?;
        let resources = req.resources.into_iter().map(assign_resource_id).collect::<Vec<_>>();
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
        self.store.insert(&ctx.workspace, session.clone()).await.map_err(map_store)?;
        Ok(session)
    }

    /// `GET /v1/sessions/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/sessions`.
    pub async fn list(&self, ctx: &AuthContext, page: &Pagination) -> Result<Vec<Session>, ApiError> {
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
            m.validate(shared::metadata::MetadataPolicy::unlimited_entries())?;
            s.metadata = m;
        }
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

    /// `DELETE /v1/sessions/{id}` — removes session, events, checkpoints.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let s = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if s.status == SessionStatus::Running {
            return Err(ApiError::conflict_invalid_request("cannot delete a running session"));
        }
        self.channels.lock().remove(&id);
        self.store.delete(&ctx.workspace, &id).await.map_err(map_store)
    }
}

fn assign_resource_id(input: SessionResourceInput) -> SessionResource {
    let id = Some(SessionResourceId::generate());
    match input {
        SessionResourceInput::File { file_id, mount_path } => SessionResource::File {
            id,
            file_id,
            mount_path: mount_path.unwrap_or_else(|| "/workspace/file".into()),
        },
        SessionResourceInput::GithubRepository { url, mount_path, checkout, .. } => {
            SessionResource::GithubRepository { id, url, mount_path, checkout }
        }
        SessionResourceInput::MemoryStore { memory_store_id, access, instructions } => {
            SessionResource::MemoryStore { id, memory_store_id, access, instructions }
        }
    }
}
