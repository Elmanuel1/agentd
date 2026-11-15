//! Compaction end to end: a scripted model whose window is small and whose
//! turns report most of it, so a thread compacts within a few turns (CP-4 to
//! CP-10).

use serde_json::{json, Value};

use super::common::Rig;
use super::{events_until, message, on_loop, send, session, Block, Say, Scripted};

/// A model with a 2,000-token window whose turns report 1,700 tokens.
fn crowded(script: Vec<Say>) -> std::sync::Arc<Scripted> {
    let model = Scripted::new(script);
    *model.window.lock() = 2_000;
    *model.input.lock() = 1_700;
    model
}

fn compactions(model: &Scripted) -> Vec<usize> {
    model.asked.lock().iter().enumerate().filter(|(_, r)| r.compact.is_some()).map(|(i, _)| i).collect()
}

#[tokio::test]
async fn a_crowded_thread_compacts_and_its_next_call_starts_with_the_block() {
    // CP-4, CP-6, CP-7, CP-9.
    let Some(rig) = Rig::new().await else { return };
    let model = crowded(vec![Say::Text("one"), Say::Text("two"), Say::Text("three")]);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    for (i, text) in ["the code word is heron", "second", "third"].into_iter().enumerate() {
        send(&app, &s, message(text)).await;
        events_until(&app, &s, "session.status_idle", i + 1).await;
    }
    let events = events_until(&app, &s, "session.status_idle", 3).await;
    let compacted: Vec<&Value> =
        events.iter().filter(|e| e["type"] == "agent.thread_context_compacted").collect();
    assert_eq!(compacted.len(), 1);
    let mut fields: Vec<&str> = compacted[0].as_object().unwrap().keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(fields, ["id", "processed_at", "type"], "live's fields only");
    let at = compactions(&model);
    assert_eq!(at.len(), 1, "one compaction");
    let asked = model.asked.lock();
    let (summary, next) = (&asked[at[0]], &asked[at[0] + 1]);
    assert!(
        summary.compact.as_deref().is_some_and(|i| i.contains("the code word is heron")),
        "the user's words, verbatim"
    );
    assert_eq!(summary.tools, next.tools, "the same tools, so the cache holds");
    assert_eq!(summary.system, next.system);
    assert_eq!(
        next.messages[0].content,
        [Block::Compaction { content: "the summary".into(), signature: "sig".into() }]
    );
    let last = next.messages.last().unwrap();
    assert!(matches!(&last.content[..], [Block::Text { text }] if text == "third"));
}

#[tokio::test]
async fn a_second_compaction_starts_from_the_first_checkpoint() {
    // CP-6: never the whole log.
    let Some(rig) = Rig::new().await else { return };
    let model = crowded(vec![]);
    model.summaries.lock().extend([Some("first"), Some("second"), Some("third")]);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    for i in 0..5 {
        send(&app, &s, message(&format!("message {i}"))).await;
        events_until(&app, &s, "session.status_idle", i + 1).await;
    }
    let at = compactions(&model);
    assert_eq!(at.len(), 3, "each crowded turn from the third compacts first: {at:?}");
    let asked = model.asked.lock();
    let block = |s: &str| vec![Block::Compaction { content: s.into(), signature: "sig".into() }];
    assert_eq!(
        asked[at[1]].messages[0].content,
        block("first"),
        "the second summarises the first and what followed"
    );
    assert_eq!(asked[at[2]].messages[0].content, block("second"));
    assert_eq!(asked.last().unwrap().messages[0].content, block("third"), "only the newest block is sent");
}

#[tokio::test]
async fn a_compaction_with_no_summary_lets_the_turn_run_and_tries_again_later() {
    // CP-10.
    let Some(rig) = Rig::new().await else { return };
    let model = crowded(vec![]);
    model.summaries.lock().push_back(None);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    for i in 0..4 {
        send(&app, &s, message(&format!("message {i}"))).await;
        events_until(&app, &s, "session.status_idle", i + 1).await;
    }
    let events = events_until(&app, &s, "session.status_idle", 4).await;
    let at = compactions(&model);
    assert_eq!(at.len(), 2, "the empty one, then the next turn's: {at:?}");
    let asked = model.asked.lock();
    assert!(asked[at[0] + 1].compact.is_none(), "the turn runs after the empty compaction");
    let compacted = events.iter().filter(|e| e["type"] == "agent.thread_context_compacted").count();
    assert_eq!(compacted, 1);
}

#[tokio::test]
async fn an_oversized_compaction_request_leaves_out_its_oldest_calls() {
    // CP-11: the summarised part is trimmed before the call until it fits.
    let Some(rig) = Rig::new().await else { return };
    let model = Scripted::new(vec![]);
    *model.window.lock() = 2_000;
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    let long = "w".repeat(8_000);
    for (i, text) in [long.as_str(), "one", "two", "three", "four"].into_iter().enumerate() {
        if i == 3 {
            *model.input.lock() = 1_700;
        }
        send(&app, &s, message(text)).await;
        events_until(&app, &s, "session.status_idle", i + 1).await;
    }
    let at = compactions(&model);
    assert_eq!(at.len(), 1, "{at:?}");
    let fed = serde_json::to_string(&model.asked.lock()[at[0]].messages).unwrap();
    assert!(!fed.contains(&long), "the oldest call is left out of the request");
    assert!(fed.contains("one"), "the later calls are summarised");
}

#[tokio::test]
async fn a_checkpoint_written_by_another_model_compacts_again() {
    // CP-12.
    let Some(rig) = Rig::new().await else { return };
    let model = crowded(vec![]);
    model.summaries.lock().extend([Some("first"), Some("again")]);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    for i in 0..3 {
        send(&app, &s, message(&format!("message {i}"))).await;
        events_until(&app, &s, "session.status_idle", i + 1).await;
    }
    assert_eq!(compactions(&model).len(), 1);
    *model.input.lock() = 10;
    let pg = super::db().await;
    let mut conn = pg.diesel.get().await.unwrap();
    let q = diesel::sql_query(
        "UPDATE session_events SET doc = jsonb_set(doc, '{engine,checkpoint,model}', '\"another-model\"') \
         WHERE session_id = $1 AND doc->>'type' = 'agent.thread_context_compacted'",
    )
    .bind::<diesel::sql_types::Text, _>(&s);
    diesel_async::RunQueryDsl::execute(q, &mut *conn).await.unwrap();
    send(&app, &s, message("message 3")).await;
    events_until(&app, &s, "session.status_idle", 4).await;
    let at = compactions(&model);
    assert_eq!(at.len(), 2, "the uncrowded thread compacts again for its own model: {at:?}");
}
