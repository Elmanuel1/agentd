//! The loop end to end over in-memory fakes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sessions::events::{AgentEvent, EventId, EventPayload, SessionEvent, StopReason, UserEvent};
use sessions::types::Session;
use sessions::TurnTrigger;
use shared::ports::lease::LocalLeases;
use shared::ports::sandbox::ExecOutput;
use shared::{AuthContext, ContentBlock, OrgId, WorkspaceId};

use crate::model::{Block, ModelClient, ModelResponse, StopKind};
use crate::testing::{FakeAssets, FakeSandbox, FakeSession};
use crate::worker::{Dispatcher, Engine, SkillFiles};
use crate::MockModel;

mod flows;
mod multiagent;

pub(super) fn say(text: &str) -> ModelResponse {
    ModelResponse::text(text)
}

pub(super) fn call(name: &str, input: Value) -> ModelResponse {
    ModelResponse {
        content: vec![Block::ToolUse { id: format!("toolu_{name}"), name: name.into(), input }],
        stop: StopKind::ToolUse,
        usage: sessions::events::ModelUsage { input_tokens: 10, output_tokens: 5, ..Default::default() },
        thought: true,
    }
}

pub(super) fn agent(name: &str, tools: Value, multiagent: Value) -> Value {
    json!({
        "id": "agent_011111111111111111111111", "type": "agent", "version": 1, "name": name,
        "model": {"id": "claude-haiku-4-5", "speed": "standard"}, "description": format!("{name} does work"),
        "system": format!("You are {name}."), "tools": tools, "mcp_servers": [],
        "skills": [{"type": "custom", "skill_id": "skill_01", "version": "latest"}], "multiagent": multiagent,
    })
}

pub(super) fn toolset(policy: &str) -> Value {
    json!([{"type": "agent_toolset_20260401", "configs": [],
            "default_config": {"enabled": true, "permission_policy": {"type": policy}}}])
}

pub(super) fn session(agent: Value) -> Session {
    serde_json::from_value(json!({
        "agent": agent, "archived_at": null, "created_at": "2026-11-02T00:00:00Z",
        "environment_id": "env_011111111111111111111111", "id": "sesn_01AAAAAAAAAAAAAAAAAAAAAA",
        "metadata": {}, "outcome_evaluations": [],
        "resources": [{"type": "file", "file_id": "file_01doc", "mount_path": "/extraction/docs/a.json"}],
        "stats": {"active_seconds": 0, "duration_seconds": 0}, "status": "idle", "title": null,
        "type": "session", "updated_at": "2026-11-02T00:00:00Z",
        "usage": {"cache_creation": null, "cache_read_input_tokens": 0, "input_tokens": 0, "output_tokens": 0},
        "vault_ids": [],
    }))
    .unwrap()
}

/// A session with its fakes and a dispatcher.
pub(super) struct Rig {
    pub session: FakeSession,
    pub sandbox: FakeSandbox,
    pub assets: FakeAssets,
    pub leases: Arc<LocalLeases>,
    pub dispatcher: Dispatcher,
    pub ctx: AuthContext,
}

impl Rig {
    pub fn new(session: Session, models: Arc<dyn ModelClient>, sandbox: FakeSandbox) -> Self {
        let session = FakeSession::new(session);
        let assets = FakeAssets::default();
        assets.files.lock().insert("file_01doc".into(), b"{\"pages\": 1}".to_vec());
        let skill_md = b"---\nname: citing-sources\ndescription: Cites pages.\n---\n# Citing".to_vec();
        assets.skills.lock().insert(
            "skill_01".into(),
            SkillFiles { name: "citing-sources".into(), files: vec![("SKILL.md".into(), skill_md)] },
        );
        *assets.environment.lock() =
            json!({"type": "cloud", "networking": {"type": "limited", "allowed_hosts": []}});
        let engine = Arc::new(Engine {
            contract: Arc::new(session.clone()),
            models,
            sandbox: Arc::new(sandbox.clone()),
            assets: Arc::new(assets.clone()),
            http: reqwest::Client::new(),
            search_key: None,
            max_tokens: 1024,
        });
        let leases = Arc::new(LocalLeases::default());
        let dispatcher = Dispatcher::new(engine, leases.clone());
        Self {
            session,
            sandbox,
            assets,
            leases,
            dispatcher,
            ctx: AuthContext {
                org: OrgId::new("org_test"),
                workspace: WorkspaceId::new("wrkspc_01test"),
                api_key_id: "apikey_test".into(),
            },
        }
    }

    pub fn scripted(
        session: Session,
        scripts: Vec<(&str, Vec<ModelResponse>)>,
        sandbox: FakeSandbox,
    ) -> (Self, Arc<MockModel>) {
        let mock = Arc::new(MockModel::per_agent(
            scripts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<HashMap<_, _>>(),
        ));
        (Self::new(session, mock.clone(), sandbox), mock)
    }

    /// Send a client event and wait until the session loop is done with it.
    pub async fn send(&self, payload: impl Into<EventPayload>) -> EventId {
        let id = self.session.send(payload).id;
        self.dispatcher.wake(&self.ctx, &self.session.get().id);
        self.idle().await;
        id
    }

    pub async fn idle(&self) {
        let id = self.session.get().id;
        for _ in 0..500 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if !self.dispatcher.is_running(&id) {
                return;
            }
        }
        panic!("the session loop did not finish: {:?}", self.session.types());
    }

    pub fn last_stop(&self) -> StopReason {
        self.session
            .log()
            .iter()
            .rev()
            .find_map(|e| match &e.payload {
                EventPayload::Session(SessionEvent::StatusIdle { stop_reason }) => Some(stop_reason.clone()),
                _ => None,
            })
            .unwrap()
    }

    pub fn messages(&self) -> Vec<String> {
        self.session
            .log()
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::Agent(AgentEvent::Message { content }) => {
                    content[0].as_text().map(str::to_owned)
                }
                _ => None,
            })
            .collect()
    }
}

pub(super) fn message(text: &str) -> UserEvent {
    UserEvent::Message { content: vec![ContentBlock::text(text)] }
}

pub(super) fn answering(stdout: &'static str) -> FakeSandbox {
    FakeSandbox::new(move |_| ExecOutput { stdout: stdout.as_bytes().to_vec(), ..ExecOutput::default() })
}
