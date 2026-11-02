//! The session event protocol: one exhaustive enum per event category.
//!
//! Envelope (observed): every event carries `id` (`sevt_…`), `type`
//! (dot-namespaced), and `processed_at` (null until the agent consumes a
//! client-sent event). Payload fields are flattened beside the envelope.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use shared::{Id, Resource, Timestamp};

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
        let category = match value.get("type").and_then(serde_json::Value::as_str) {
            Some(t) => t.split('.').next().unwrap_or_default().to_owned(),
            None => return Err(D::Error::missing_field("type")),
        };
        let payload = match category.as_str() {
            "user" | "system" => serde_json::from_value(value).map(Self::User),
            "agent" => serde_json::from_value(value).map(Self::Agent),
            "session" => serde_json::from_value(value).map(Self::Session),
            "span" => serde_json::from_value(value).map(Self::Span),
            _ => return Err(D::Error::custom(format!("unknown variant `{category}`"))),
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
    /// The typed payload, flattened beside the envelope.
    #[serde(flatten)]
    pub payload: EventPayload,
}

impl Event {
    /// Wrap a payload with a fresh id and no processed stamp.
    pub fn new(payload: EventPayload) -> Self {
        Self { id: EventId::generate(), processed_at: None, payload }
    }
}

#[cfg(test)]
mod tests;
