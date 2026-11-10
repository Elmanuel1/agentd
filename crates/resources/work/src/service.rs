//! The work API's rules: who reaches a work item, and the reads and metadata
//! writes on it. Its lifecycle (poll, ack, heartbeat, stop) is in
//! `lifecycle.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use shared::{ApiError, AuthContext, Page, Pagination, Timestamp};

use crate::caller::WorkCaller;
use crate::secret::CloudVariables;
use crate::store::{StoreError, WorkItem, WorkStore, HOSTED};
use crate::tokens::{WorkClaims, WorkTokens};
use crate::types::{QueueStats, StatsTag, Work, WorkData, WorkDataTag, WorkState, WorkTag};

/// Work items over their store.
pub struct WorkService {
    pub(crate) store: Arc<dyn WorkStore>,
    pub(crate) tokens: WorkTokens,
    pub(crate) api_base_url: String,
    pub(crate) cloud: Option<CloudVariables>,
}

impl std::fmt::Debug for WorkService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkService").field("api_base_url", &self.api_base_url).finish_non_exhaustive()
    }
}

pub(crate) fn not_found() -> ApiError {
    ApiError::not_found("work not found")
}

pub(crate) fn backend(e: StoreError) -> ApiError {
    ApiError::internal(e.to_string())
}

impl WorkService {
    /// Items in `store`, tokens signed by `tokens`; a work secret's
    /// `api_base_url` is `api_base_url`.
    pub fn new(store: Arc<dyn WorkStore>, tokens: WorkTokens, api_base_url: impl Into<String>) -> Self {
        Self { store, tokens, api_base_url: api_base_url.into(), cloud: None }
    }

    /// Give runtime hand-outs their `environment_variables` from `cloud`.
    #[must_use]
    pub fn with_cloud_variables(self, cloud: CloudVariables) -> Self {
        Self { cloud: Some(cloud), ..self }
    }

    /// The token signer, shared with the edge.
    pub fn tokens(&self) -> &WorkTokens {
        &self.tokens
    }

    /// The store, which the edge reads a token's item from.
    pub fn store(&self) -> Arc<dyn WorkStore> {
        self.store.clone()
    }

    /// Queue `session`'s work in `environment_id` on `queue`: its item, with
    /// the session's id, `started_at` set at once as on live.
    pub async fn queue(
        &self,
        ctx: &AuthContext,
        environment_id: &str,
        session: &str,
        queue: &str,
    ) -> Result<Work, ApiError> {
        let now = Timestamp::now();
        let work = Work {
            id: session.to_owned(),
            acknowledged_at: None,
            actor: None,
            created_at: now,
            data: WorkData { id: session.to_owned(), tag: WorkDataTag::Session },
            environment_id: environment_id.to_owned(),
            latest_heartbeat_at: None,
            metadata: BTreeMap::new(),
            secret: None,
            started_at: Some(now),
            state: WorkState::Queued,
            stop_requested_at: None,
            stopped_at: None,
            tag: WorkTag::Work,
        };
        let item = WorkItem {
            workspace: ctx.workspace.clone(),
            org: ctx.org.clone(),
            queue: queue.to_owned(),
            work: work.clone(),
            last_heartbeat: None,
            lease_until: None,
            handed_out_at: None,
        };
        self.store.insert(&item).await.map_err(backend)?;
        Ok(work)
    }

    /// Queue the work of a session whose own sandbox's harness takes it,
    /// with the secret that sandbox starts with.
    pub async fn issue(
        &self,
        ctx: &AuthContext,
        environment_id: &str,
        session: &str,
    ) -> Result<(Work, String), ApiError> {
        let work = self.queue(ctx, environment_id, session, HOSTED).await?;
        let claims = WorkClaims {
            session: session.to_owned(),
            ws: ctx.workspace.as_str().to_owned(),
            org: ctx.org.as_str().to_owned(),
        };
        Ok((work, self.tokens.secret(&claims, &self.api_base_url, &BTreeMap::new())))
    }

    /// `GET …/work/{id}`.
    pub async fn retrieve(&self, caller: &WorkCaller, env: &str, id: &str) -> Result<Work, ApiError> {
        let (ws, reach) = self.target(caller, env, id)?;
        let item = self.store.get(&ws, id).await.map_err(backend)?.ok_or_else(not_found)?;
        let reached = reach.environment.as_ref() == Some(&item.work.environment_id)
            || reach.queue.as_ref() == Some(&item.queue);
        reached.then_some(item.work).ok_or_else(not_found)
    }

    /// `GET …/work`: the environment's or queue's work, newest first.
    pub async fn list(
        &self,
        caller: &WorkCaller,
        env: &str,
        page: &Pagination,
    ) -> Result<Page<Work>, ApiError> {
        if matches!(caller, WorkCaller::Token(_)) {
            return Err(not_found());
        }
        let items = self.store.list(&crate::caller::scope(caller, env)?, page).await.map_err(backend)?;
        Ok(Page::from_fetched(items.into_iter().map(|i| i.work).collect(), page))
    }

    /// `POST …/work/{id}`: merge metadata (`null` deletes a key).
    pub async fn update(
        &self,
        caller: &WorkCaller,
        env: &str,
        id: &str,
        patch: BTreeMap<String, Option<String>>,
    ) -> Result<Work, ApiError> {
        if matches!(caller, WorkCaller::Token(_)) {
            return Err(not_found());
        }
        let (ws, reach) = self.target(caller, env, id)?;
        let remove: Vec<String> = patch.iter().filter(|(_, v)| v.is_none()).map(|(k, _)| k.clone()).collect();
        let set: BTreeMap<String, String> = patch.into_iter().filter_map(|(k, v)| Some((k, v?))).collect();
        let item = self.store.update_metadata(&ws, id, &reach, &set, &remove).await.map_err(backend)?;
        item.map(|i| i.work).ok_or_else(not_found)
    }

    /// `GET …/work/stats`.
    pub async fn stats(&self, caller: &WorkCaller, env: &str) -> Result<QueueStats, ApiError> {
        if matches!(caller, WorkCaller::Token(_)) {
            return Err(not_found());
        }
        let counts = self
            .store
            .counts(&crate::caller::scope(caller, env)?, crate::lifecycle::RECLAIM, Timestamp::now())
            .await
            .map_err(backend)?;
        Ok(QueueStats {
            depth: counts.depth,
            oldest_queued_at: counts.oldest_queued_at,
            pending: counts.pending,
            tag: StatsTag::WorkQueueStats,
            workers_polling: None,
        })
    }
}
