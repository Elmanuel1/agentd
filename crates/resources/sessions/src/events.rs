//! The session event protocol: one exhaustive enum per event category.
//!
//! Envelope (observed): every event carries `id` (`sevt_…`), `type`
//! (dot-namespaced), and `processed_at` (null until the agent consumes a
//! client-sent event). Payload fields are flattened beside the envelope.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use shared::{Id, Resource, Timestamp};

use crate::types::ThreadId;

mod agent;
mod fields;
mod session;
mod span;
mod user;

pub use agent::AgentEvent;
pub use fields::{ConfirmationResult, ModelUsage, Rubric, StopReason};
pub use session::SessionEvent;
pub use span::SpanEvent;
pub use user::UserEvent;

/// Marker for `sevt_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct EventResource_;
impl Resource for EventResource_ {
    const PREFIX: &'static str = "sevt";
    const LABEL: &'static str = "event";
}
/// `Id<EventResource_>` alias.
pub type EventId = Id<EventResource_>;

/// Every event on the wire, by category. The `type` prefix names the
/// category (`system.message` is a client event), so a new event is a variant
/// of one category enum and its consumers match only that category.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum EventPayload {
    /// Sent by the client.
    User(UserEvent),
    /// Emitted by the agent.
    Agent(AgentEvent),
    /// Session status changes and errors.
    Session(SessionEvent),
    /// Timing spans.
    Span(SpanEvent),
}

impl<'de> Deserialize<'de> for EventPayload {
    /// Dispatch on the `type` prefix, so serde's own messages for the
    /// category (`missing field \`content\``) reach the caller unchanged.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let kind = match value.get("type").and_then(serde_json::Value::as_str) {
            Some(t) => t.to_owned(),
            None => return Err(D::Error::missing_field("type")),
        };
        let payload = match kind.split('.').next().unwrap_or_default() {
            "user" | "system" => serde_json::from_value(value).map(Self::User),
            "agent" => serde_json::from_value(value).map(Self::Agent),
            "session" => serde_json::from_value(value).map(Self::Session),
            "span" => serde_json::from_value(value).map(Self::Span),
            _ => return Err(D::Error::custom(format!("unknown variant `{kind}`"))),
        };
        payload.map_err(D::Error::custom)
    }
}

macro_rules! from_category {
    ($($category:ident => $variant:ident),+) => {$(
        impl From<$category> for EventPayload {
            fn from(event: $category) -> Self {
                Self::$variant(event)
            }
        }
    )+};
}

from_category!(UserEvent => User, AgentEvent => Agent, SessionEvent => Session, SpanEvent => Span);

impl EventPayload {
    /// Is this a client-sent event (subject to `processed_at` stamping)?
    pub fn is_user_event(&self) -> bool {
        matches!(self, Self::User(_))
    }

    /// Whether a session waiting on a tool takes it: an answer or an interrupt.
    pub fn answers_or_interrupts(&self) -> bool {
        matches!(
            self,
            Self::User(
                UserEvent::ToolConfirmation { .. }
                    | UserEvent::CustomToolResult { .. }
                    | UserEvent::ToolResult { .. }
                    | UserEvent::Interrupt {}
            )
        )
    }

    /// The call a client's result answers, and whether it is a custom tool's.
    pub fn answered_call(&self) -> Option<(&EventId, bool)> {
        match self {
            Self::User(UserEvent::ToolResult { tool_use_id, .. }) => Some((tool_use_id, false)),
            Self::User(UserEvent::CustomToolResult { custom_tool_use_id, .. }) => {
                Some((custom_tool_use_id, true))
            }
            _ => None,
        }
    }
}

/// The full wire event: envelope + flattened payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// `sevt_…`.
    pub id: EventId,
    /// Set when the agent consumes the event. Live OMITS this field until the
    /// event is processed (client-sent events appear twice on the stream:
    /// queued without it, then processed with it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed_at: Option<Timestamp>,
    /// The thread the event belongs to: set on thread lifecycle events and
    /// on every event of a thread other than the primary one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_thread_id: Option<ThreadId>,
    /// The typed payload, flattened beside the envelope.
    #[serde(flatten)]
    pub payload: EventPayload,
}

impl Event {
    /// Wrap a payload with a fresh id and no processed stamp.
    pub fn new(payload: EventPayload) -> Self {
        Self { id: EventId::generate(), processed_at: None, session_thread_id: None, payload }
    }

    /// Whether the session's own stream shows this event: everything but a
    /// secondary thread's work, which shows only where it blocks (a tool
    /// awaiting confirmation or a custom tool).
    pub fn on_session_stream(&self) -> bool {
        self.session_thread_id.is_none()
            || match &self.payload {
                EventPayload::Session(_) => true,
                EventPayload::Agent(AgentEvent::ToolUse { evaluated_permission, .. }) => {
                    evaluated_permission.as_deref() == Some("ask")
                }
                EventPayload::Agent(AgentEvent::CustomToolUse { .. }) => true,
                _ => false,
            }
    }

    /// Whether thread `thread`'s stream shows this event; `None` names the
    /// primary thread, whose stream is the session's.
    pub fn on_thread_stream(&self, thread: Option<&ThreadId>) -> bool {
        match thread {
            None => self.on_session_stream(),
            Some(t) => {
                self.session_thread_id.as_ref() == Some(t)
                    && !matches!(self.payload, EventPayload::Session(SessionEvent::ThreadCreated { .. }))
            }
        }
    }

    /// The event as a thread's stream shows it: the thread id stays only on
    /// lifecycle events, as live sends them.
    pub fn thread_wire(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if !matches!(self.payload, EventPayload::Session(_)) {
            if let Some(map) = v.as_object_mut() {
                map.remove("session_thread_id");
            }
        }
        v
    }
}

#[cfg(test)]
mod tests;
