//! The scripted model: each agent plays its own queue of responses, so a
//! multi-agent session runs deterministically in tests and in CI.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::{ModelClient, ModelError, ModelRequest, ModelResponse};

/// The queue every agent without its own script plays.
const ANY_AGENT: &str = "*";
/// A response played, again and again, once every queue is spent.
const DEFAULT: &str = "default";

/// Plays scripted responses per agent and records every request.
#[derive(Debug, Default)]
pub struct MockModel {
    scripts: Mutex<HashMap<String, Vec<ModelResponse>>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl MockModel {
    /// One script, played in order to whichever agent asks.
    pub fn scripted(responses: Vec<ModelResponse>) -> Self {
        Self::per_agent(HashMap::from([(ANY_AGENT.to_owned(), responses)]))
    }

    /// A script per agent name; `*` serves agents without their own, and the
    /// one response under `default` (if any) answers after that.
    pub fn per_agent(scripts: HashMap<String, Vec<ModelResponse>>) -> Self {
        let scripts = scripts
            .into_iter()
            .map(|(agent, mut responses)| {
                responses.reverse();
                (agent, responses)
            })
            .collect();
        Self { scripts: Mutex::new(scripts), requests: Mutex::new(Vec::new()) }
    }

    /// Load per-agent scripts from a JSON file:
    /// `{"agent name": [ModelResponse, …], "*": […]}`.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let scripts = serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self::per_agent(scripts))
    }

    /// The requests the loop actually sent.
    pub fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().clone()
    }
}

#[async_trait]
impl ModelClient for MockModel {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        let agent = req.agent.clone();
        self.requests.lock().push(req);
        let mut scripts = self.scripts.lock();
        let own = scripts.get_mut(&agent).and_then(Vec::pop);
        own.or_else(|| scripts.get_mut(ANY_AGENT).and_then(Vec::pop))
            .or_else(|| scripts.get(DEFAULT).and_then(|d| d.first().cloned()))
            .ok_or_else(|| ModelError::Fatal(format!("the mock script for `{agent}` is exhausted")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(agent: &str) -> ModelRequest {
        ModelRequest {
            model: "claude-sonnet-4-6".into(),
            agent: agent.into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
            effort: None,
        }
    }

    #[tokio::test]
    async fn each_agent_plays_its_own_script_then_the_shared_one() {
        let mock = MockModel::per_agent(HashMap::from([
            ("grader".to_owned(), vec![ModelResponse::text("graded")]),
            ("*".to_owned(), vec![ModelResponse::text("one"), ModelResponse::text("two")]),
        ]));
        assert_eq!(mock.complete(request("worker")).await.unwrap(), ModelResponse::text("one"));
        assert_eq!(mock.complete(request("grader")).await.unwrap(), ModelResponse::text("graded"));
        assert_eq!(mock.complete(request("grader")).await.unwrap(), ModelResponse::text("two"));
        assert!(matches!(mock.complete(request("worker")).await, Err(ModelError::Fatal(_))));
        assert_eq!(mock.requests().len(), 4);
        let fallback =
            MockModel::per_agent(HashMap::from([("default".to_owned(), vec![ModelResponse::text("ok")])]));
        assert_eq!(fallback.complete(request("a")).await.unwrap(), ModelResponse::text("ok"));
        assert_eq!(fallback.complete(request("a")).await.unwrap(), ModelResponse::text("ok"));
    }
}
