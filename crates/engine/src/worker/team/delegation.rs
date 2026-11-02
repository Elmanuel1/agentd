//! The coordinator's side of delegation: `send_to_agent` opens or follows up
//! a roster agent's thread, and a thread's final text goes back to the
//! coordinator as a message.

use async_trait::async_trait;
use sessions::events::{AgentEvent, EventId, SessionEvent};
use sessions::types::ThreadId;
use shared::{ApiError, ContentBlock};

use super::Team;
use crate::turn::{Delegation, Inbound, ThreadCtx};

#[async_trait]
impl Delegation for Team {
    async fn send(&self, _from: &ThreadCtx, input: &serde_json::Value) -> Result<(EventId, String), String> {
        let text = crate::json::at(input, "/message")
            .as_str()
            .ok_or("send_to_agent: `message` is required")?
            .to_owned();
        let member = match (
            crate::json::at(input, "/session_thread_id").as_str(),
            crate::json::at(input, "/agent_name").as_str(),
        ) {
            (Some(tid), _) => ThreadId::parse(tid)
                .ok()
                .and_then(|id| self.member(&id))
                .filter(|m| m.ctx.scope.is_some())
                .ok_or_else(|| format!("no thread {tid} of yours to send to"))?,
            (None, Some(name)) => self.open(name).await?,
            (None, None) => return Err("send_to_agent: give `agent_name` or `session_thread_id`".into()),
        };
        let content = vec![ContentBlock::text(text.clone())];
        let to = member.ctx.id.clone();
        let sent = AgentEvent::ThreadMessageSent {
            content: content.clone(),
            to_agent_name: Some(member.ctx.name().to_owned()),
            to_session_thread_id: to.clone(),
        };
        let sent = self.message(None, sent).await.map_err(|e| e.message)?;
        // Live: a thread is running before the message it runs on arrives.
        if !self.busy.lock().contains(&to) {
            let running = SessionEvent::ThreadStatusRunning { agent_name: member.ctx.name().to_owned() };
            let ws = &self.primary.ws;
            self.contract
                .emit(ws, &self.primary.session, Some(&to), running.into())
                .await
                .map_err(|e| e.message)?;
        }
        let received = AgentEvent::ThreadMessageReceived {
            content,
            from_agent_name: self.primary.name().to_owned(),
            from_session_thread_id: self.primary.id.clone(),
        };
        let received = self.message(Some(&to), received).await.map_err(|e| e.message)?;
        self.deliver(&to, Some(Inbound { event: received.id.as_str().to_owned(), text }));
        Ok((sent.id, format!("Sent to thread {to}. The reply arrives as a message from that thread.")))
    }

    fn roster(&self) -> String {
        self.roster
            .iter()
            .map(|a| format!("- {}: {}", a.config.name, a.config.description.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn reply(&self, from: &ThreadCtx, text: &str) -> Result<(), ApiError> {
        let content = vec![ContentBlock::text(text)];
        let sent = AgentEvent::ThreadMessageSent {
            content: content.clone(),
            to_agent_name: None,
            to_session_thread_id: self.primary.id.clone(),
        };
        self.message(from.scope.as_ref(), sent).await?;
        let received = AgentEvent::ThreadMessageReceived {
            content,
            from_agent_name: from.name().to_owned(),
            from_session_thread_id: from.id.clone(),
        };
        let received = self.message(None, received).await?;
        let text = format!("Message from {} (thread {}):\n{text}", from.name(), from.id);
        self.deliver(&self.primary.id, Some(Inbound { event: received.id.as_str().to_owned(), text }));
        Ok(())
    }
}
