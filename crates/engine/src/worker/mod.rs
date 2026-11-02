//! The session worker: runs a session's threads until every one is idle or
//! waits on the client, grades its outcome, then leaves the session idle.

use std::collections::HashMap;
use std::sync::Arc;

use sessions::events::{EventPayload, Rubric, StopReason, UserEvent};
use sessions::types::{primary_thread_id, Session, SessionId, SessionStatus};
use sessions::EngineContract;
use shared::ports::sandbox::Sandbox;
use shared::{ApiError, AuthContext, WorkspaceId};
use tokio::sync::{watch, Notify};

use crate::model::ModelClient;
use crate::tools::web::WebTools;
use crate::tools::SandboxTools;
use crate::turn::policy::Toolbox;
use crate::turn::prompt::{credentials_section, system_prompt};
use crate::turn::ThreadCtx;

pub mod assets;
pub mod dispatch;
mod grade;
mod intake;
mod outcome;
mod run;
pub mod setup;
pub mod team;

pub use assets::{SandboxEgress, SessionAssets, SkillFiles};
pub use dispatch::Dispatcher;

/// Everything a session's agent loop runs on.
pub struct Engine {
    /// The sessions service.
    pub contract: Arc<dyn EngineContract>,
    /// The model providers.
    pub models: Arc<dyn ModelClient>,
    /// The sandboxes.
    pub sandbox: Arc<dyn Sandbox>,
    /// Files, skills, environments and outputs.
    pub assets: Arc<dyn SessionAssets>,
    /// The client `web_fetch` and `web_search` use.
    pub http: reqwest::Client,
    /// A Brave Search API key, when `web_search` is configured.
    pub search_key: Option<String>,
    /// The output cap per model call.
    pub max_tokens: u32,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").field("max_tokens", &self.max_tokens).finish_non_exhaustive()
    }
}

/// One run of one session's loop.
pub(crate) struct SessionRun {
    pub(crate) engine: Arc<Engine>,
    /// The caller whose events started the run; the loop reads files,
    /// skills and the environment as it.
    pub(crate) ctx: AuthContext,
    pub(crate) ws: WorkspaceId,
    pub(crate) session: SessionId,
    pub(crate) tools: SandboxTools,
    pub(crate) primary_model: String,
    pub(crate) egress: SandboxEgress,
}

impl SessionRun {
    async fn emit(&self, payload: impl Into<EventPayload> + Send) -> Result<sessions::Event, ApiError> {
        self.engine.contract.emit(&self.ws, &self.session, None, payload.into()).await
    }
}

/// The hosts a `limited` environment allows; `None` when unrestricted.
fn allowed_hosts(environment: &serde_json::Value) -> Option<Vec<String>> {
    let net = environment.get("networking")?;
    (crate::json::at(net, "/type").as_str() == Some("limited")).then(|| {
        crate::json::at(net, "/allowed_hosts")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| h.as_str().map(str::to_owned))
            .collect()
    })
}

impl Engine {
    /// Run `session` until it is idle again. Call only under its lease.
    pub async fn run_session(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &SessionId,
        wake: Arc<Notify>,
    ) -> Result<(), ApiError> {
        let ws = &ctx.workspace;
        let session = self.contract.session(ws, id).await?;
        if session.archived_at.is_some() || session.status == SessionStatus::Terminated {
            return Ok(());
        }
        let environment = self.assets.environment(ctx, &session.environment_id).await?;
        let web = WebTools::new(self.http.clone(), allowed_hosts(&environment), self.search_key.clone());
        let egress = self.assets.egress(ctx, id.as_str()).await?;
        let run = SessionRun {
            engine: self.clone(),
            ctx: ctx.clone(),
            ws: ws.clone(),
            session: id.clone(),
            tools: SandboxTools::new(self.sandbox.clone(), id.as_str(), web).with_env(egress.env.clone()),
            primary_model: session.agent.config.model.id.clone(),
            egress,
        };
        run.drive(session, wake).await
    }
}

impl SessionRun {
    /// Rubric file texts for the session's outcomes.
    async fn rubrics(&self, events: &[sessions::Event]) -> Result<HashMap<String, String>, ApiError> {
        let mut out = HashMap::new();
        for e in events {
            if let EventPayload::User(UserEvent::DefineOutcome { rubric: Rubric::File { file_id }, .. }) =
                &e.payload
            {
                if !out.contains_key(file_id) {
                    let bytes = self.engine.assets.file(&self.ctx, file_id).await?;
                    out.insert(file_id.clone(), String::from_utf8_lossy(&bytes).into_owned());
                }
            }
        }
        Ok(out)
    }

    /// The primary thread of `session`.
    fn primary(
        &self,
        session: &Session,
        skills: &HashMap<String, Vec<crate::turn::prompt::SkillInfo>>,
    ) -> ThreadCtx {
        let agent = session.agent.clone();
        let roster: Vec<_> = agent.config.multiagent.as_ref().map(|m| m.roster_agents()).unwrap_or_default();
        let own = skills.get(&agent.config.name).cloned().unwrap_or_default();
        ThreadCtx {
            ws: self.ws.clone(),
            session: self.session.clone(),
            id: primary_thread_id(&session.id),
            scope: None,
            toolbox: Toolbox::for_agent(&agent, !roster.is_empty()),
            system: system_prompt(&agent, &own, &roster, &credentials_section(&self.egress.credentials)),
            threaded: !roster.is_empty(),
            agent,
        }
    }

    /// Leave the session idle for `stop_reason`, after copying out its
    /// outputs.
    async fn settle(&self, stop_reason: StopReason) -> Result<(), ApiError> {
        if let Err(e) = self.sync_outputs(self.session.as_str()).await {
            tracing::warn!(session = %self.session, error = %e.message, "engine: could not copy the session's outputs");
        }
        self.engine.contract.set_status(&self.ws, &self.session, SessionStatus::Idle, Some(stop_reason)).await
    }

    fn cancel_on(drained: &[sessions::Event], tx: &watch::Sender<bool>) -> bool {
        let interrupted =
            drained.iter().any(|e| matches!(e.payload, EventPayload::User(UserEvent::Interrupt {})));
        if interrupted {
            let _ = tx.send(true);
        }
        interrupted
    }
}

#[cfg(test)]
mod tests;
