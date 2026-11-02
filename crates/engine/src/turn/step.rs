//! One step of a turn: a model call and the tool calls it asks for.

use sessions::events::{AgentEvent, SessionEvent, SpanEvent};
use shared::{ApiError, ContentBlock, Usage};

use super::{Runner, ThreadCtx, TurnEnd};
use crate::model::{Block, Message, ModelRequest};

/// How a step ended.
#[derive(Debug)]
pub(super) enum StepEnd {
    /// Tool results are in; call the model again. `text` is what it said.
    Continue {
        /// The step's text, if any.
        text: Option<String>,
    },
    /// The model finished.
    Done {
        /// Its last text.
        text: String,
    },
    /// The turn ends here.
    Stopped(TurnEnd),
}

impl Runner {
    pub(super) async fn step(&self, t: &ThreadCtx, messages: &mut Vec<Message>) -> Result<StepEnd, ApiError> {
        let start = self.emit(t, SpanEvent::ModelRequestStart {}).await?;
        let req = ModelRequest {
            model: t.agent.config.model.id.clone(),
            agent: t.name().to_owned(),
            system: t.system.clone(),
            messages: messages.clone(),
            tools: t.toolbox.tools.clone(),
            max_tokens: self.max_tokens,
            effort: t.agent.config.model.effort.map(|e| format!("{:?}", e.level).to_lowercase()),
        };
        let response = match self.call(req).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(StepEnd::Stopped(TurnEnd::Interrupted)),
            Err(e) => {
                let end = SpanEvent::ModelRequestEnd {
                    model_request_start_id: start.id.clone(),
                    is_error: true,
                    model_usage: Default::default(),
                };
                self.emit(t, end).await?;
                let error = serde_json::json!({"type": "model_error", "message": e.to_string()});
                self.contract
                    .emit(&t.ws, &t.session, t.scope.as_ref(), SessionEvent::Error { error }.into())
                    .await?;
                return Ok(StepEnd::Stopped(TurnEnd::Failed(e.to_string())));
            }
        };
        if response.thought {
            self.emit(t, AgentEvent::Thinking { content: None }).await?;
        }
        let text: Vec<&str> = response
            .content
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let text = (!text.is_empty()).then(|| text.join("\n"));
        if let Some(text) = &text {
            self.emit(t, AgentEvent::Message { content: vec![ContentBlock::text(text.clone())] }).await?;
        }
        let u = &response.usage;
        let end = SpanEvent::ModelRequestEnd {
            model_request_start_id: start.id,
            is_error: false,
            model_usage: u.clone(),
        };
        self.emit(t, end).await?;
        let usage = Usage {
            cache_creation: None,
            cache_read_input_tokens: u.cache_read_input_tokens,
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        };
        self.contract.add_usage(&t.ws, &t.session, &usage).await?;
        self.act(t, messages, response, text).await
    }
}
