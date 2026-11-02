//! The model's tool calls: each is recorded, then run now, parked for the
//! client, or delegated to a roster thread, as its disposition says.

use sessions::events::{AgentEvent, EventId};
use shared::ApiError;

use super::policy::{Disposition, LIST_AGENTS};
use super::step::StepEnd;
use super::{result_content, Runner, ThreadCtx, TurnEnd};
use crate::model::{Block, Message, ModelResponse, Role};
use crate::tools::{ToolCall, ToolOutcome};

impl Runner {
    /// Record the model's calls, run the allowed ones and park the rest.
    pub(super) async fn act(
        &self,
        t: &ThreadCtx,
        messages: &mut Vec<Message>,
        response: ModelResponse,
        text: Option<String>,
    ) -> Result<StepEnd, ApiError> {
        let mut assistant: Vec<Block> = text.iter().map(|s| Block::text(s.clone())).collect();
        let mut results = Vec::new();
        let mut run = Vec::new();
        let mut blocked: Vec<EventId> = Vec::new();
        for (_, name, input) in response.tool_uses() {
            // `logged`: the result is already in the log (a delegation's
            // `agent.thread_message_sent` stands for its call and result).
            let (id, result, logged) = match t.toolbox.disposition(name) {
                Some(Disposition::Delegate) if name == LIST_AGENTS => {
                    let roster = self.delegation.as_ref().map(|d| d.roster()).unwrap_or_default();
                    (
                        self.tool_use(t, name, input, "allow", None).await?,
                        Some(ToolOutcome::ok(roster)),
                        false,
                    )
                }
                Some(Disposition::Delegate) => {
                    let sent = match &self.delegation {
                        Some(d) => d.send(t, input).await,
                        None => Err("this agent has no roster".to_owned()),
                    };
                    match sent {
                        Ok((ev, note)) => (ev.as_str().to_owned(), Some(ToolOutcome::ok(note)), true),
                        Err(why) => (
                            self.tool_use(t, name, input, "allow", None).await?,
                            Some(ToolOutcome::err(why)),
                            false,
                        ),
                    }
                }
                Some(Disposition::Allow { evaluation }) => {
                    let id = self.tool_use(t, name, input, "allow", evaluation.clone()).await?;
                    run.push((id.clone(), name.to_owned(), input.clone()));
                    (id, None, false)
                }
                Some(Disposition::Ask) => {
                    let id = self.tool_use(t, name, input, "ask", None).await?;
                    blocked.push(EventId::parse(&id).map_err(|e| ApiError::internal(e.to_string()))?);
                    (id, None, false)
                }
                Some(Disposition::Custom) => {
                    let ev = self
                        .emit(t, AgentEvent::CustomToolUse { name: name.to_owned(), input: input.clone() })
                        .await?;
                    blocked.push(ev.id.clone());
                    (ev.id.as_str().to_owned(), None, false)
                }
                None => {
                    let id = self.tool_use(t, name, input, "allow", None).await?;
                    (id, Some(ToolOutcome::err(format!("unknown tool: {name}"))), false)
                }
            };
            assistant.push(Block::ToolUse { id: id.clone(), name: name.to_owned(), input: input.clone() });
            if let Some(outcome) = result {
                if !logged {
                    self.tool_result(t, &id, &outcome).await?;
                }
                results.push(Block::ToolResult {
                    tool_use_id: id,
                    content: outcome.output,
                    is_error: outcome.is_error,
                });
            }
        }
        let called = assistant.iter().any(|b| matches!(b, Block::ToolUse { .. }));
        messages.push(Message { role: Role::Assistant, content: assistant });
        for (id, name, input) in run {
            if *self.cancel.borrow() {
                return Ok(StepEnd::Stopped(TurnEnd::Interrupted));
            }
            let call = ToolCall { id: id.clone(), name, input, thread: t.id.as_str().to_owned() };
            let mut cancel = self.cancel.clone();
            let outcome = tokio::select! {
                o = self.tools.execute(&call) => o,
                _ = cancel.wait_for(|c| *c) => return Ok(StepEnd::Stopped(TurnEnd::Interrupted)),
            };
            self.tool_result(t, &id, &outcome).await?;
            results.push(Block::ToolResult {
                tool_use_id: id,
                content: outcome.output,
                is_error: outcome.is_error,
            });
        }
        if !results.is_empty() {
            messages.push(Message { role: Role::User, content: results });
        }
        if !blocked.is_empty() {
            return Ok(StepEnd::Stopped(TurnEnd::Blocked(blocked)));
        }
        Ok(if called { StepEnd::Continue { text } } else { StepEnd::Done { text: text.unwrap_or_default() } })
    }

    async fn tool_use(
        &self,
        t: &ThreadCtx,
        name: &str,
        input: &serde_json::Value,
        permission: &str,
        evaluation: Option<serde_json::Value>,
    ) -> Result<String, ApiError> {
        let ev = AgentEvent::ToolUse {
            name: name.to_owned(),
            input: input.clone(),
            evaluated_permission: Some(permission.into()),
            evaluation,
        };
        Ok(self.emit(t, ev).await?.id.as_str().to_owned())
    }

    pub(super) async fn tool_result(
        &self,
        t: &ThreadCtx,
        id: &str,
        outcome: &ToolOutcome,
    ) -> Result<(), ApiError> {
        let tool_use_id = EventId::parse(id).map_err(|e| ApiError::internal(e.to_string()))?;
        let ev = AgentEvent::ToolResult {
            tool_use_id,
            content: result_content(outcome),
            is_error: outcome.is_error,
        };
        self.emit(t, ev).await.map(|_| ())
    }
}
