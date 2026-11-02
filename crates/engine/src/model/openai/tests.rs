use super::*;

#[test]
fn tool_results_become_tool_messages_and_text_follows_them() {
    let req = ModelRequest {
        model: "openai/gpt-6-luna".into(),
        agent: "a".into(),
        system: "sys".into(),
        messages: vec![
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![
                    Block::ToolResult { tool_use_id: "c1".into(), content: "boom".into(), is_error: true },
                    Block::text("and also"),
                ],
            },
        ],
        tools: vec![],
        max_tokens: 10,
        effort: Some("max".into()),
    };
    let b = body(&req);
    let m = b["messages"].as_array().unwrap();
    assert_eq!(m[0]["role"], "system");
    assert_eq!(m[1]["tool_calls"][0]["function"]["arguments"], "{\"command\":\"ls\"}");
    assert_eq!(m[2]["role"], "tool");
    assert_eq!(m[2]["content"], "Error: boom");
    assert_eq!(m[3]["role"], "user");
    assert_eq!(b["reasoning"]["effort"], "high");
}

#[test]
fn a_response_maps_calls_and_cached_usage() {
    let r = parse(&json!({
        "choices": [{"finish_reason": "tool_calls", "message": {"content": null, "tool_calls": [
            {"id": "c", "type": "function", "function": {"name": "read", "arguments": "{\"file_path\":\"/x\"}"}}]}}],
        "usage": {"prompt_tokens": 100, "completion_tokens": 7, "prompt_tokens_details": {"cached_tokens": 60},
                  "completion_tokens_details": {"reasoning_tokens": 3}},
    }))
    .unwrap();
    assert_eq!(r.stop, StopKind::ToolUse);
    assert_eq!(r.usage.input_tokens, 40);
    assert_eq!(r.usage.cache_read_input_tokens, 60);
    assert!(r.thought);
    assert_eq!(r.tool_uses().next().unwrap().2["file_path"], "/x");
}
