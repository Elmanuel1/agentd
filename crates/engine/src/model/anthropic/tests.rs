use super::*;
use crate::model::ModelTool;

#[test]
fn tool_calls_and_results_use_the_messages_api_blocks() {
    let req = ModelRequest {
        model: "claude-haiku-4-5".into(),
        agent: "a".into(),
        system: "be brief".into(),
        messages: vec![
            Message { role: Role::User, content: vec![Block::text("list files")] },
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "a\nb".into(),
                    is_error: false,
                }],
            },
        ],
        tools: vec![ModelTool {
            name: "bash".into(),
            description: "run".into(),
            input_schema: json!({"type": "object"}),
        }],
        max_tokens: 100,
        effort: Some("low".into()),
        compact: None,
    };
    let b = body(&req);
    assert_eq!(b["system"][0]["text"], "be brief");
    assert_eq!(b["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(b["messages"][2]["content"][0]["tool_use_id"], "t1");
    assert_eq!(b["messages"][2]["content"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(b["tools"][0]["name"], "bash");
    assert!(b.get("output_config").is_none());
}

#[test]
fn a_response_maps_text_tool_use_stop_and_usage() {
    let r = parse(&json!({
        "content": [{"type": "thinking", "thinking": "", "signature": "sig"}, {"type": "text", "text": "hi"},
                    {"type": "tool_use", "id": "t", "name": "read", "input": {"file_path": "/a"}}],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 3, "output_tokens": 4, "cache_read_input_tokens": 5, "cache_creation_input_tokens": 6},
    }));
    assert!(r.thought);
    assert_eq!(r.stop, StopKind::ToolUse);
    assert_eq!(r.content.len(), 3);
    assert_eq!(r.content[0], Block::Thinking { thinking: String::new(), signature: "sig".into() });
    assert_eq!(r.usage.cache_creation_input_tokens, 6);
}

#[test]
fn an_empty_system_prompt_sends_no_system_block() {
    // The API refuses an empty system text block that carries a cache mark.
    let req = ModelRequest {
        model: "claude-haiku-4-5".into(),
        agent: "a".into(),
        system: String::new(),
        messages: vec![Message { role: Role::User, content: vec![Block::text("hi")] }],
        tools: Vec::new(),
        max_tokens: 10,
        effort: None,
        compact: None,
    };
    assert!(body(&req).get("system").is_none());
}

#[test]
fn thinking_is_adaptive_except_on_the_models_that_take_a_budget_and_goes_back_unmodified() {
    let req = |model: &str, max_tokens: u32| ModelRequest {
        model: model.into(),
        agent: "a".into(),
        system: String::new(),
        messages: vec![
            Message { role: Role::User, content: vec![Block::text("hi")] },
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::Thinking { thinking: String::new(), signature: "sig".into() },
                    Block::RedactedThinking { data: "enc".into() },
                ],
            },
        ],
        tools: Vec::new(),
        max_tokens,
        effort: None,
        compact: None,
    };
    let b = body(&req("claude-sonnet-4-6", 16000));
    assert_eq!(b["thinking"], json!({"type": "adaptive"}));
    assert_eq!(
        b["messages"][1]["content"][0],
        json!({"type": "thinking", "thinking": "", "signature": "sig"})
    );
    assert_eq!(b["messages"][1]["content"][1]["type"], "redacted_thinking");
    assert_eq!(
        body(&req("claude-haiku-4-5", 16000))["thinking"],
        json!({"type": "enabled", "budget_tokens": 4000})
    );
    assert!(body(&req("claude-haiku-4-5", 1000)).get("thinking").is_none(), "no room under the cap");
}

#[test]
fn a_compaction_asks_with_its_instructions_and_its_block_goes_back_as_it_came() {
    // C1: the request's `compaction`, the reply's block and its iterations' usage.
    let block = Block::Compaction { content: "so far".into(), signature: "sig".into() };
    let req = ModelRequest {
        model: "claude-haiku-4-5".into(),
        agent: "a".into(),
        system: String::new(),
        messages: vec![
            Message { role: Role::Assistant, content: vec![block.clone()] },
            Message { role: Role::User, content: vec![Block::text("go on")] },
        ],
        tools: Vec::new(),
        max_tokens: 4096,
        effort: None,
        compact: Some("keep the goal".into()),
    };
    assert!(compacts(&req));
    let b = body(&req);
    assert_eq!(b["compaction"], json!({"type": "summarize", "instructions": "keep the goal"}));
    assert_eq!(
        b["messages"][0]["content"][0],
        json!({"type": "compaction", "content": "so far", "signature": "sig"})
    );
    let plain = ModelRequest { messages: vec![req.messages[1].clone()], compact: None, ..req };
    assert!(!compacts(&plain), "a request with no block sends no beta");
    let reply = parse(&json!({
        "content": [{"type": "compaction", "content": "new summary", "signature": "s2"}],
        "stop_reason": "compaction",
        "usage": {"input_tokens": 0, "output_tokens": 0,
                  "iterations": [{"type": "compaction", "input_tokens": 144, "output_tokens": 276}]},
    }));
    assert_eq!(reply.stop, StopKind::Compaction);
    assert_eq!(reply.content, [Block::Compaction { content: "new summary".into(), signature: "s2".into() }]);
    assert_eq!((reply.usage.input_tokens, reply.usage.output_tokens), (144, 276));
}
