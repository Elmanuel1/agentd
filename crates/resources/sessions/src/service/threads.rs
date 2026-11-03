//! `/v1/sessions/{id}/threads`: the threads of a session, read from its
//! event log.

use shared::{ApiError, AuthContext, Timestamp, Usage};

use super::{map_store, parse_id, HarnessContract, SessionsService};
use crate::events::{Event, EventPayload, SessionEvent, SpanEvent};
use crate::types::{
    primary_thread_id, Session, SessionThread, ThreadId, ThreadStats, ThreadStatus, ThreadTag,
};

impl SessionsService {
    /// Every thread of the session, the primary one first.
    pub async fn threads(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vec<SessionThread>, ApiError> {
        let id = parse_id(raw_id)?;
        let session = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        let events = self.events.events(&ctx.workspace, &id).await.map_err(map_store)?;
        Ok(threads_projection(&session, &events))
    }

    /// One thread, by id.
    pub async fn thread(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        tid: &str,
    ) -> Result<SessionThread, ApiError> {
        self.threads(ctx, raw_id)
            .await?
            .into_iter()
            .find(|t| t.id.as_str() == tid)
            .ok_or_else(|| ApiError::not_found("session thread not found"))
    }

    /// Archive a secondary thread: it stops taking messages and its slot is
    /// freed. The primary thread lives as long as its session.
    pub async fn archive_thread(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        tid: &str,
    ) -> Result<SessionThread, ApiError> {
        let thread = self.thread(ctx, raw_id, tid).await?;
        if thread.parent_thread_id.is_none() {
            return Err(ApiError::invalid_request(
                "the primary thread cannot be archived; archive the session",
            ));
        }
        if thread.status == ThreadStatus::Running {
            return Err(ApiError::conflict_invalid_request("cannot archive a running thread"));
        }
        if thread.archived_at.is_none() {
            let payload =
                SessionEvent::ThreadStatusTerminated { agent_name: thread.agent.config.name.clone() };
            self.emit(&ctx.workspace, &thread.session_id, Some(&thread.id), payload.into()).await?;
        }
        self.thread(ctx, raw_id, tid).await
    }

    /// The thread's id as its events record it: `None` for the primary
    /// thread, whose events are the session's own.
    pub async fn thread_scope(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        tid: &str,
    ) -> Result<Option<ThreadId>, ApiError> {
        let thread = self.thread(ctx, raw_id, tid).await?;
        Ok(thread.parent_thread_id.map(|_| thread.id))
    }
}

/// The threads the log records: the primary thread, then each thread a
/// coordinator created, with its latest status and summed model usage.
/// A session without a coordinator has no threads.
pub fn threads_projection(session: &Session, events: &[Event]) -> Vec<SessionThread> {
    if session.agent.config.multiagent.is_none() {
        return Vec::new();
    }
    let primary = primary_thread_id(&session.id);
    let mut threads = vec![SessionThread {
        agent: session.agent.clone(),
        archived_at: None,
        created_at: session.created_at,
        id: primary.clone(),
        parent_thread_id: None,
        session_id: session.id.clone(),
        stats: session.stats.into(),
        status: ThreadStatus::Idle,
        tag: ThreadTag::SessionThread,
        updated_at: session.created_at,
        usage: Usage::default(),
    }];
    for ev in events {
        let at = ev.processed_at.unwrap_or(session.created_at);
        let EventPayload::Session(SessionEvent::ThreadCreated { agent_name }) = &ev.payload else {
            continue;
        };
        let (Some(tid), Some(multi)) = (&ev.session_thread_id, &session.agent.config.multiagent) else {
            continue;
        };
        if let Some(agent) = multi.roster_agent(agent_name) {
            threads.push(SessionThread {
                agent: agent.clone(),
                archived_at: None,
                created_at: at,
                id: tid.clone(),
                parent_thread_id: Some(primary.clone()),
                session_id: session.id.clone(),
                stats: ThreadStats::default(),
                status: ThreadStatus::Idle,
                tag: ThreadTag::SessionThread,
                updated_at: at,
                usage: Usage::default(),
            });
        }
    }
    for ev in events {
        let tid = ev.session_thread_id.as_ref();
        let at = ev.processed_at.unwrap_or(session.created_at);
        let status = match &ev.payload {
            EventPayload::Session(SessionEvent::ThreadStatusRunning { .. }) => Some(ThreadStatus::Running),
            EventPayload::Session(SessionEvent::ThreadStatusIdle { .. }) => Some(ThreadStatus::Idle),
            EventPayload::Session(SessionEvent::ThreadStatusTerminated { .. }) => {
                Some(ThreadStatus::Terminated)
            }
            _ => None,
        };
        if let (Some(status), Some(t)) = (status, tid.and_then(|t| threads.iter_mut().find(|x| &x.id == t))) {
            t.status = status;
            t.updated_at = at;
            t.stats.duration_seconds = elapsed(t.created_at, at);
            if status == ThreadStatus::Terminated {
                t.archived_at = Some(at);
            }
        }
        if let EventPayload::Span(SpanEvent::ModelRequestEnd { model_usage, .. }) = &ev.payload {
            let owner = tid.unwrap_or(&primary);
            if let Some(t) = threads.iter_mut().find(|x| &x.id == owner) {
                t.usage.add(&Usage {
                    cache_creation: None,
                    cache_read_input_tokens: model_usage.cache_read_input_tokens,
                    input_tokens: model_usage.input_tokens,
                    output_tokens: model_usage.output_tokens,
                });
            }
        }
    }
    threads
}

fn elapsed(from: Timestamp, to: Timestamp) -> u64 {
    u64::try_from((to.as_datetime() - from.as_datetime()).num_seconds()).unwrap_or(0)
}
