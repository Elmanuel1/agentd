//! Thinking blocks the loop kept go back as they came.

use super::*;

#[test]
fn a_call_s_kept_thinking_goes_back_first_in_its_message() {
    let (s, thought) = (start(), ev(AgentEvent::Thinking { content: None }));
    let call = tool_use("bash", "allow");
    let events = [user("go"), s.clone(), thought.clone(), call.clone(), end(&s), result(&call, "ok")];
    let blocks = json!({"thinking": [{"type": "thinking", "thinking": "", "signature": "sig"}]});
    let kept: Kept = HashMap::from([(thought.id.clone(), blocks)]);
    let h = rebuild(&events.iter().collect::<Vec<_>>(), &HashMap::new(), &kept);
    let said = h.messages.iter().find(|m| m.role == Role::Assistant).unwrap();
    assert_eq!(said.content[0], Block::Thinking { thinking: String::new(), signature: "sig".into() });
    assert!(matches!(said.content[1], Block::ToolUse { .. }));
    assert_eq!(build(&events).messages.iter().find(|m| m.role == Role::Assistant).unwrap().content.len(), 1);
}
