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
        "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "hi"},
                    {"type": "tool_use", "id": "t", "name": "read", "input": {"file_path": "/a"}}],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 3, "output_tokens": 4, "cache_read_input_tokens": 5, "cache_creation_input_tokens": 6},
    }));
    assert!(r.thought);
    assert_eq!(r.stop, StopKind::ToolUse);
    assert_eq!(r.content.len(), 2);
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
    };
    assert!(body(&req).get("system").is_none());
}
