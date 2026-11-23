//! Threads that wait while others work.

use axum::http::StatusCode;
use serde_json::json;

use super::common::{self, Auth, KEY, Rig, request, send_to};
use super::{Say, Scripted, events_until, message, on_loop, send, types};

#[tokio::test]
async fn a_coordinator_waiting_on_a_tool_lets_its_delegate_run() {
    // L-15: the coordinator's call waits for the client while its helper
    // works; only the coordinator's thread idles, so the helper runs and
    // replies before the call is answered, and the session idles once, after.
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let name = format!("helper-{}", common::stamp());
    let model = Scripted::new(vec![
        // One reply starts the helper and waits on the client; the helper then
        // takes the next step while the coordinator waits.
        Say::Calls(vec![
            ("create_agent", json!({"agent_name": name, "task": "do it"})),
            ("lookup", json!({"k": 1})),
        ]),
        Say::Text("done by the helper"),
        Say::Text("all done"),
    ]);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let helper = json!({"name": name, "model": "claude-haiku-4-5"});
    let (_, helper) = send_to(&app, request("POST", "/v1/agents", Auth::Key(KEY), Some(helper))).await;
    let coord = json!({
        "name": format!("coord-{}", common::stamp()), "model": "claude-haiku-4-5",
        "tools": [{"type": "custom", "name": "lookup", "description": "Look a value up.", "input_schema": {"type": "object"}}],
        "multiagent": {"type": "coordinator", "agents": [{"type": "agent", "id": helper["id"], "version": helper["version"]}]},
    });
    let (st, coord) = send_to(&app, request("POST", "/v1/agents", Auth::Key(KEY), Some(coord))).await;
    assert_eq!(st, StatusCode::OK, "{coord}");
    let body = json!({"agent": coord["id"], "environment_id": env});
    let (_, created) = send_to(&app, request("POST", "/v1/sessions", Auth::Key(KEY), Some(body))).await;
    let s = created["id"].as_str().unwrap().to_owned();
    send(&app, &s, message("run it")).await;
    let events = events_until(&app, &s, "agent.thread_message_received", 1).await;
    let t = types(&events);
    let replied = t.iter().rposition(|k| *k == "agent.thread_message_received").unwrap();
    let idled = t.iter().position(|k| *k == "session.status_idle");
    assert!(idled.is_none_or(|i| i > replied), "the session runs until the helper replies: {t:?}");
    let custom = events.iter().find(|e| e["type"] == "agent.custom_tool_use").unwrap().clone();
    // With the helper done, the session idles on the coordinator's open call.
    let waited = events_until(&app, &s, "session.status_idle", 1).await;
    let idle = waited.iter().find(|e| e["type"] == "session.status_idle").unwrap();
    assert_eq!(idle["stop_reason"], json!({"type": "requires_action", "event_ids": [custom["id"]]}));
    let answer = json!({"events": [{"type": "user.custom_tool_result", "custom_tool_use_id": custom["id"], "content": [{"type": "text", "text": "42"}]}]});
    let (st, body) = send(&app, &s, answer).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let events = events_until(&app, &s, "session.status_idle", 2).await;
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
    assert_eq!(model.asked.lock().len(), 3);
}

#[tokio::test]
async fn an_agent_s_skill_versions_are_read_in_one_call_in_its_order() {
    // Each ref reads its version (the newest for `latest`, by number or by
    // id); one that names no version is left out.
    let Some(rig) = Rig::new().await else { return };
    let name = format!("listed-{}", common::stamp());
    let md = format!("---\nname: {name}\ndescription: Reads.\n---\n# Listed\n");
    let files = || vec![(format!("{name}/SKILL.md"), bytes::Bytes::from(md.clone()))];
    let skills = skills::SkillsService::new(rig.stores.skills.clone(), rig.stores.skill_versions.clone());
    let ctx = rig.ctx();
    let skill = skills.create(&ctx, files(), None).await.unwrap();
    let id = skill.id.as_str();
    let first = skills.version(&ctx, id, "latest").await.unwrap();
    let second = skills.create_version(&ctx, id, files()).await.unwrap();
    let missing = skills::SkillId::generate();
    let refs = [
        (id, "latest"),
        (id, first.version.as_str()),
        (missing.as_str(), "latest"),
        (id, first.id.as_str()),
        (id, second.id.as_str()),
    ];
    let got = skills.listed(&ctx.workspace, &refs).await.unwrap();
    let ids: Vec<&str> = got.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, [second.id.as_str(), first.id.as_str(), first.id.as_str(), second.id.as_str()]);
}

#[tokio::test]
async fn an_agent_s_skills_are_listed_in_its_prompt() {
    // Each skill the agent names, with its frontmatter description and where
    // its SKILL.md is in the sandbox.
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let name = format!("citing-{}", common::stamp());
    let md = format!("---\nname: {name}\ndescription: Resolves snippets to pages.\n---\n# Citing\n");
    let skills = skills::SkillsService::new(rig.stores.skills.clone(), rig.stores.skill_versions.clone());
    let files = vec![(format!("{name}/SKILL.md"), bytes::Bytes::from(md))];
    let skill = skills.create(&rig.ctx(), files, None).await.unwrap();
    let model = Scripted::new(vec![Say::Text("ok")]);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let agent = json!({"name": format!("a-{}", common::stamp()), "model": "claude-haiku-4-5",
        "skills": [{"type": "custom", "skill_id": skill.id, "version": "latest"}]});
    let (st, agent) = send_to(&app, request("POST", "/v1/agents", Auth::Key(KEY), Some(agent))).await;
    assert_eq!(st, StatusCode::OK, "{agent}");
    let body = json!({"agent": agent["id"], "environment_id": env});
    let (_, created) = send_to(&app, request("POST", "/v1/sessions", Auth::Key(KEY), Some(body))).await;
    let s = created["id"].as_str().unwrap().to_owned();
    send(&app, &s, message("hi")).await;
    events_until(&app, &s, "session.status_idle", 1).await;
    let system = model.asked.lock()[0].system.clone();
    assert!(
        system.contains(&format!(
            "- `{name}` (/workspace/skills/{name}/SKILL.md): Resolves snippets to pages."
        )),
        "{system}"
    );
}
