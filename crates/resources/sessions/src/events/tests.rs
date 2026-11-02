use super::*;

#[test]
fn status_idle_wire_shape_matches_documented_example() {
    // Documented: {"id":"sevt_456","processed_at":"…","stop_reason":
    //   {"event_ids":["sevt_123"],"type":"requires_action"},"type":"session.status_idle"}
    let blocking = EventId::generate();
    let ev = Event {
        id: EventId::generate(),
        processed_at: Some(Timestamp::now()),
        payload: EventPayload::Session(SessionEvent::StatusIdle {
            stop_reason: StopReason::RequiresAction { event_ids: vec![blocking.clone()] },
        }),
    };
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["type"], "session.status_idle");
    assert_eq!(v["stop_reason"]["type"], "requires_action");
    assert_eq!(v["stop_reason"]["event_ids"][0], blocking.as_str());
    assert!(v["id"].as_str().unwrap().starts_with("sevt_01"));
}

#[test]
fn model_request_end_wire_shape() {
    let start = EventId::generate();
    let ev = Event::new(EventPayload::Span(SpanEvent::ModelRequestEnd {
        model_request_start_id: start.clone(),
        is_error: false,
        model_usage: ModelUsage {
            cache_read_input_tokens: 6656,
            input_tokens: 3571,
            output_tokens: 727,
            cache_creation_input_tokens: 0,
        },
    }));
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["type"], "span.model_request_end");
    assert_eq!(v["model_usage"]["input_tokens"], 3571);
    assert_eq!(v["model_request_start_id"], start.as_str());
    assert_eq!(v["processed_at"], serde_json::Value::Null);
}

#[test]
fn user_message_roundtrip() {
    // Live omits processed_at on a queued event (added once processed).
    let raw = serde_json::json!({
        "id": "sevt_01AbCdEfGhIjKlMnOpQrStUv",
        "type": "user.message",
        "content": [{"type":"text","text":"Summarize the repo README"}]
    });
    let ev: Event = serde_json::from_value(raw.clone()).unwrap();
    assert!(ev.payload.is_user_event());
    assert_eq!(serde_json::to_value(&ev).unwrap(), raw);
}

#[test]
fn define_outcome_roundtrip_with_rubric_forms() {
    for rubric in [
        serde_json::json!({"type":"text","content":"CSV has a numeric price column"}),
        serde_json::json!({"type":"file","file_id":"file_01AbCdEfGhIjKlMnOpQrStUv"}),
    ] {
        let raw = serde_json::json!({
            "id": "sevt_01AbCdEfGhIjKlMnOpQrStUv",
            "type": "user.define_outcome",
            "description": "Build the model",
            "rubric": rubric,
            "max_iterations": 5
        });
        let ev: Event = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&ev).unwrap(), raw);
    }
}

#[test]
fn stop_reason_terminality_gates_the_drain_loop() {
    assert!(StopReason::EndTurn {}.is_terminal());
    assert!(StopReason::RetriesExhausted {}.is_terminal());
    assert!(!StopReason::RequiresAction { event_ids: vec![] }.is_terminal());
}

#[test]
fn all_event_types_serialize_with_dot_names() {
    let cases: Vec<(EventPayload, &str)> = vec![
        (EventPayload::User(UserEvent::Interrupt {}), "user.interrupt"),
        (EventPayload::Agent(AgentEvent::ThreadContextCompacted {}), "agent.thread_context_compacted"),
        (EventPayload::Session(SessionEvent::StatusRunning {}), "session.status_running"),
        (EventPayload::Session(SessionEvent::StatusTerminated {}), "session.status_terminated"),
        (EventPayload::Span(SpanEvent::ModelRequestStart {}), "span.model_request_start"),
    ];
    for (p, expected) in cases {
        let v = serde_json::to_value(Event::new(p)).unwrap();
        assert_eq!(v["type"], expected);
    }
}

#[test]
fn a_category_keeps_serdes_own_messages() {
    let err = serde_json::from_str::<EventPayload>(r#"{"type":"user.message"}"#).unwrap_err();
    assert!(err.to_string().contains("missing field `content`"), "{err}");
    let err = serde_json::from_str::<EventPayload>(r#"{"type":"user.bogus"}"#).unwrap_err();
    assert!(err.to_string().contains("unknown variant `user.bogus`"), "{err}");
    let err = serde_json::from_str::<EventPayload>(r#"{"content":[]}"#).unwrap_err();
    assert!(err.to_string().contains("missing field `type`"), "{err}");
}

#[test]
fn every_category_round_trips_through_its_type_prefix() {
    for raw in [
        r#"{"type":"system.message","content":[{"type":"text","text":"x"}]}"#,
        r#"{"type":"agent.thread_context_compacted"}"#,
        r#"{"type":"session.status_running"}"#,
        r#"{"type":"span.model_request_start"}"#,
    ] {
        let payload: EventPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(
            serde_json::to_value(&payload).unwrap(),
            serde_json::from_str::<serde_json::Value>(raw).unwrap()
        );
    }
    let system: EventPayload = serde_json::from_str(r#"{"type":"system.message","content":[]}"#).unwrap();
    assert!(system.is_user_event());
}
