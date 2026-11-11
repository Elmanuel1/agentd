//! Assembling a conversation step by step: each step's assistant message,
//! then the user-side inputs that followed it, tool results first.

use std::collections::HashSet;

use super::{History, Unanswered};
use crate::model::{Block, Message, Role};

#[derive(Default)]
pub(super) struct Builder {
    pub(super) messages: Vec<Message>,
    pub(super) assistant: Vec<Block>,
    pub(super) results: Vec<Block>,
    pub(super) texts: Vec<Block>,
    pub(super) calls: Vec<(String, String, serde_json::Value)>,
    pub(super) answered: HashSet<String>,
    pub(super) owed: HashSet<String>,
    pub(super) custom: HashSet<String>,
}

impl Builder {
    pub(super) fn call(&mut self, id: String, name: &str, input: &serde_json::Value) {
        self.assistant.push(Block::ToolUse { id: id.clone(), name: name.to_owned(), input: input.clone() });
        self.calls.push((id, name.to_owned(), input.clone()));
    }

    pub(super) fn result(&mut self, id: String, content: String, is_error: bool) {
        if self.answered.insert(id.clone()) {
            self.results.push(Block::ToolResult { tool_use_id: id, content, is_error });
        }
    }

    pub(super) fn text(&mut self, text: String) {
        self.texts.push(Block::Text { text });
    }

    /// Close the current step: its assistant message, then the user-side
    /// inputs that followed it.
    pub(super) fn step(&mut self) {
        let unanswered: Vec<String> = self
            .calls
            .iter()
            .filter(|(id, ..)| !self.answered.contains(id))
            .map(|(id, ..)| id.clone())
            .collect();
        for id in unanswered {
            self.result(id, "The tool did not run: the turn was interrupted.".into(), true);
        }
        self.flush();
    }

    fn flush(&mut self) {
        if !self.assistant.is_empty() {
            self.messages
                .push(Message { role: Role::Assistant, content: std::mem::take(&mut self.assistant) });
        }
        let mut user = std::mem::take(&mut self.results);
        user.append(&mut self.texts);
        if !user.is_empty() {
            self.messages.push(Message { role: Role::User, content: user });
        }
        self.calls.clear();
    }

    /// The conversation so far. A call of the last step with no result yet
    /// is waiting: on the client (a confirmation or a custom tool) or on the
    /// session's worker (a built-in tool), so the model is not called.
    pub(super) fn finish(mut self) -> History {
        let unanswered = self
            .calls
            .iter()
            .filter(|(id, ..)| !self.answered.contains(id))
            .map(|(id, name, input)| Unanswered {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                custom: self.custom.contains(id),
                asked: self.owed.contains(id),
            })
            .collect();
        self.flush();
        History { messages: self.messages, unanswered }
    }
}
