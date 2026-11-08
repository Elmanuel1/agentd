//! The agent loop — one session turn.
//!
//! ```text
//! drain user events → running → [ model → (text | tool_use)* ]… → idle
//!                                        │
//!                       policy: allow → execute → tool_result → continue
//!                               ask   → emit tool_use(ask) → idle{requires_action} → STOP
//!                               custom→ emit custom_tool_use → idle{requires_action} → STOP
//! ```
//! Resume: the next `run_turn` drains the confirmation / custom result,
//! resolves the pending tool, and continues the loop.

use std::sync::Arc;

use agents::types::Tool;
use agents::AgentSnapshot;
use sessions::events::{
    AgentEvent, ConfirmationResult, Event, EventId, EventPayload, SpanEvent, StopReason, UserEvent,
};
use sessions::types::{SessionId, SessionStatus};
use sessions::EngineContract;
use shared::{ApiError, ContentBlock, Usage, WorkspaceId};

use crate::model::{ModelClient, ModelMessage, ModelOutput, ModelRequest, ModelTool};
use crate::tools::ToolExecutor;

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// Model finished; session idle with `end_turn`.
    Completed,
    /// Blocked on a client action; session idle with `requires_action`.
    AwaitingAction {
        /// The blocking event.
        blocking_event: EventId,
    },
    /// The user interrupted; session idle immediately.
    Interrupted,
}

/// A tool call parked on `always_ask` / custom, awaiting the client.
#[derive(Debug, Clone)]
struct PendingTool {
    event_id: EventId,
    name: String,
    input: serde_json::Value,
    custom: bool,
}

/// Drives one session's loop through the engine contract.
pub struct TurnRunner {
    contract: Arc<dyn EngineContract>,
    model: Arc<dyn ModelClient>,
    tools: Arc<dyn ToolExecutor>,
    ws: WorkspaceId,
    session_id: SessionId,
    agent: AgentSnapshot,
    history: Vec<ModelMessage>,
    pending: Option<PendingTool>,
    /// Hard cap on model calls per turn (runaway-loop backstop).
    max_model_calls: usize,
}

impl std::fmt::Debug for TurnRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnRunner").field("session", &self.session_id).finish_non_exhaustive()
    }
}

impl TurnRunner {
    /// Append an event to this runner's session and broadcast it.
    async fn emit(&self, payload: impl Into<EventPayload> + Send) -> Result<Event, ApiError> {
        self.contract.emit(&self.ws, &self.session_id, payload.into()).await
    }

    /// Build a runner for one session.
    pub fn new(
        contract: Arc<dyn EngineContract>,
        model: Arc<dyn ModelClient>,
        tools: Arc<dyn ToolExecutor>,
        ws: WorkspaceId,
        session_id: SessionId,
        agent: AgentSnapshot,
    ) -> Self {
        Self {
            contract,
            model,
            tools,
            ws,
            session_id,
            agent,
            history: Vec::new(),
            pending: None,
            max_model_calls: 64,
        }
    }

    /// Run one turn: drain queued events, then loop until the model stops
    /// or a client action is required.
    pub async fn run_turn(&mut self) -> Result<TurnEnd, ApiError> {
        // 1. Drain queued user events.
        let drained = self.contract.drain_user_events(&self.ws, &self.session_id).await?;
        let mut interrupted = false;
        for ev in &drained {
            match &ev.payload {
                EventPayload::User(UserEvent::Interrupt {}) => interrupted = true,
                EventPayload::User(UserEvent::Message { content }) => {
                    self.history.push(ModelMessage { role: "user".into(), content: blocks_to_text(content) });
                }
                EventPayload::User(UserEvent::SystemMessage { content }) => {
                    self.history.push(ModelMessage {
                        role: "user".into(),
                        content: format!("<system>{}</system>", blocks_to_text(content)),
                    });
                }
                EventPayload::User(UserEvent::ToolConfirmation { tool_use_id, result, deny_message }) => {
                    self.resolve_confirmation(tool_use_id.clone(), *result, deny_message.clone()).await?;
                }
                EventPayload::User(UserEvent::CustomToolResult { custom_tool_use_id, content, is_error }) => {
                    self.resolve_custom_result(
                        custom_tool_use_id.clone(),
                        blocks_to_text(content),
                        *is_error,
                    );
                }
                _ => {}
            }
        }
        if interrupted {
            // Interrupt jumps the queue and halts without a model call.
            self.pending = None;
            self.contract
                .set_status(&self.ws, &self.session_id, SessionStatus::Idle, Some(StopReason::EndTurn {}))
                .await?;
            return Ok(TurnEnd::Interrupted);
        }
        // Still blocked? Stay idle on the same requires_action.
        if let Some(p) = &self.pending {
            let blocking = p.event_id.clone();
            self.contract
                .set_status(
                    &self.ws,
                    &self.session_id,
                    SessionStatus::Idle,
                    Some(StopReason::RequiresAction { event_ids: vec![blocking.clone()] }),
                )
                .await?;
            return Ok(TurnEnd::AwaitingAction { blocking_event: blocking });
        }

        // 2. Go to work.
        self.contract.set_status(&self.ws, &self.session_id, SessionStatus::Running, None).await?;

        // 3. The loop.
        for _ in 0..self.max_model_calls {
            let start = self.emit(SpanEvent::ModelRequestStart {}).await?;
            let response = self
                .model
                .complete(ModelRequest {
                    model: self.agent.config.model.id.clone(),
                    system: self.agent.config.system.clone(),
                    messages: self.history.clone(),
                    tools: self.model_tools(),
                })
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
            self.emit(SpanEvent::ModelRequestEnd {
                model_request_start_id: start.id.clone(),
                is_error: false,
                model_usage: response.usage.clone(),
            })
            .await?;
            self.contract
                .add_usage(
                    &self.ws,
                    &self.session_id,
                    &Usage {
                        cache_creation: None,
                        cache_read_input_tokens: response.usage.cache_read_input_tokens,
                        input_tokens: response.usage.input_tokens,
                        output_tokens: response.usage.output_tokens,
                    },
                )
                .await?;

            let had_tool_use = response.has_tool_use();
            for output in response.outputs {
                match output {
                    ModelOutput::Text { text } => {
                        self.history.push(ModelMessage { role: "assistant".into(), content: text.clone() });
                        self.emit(AgentEvent::Message { content: vec![ContentBlock::text(text)] }).await?;
                    }
                    ModelOutput::ToolUse { name, input } => match self.policy_for(&name) {
                        ToolDisposition::Custom => {
                            let ev = self
                                .emit(AgentEvent::CustomToolUse { name: name.clone(), input: input.clone() })
                                .await?;
                            self.pending =
                                Some(PendingTool { event_id: ev.id.clone(), name, input, custom: true });
                            return self.park(ev.id).await;
                        }
                        ToolDisposition::Ask => {
                            let ev = self
                                .emit(AgentEvent::ToolUse {
                                    name: name.clone(),
                                    input: input.clone(),
                                    evaluated_permission: Some("ask".into()),
                                })
                                .await?;
                            self.pending =
                                Some(PendingTool { event_id: ev.id.clone(), name, input, custom: false });
                            return self.park(ev.id).await;
                        }
                        ToolDisposition::Allow => {
                            let ev = self
                                .emit(AgentEvent::ToolUse {
                                    name: name.clone(),
                                    input: input.clone(),
                                    evaluated_permission: Some("allow".into()),
                                })
                                .await?;
                            let outcome = self.tools.execute(&name, &input).await;
                            self.emit(AgentEvent::ToolResult {
                                tool_use_id: ev.id,
                                content: vec![ContentBlock::text(outcome.output.clone())],
                                is_error: outcome.is_error,
                            })
                            .await?;
                            self.history.push(ModelMessage {
                                role: "user".into(),
                                content: format!(
                                    "[tool_result {name}{}] {}",
                                    if outcome.is_error { " ERROR" } else { "" },
                                    outcome.output
                                ),
                            });
                        }
                    },
                }
            }
            if !had_tool_use {
                break; // the model is done
            }
        }

        // 4. Done.
        self.contract
            .set_status(&self.ws, &self.session_id, SessionStatus::Idle, Some(StopReason::EndTurn {}))
            .await?;
        Ok(TurnEnd::Completed)
    }

    async fn park(&self, blocking: EventId) -> Result<TurnEnd, ApiError> {
        self.contract
            .set_status(
                &self.ws,
                &self.session_id,
                SessionStatus::Idle,
                Some(StopReason::RequiresAction { event_ids: vec![blocking.clone()] }),
            )
            .await?;
        Ok(TurnEnd::AwaitingAction { blocking_event: blocking })
    }

    /// Resolve an `always_ask` confirmation: allow → execute now; deny →
    /// feed the denial to the model.
    async fn resolve_confirmation(
        &mut self,
        tool_use_id: EventId,
        result: ConfirmationResult,
        deny_message: Option<String>,
    ) -> Result<(), ApiError> {
        let Some(pending) = self.pending.take() else {
            return Ok(()); // stale confirmation — ignore
        };
        if pending.event_id != tool_use_id || pending.custom {
            self.pending = Some(pending); // not ours — put it back
            return Ok(());
        }
        match result {
            ConfirmationResult::Allow => {
                let outcome = self.tools.execute(&pending.name, &pending.input).await;
                self.emit(AgentEvent::ToolResult {
                    tool_use_id,
                    content: vec![ContentBlock::text(outcome.output.clone())],
                    is_error: outcome.is_error,
                })
                .await?;
                self.history.push(ModelMessage {
                    role: "user".into(),
                    content: format!("[tool_result {}] {}", pending.name, outcome.output),
                });
            }
            ConfirmationResult::Deny => {
                let msg = deny_message.unwrap_or_else(|| "denied by user".into());
                // Statelessness: the denial MUST enter the event log, or a
                // replica reconstructing history from the log would miss it
                // and feed the model a different conversation. Emit it as an
                // errored tool_result carrying the deny reason.
                self.emit(AgentEvent::ToolResult {
                    tool_use_id,
                    content: vec![ContentBlock::text(format!("denied: {msg}"))],
                    is_error: true,
                })
                .await?;
                self.history.push(ModelMessage {
                    role: "user".into(),
                    content: format!("[tool_denied {}] {msg}", pending.name),
                });
            }
        }
        Ok(())
    }

    fn resolve_custom_result(&mut self, custom_tool_use_id: EventId, text: String, is_error: bool) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if pending.event_id != custom_tool_use_id || !pending.custom {
            self.pending = Some(pending);
            return;
        }
        self.history.push(ModelMessage {
            role: "user".into(),
            content: format!(
                "[custom_tool_result {}{}] {text}",
                pending.name,
                if is_error { " ERROR" } else { "" }
            ),
        });
    }

    /// Tool disposition from the agent snapshot's toolset + custom tools.
    ///
    /// Custom tools are matched FIRST across all entries — a custom tool
    /// name must never fall through to the toolset default policy.
    fn policy_for(&self, name: &str) -> ToolDisposition {
        let tools = &self.agent.config.tools;
        if tools.iter().any(|t| matches!(t, Tool::Custom { name: n, .. } if n == name)) {
            return ToolDisposition::Custom;
        }
        // No toolset declared → default allow (executor rejects unknowns).
        match tools.iter().find_map(|t| t.toolset_policy(name)) {
            Some(policy) if policy.asks() => ToolDisposition::Ask,
            _ => ToolDisposition::Allow,
        }
    }

    /// The tool list surfaced to the model.
    fn model_tools(&self) -> Vec<ModelTool> {
        let mut out = Vec::new();
        for tool in &self.agent.config.tools {
            match tool {
                Tool::AgentToolset { .. } => {
                    for name in ["read", "write", "edit", "glob", "grep"] {
                        out.push(ModelTool {
                            name: name.into(),
                            description: format!("workspace {name} tool"),
                            input_schema: serde_json::json!({"type":"object"}),
                        });
                    }
                }
                Tool::Custom { name, description, input_schema } => out.push(ModelTool {
                    name: name.clone(),
                    description: description.clone(),
                    input_schema: input_schema.clone(),
                }),
                Tool::McpToolset { .. } => {
                    // MCP tools surface through the egress proxy — later phase.
                }
            }
        }
        out
    }
}

enum ToolDisposition {
    Allow,
    Ask,
    Custom,
}

fn blocks_to_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
