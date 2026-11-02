//! Calls that waited on the client: run or refuse each confirmed one, and
//! report those still waiting. A custom tool's answer is a client event the
//! history already reads.

use sessions::events::{ConfirmationResult, Event, EventId, EventPayload, UserEvent};
use shared::ApiError;

use super::history::Unanswered;
use super::{Runner, ThreadCtx};
use crate::tools::{ToolCall, ToolOutcome};

impl Runner {
    /// Settle `owed` from the confirmations in `all`; the calls still
    /// waiting.
    pub(super) async fn resolve(
        &self,
        t: &ThreadCtx,
        all: &[Event],
        owed: &[Unanswered],
    ) -> Result<Vec<EventId>, ApiError> {
        let mut waiting = Vec::new();
        for call in owed {
            let answer = (!call.custom).then(|| confirmation(all, &call.id)).flatten();
            match answer {
                Some((ConfirmationResult::Allow, _)) => {
                    let exec = ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        input: call.input.clone(),
                        thread: t.id.as_str().to_owned(),
                    };
                    let outcome = self.tools.execute(&exec).await;
                    self.tool_result(t, &call.id, &outcome).await?;
                }
                Some((ConfirmationResult::Deny, message)) => {
                    let why = message.unwrap_or_else(|| "The user denied this tool call.".into());
                    self.tool_result(t, &call.id, &ToolOutcome::err(why)).await?;
                }
                None => {
                    waiting.push(EventId::parse(&call.id).map_err(|e| ApiError::internal(e.to_string()))?)
                }
            }
        }
        Ok(waiting)
    }
}

/// The processed confirmation answering tool call `id`.
fn confirmation(all: &[Event], id: &str) -> Option<(ConfirmationResult, Option<String>)> {
    all.iter().rev().find_map(|e| match &e.payload {
        EventPayload::User(UserEvent::ToolConfirmation { tool_use_id, result, deny_message })
            if e.processed_at.is_some() && tool_use_id.as_str() == id =>
        {
            Some((*result, deny_message.clone()))
        }
        _ => None,
    })
}
