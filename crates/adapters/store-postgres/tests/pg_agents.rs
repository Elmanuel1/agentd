//! Agents over live Postgres: the version race, list filters and order, and
//! update semantics. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::sync::Arc;

use agents::store::{AgentFilter, AgentsStore, StoreError};
use agents::types::{CreateAgentRequest, Multiagent, RosterMember, UpdateAgentRequest};
use agents::AgentsService;
use shared::limits::ModelCatalog;
use shared::{AuthContext, OrgId, Pagination, Timestamp, WorkspaceId};
use store_postgres::{PgAgentsStore, PgBackend};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn store() -> Arc<PgAgentsStore> {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    Arc::new(PgAgentsStore::new(PgBackend::connect(&url).await.unwrap()))
}

fn ctx(tag: &str) -> AuthContext {
    let stamp = Timestamp::now().as_datetime().timestamp_micros();
    AuthContext {
        org: OrgId::new(format!("org_{tag}{stamp}")),
        workspace: WorkspaceId::new(format!("wrkspc_{tag}{stamp}")),
        api_key_id: "apikey_agents".into(),
    }
}

fn create(name: &str) -> CreateAgentRequest {
    serde_json::from_value(serde_json::json!({"name": name, "model": "claude-sonnet-4-6"})).unwrap()
}

fn update(json: serde_json::Value) -> UpdateAgentRequest {
    serde_json::from_value(json).unwrap()
}

fn page() -> Pagination {
    Pagination::from_params(&HashMap::new()).unwrap()
}

#[tokio::test]
async fn a_second_writer_of_one_version_is_a_conflict_not_a_500() {
    if !enabled() {
        return;
    }
    let store = store().await;
    let svc = AgentsService::new(store.clone(), ModelCatalog::default_catalog());
    let ctx = ctx("race");
    let a = svc.create(&ctx, create("racer")).await.unwrap();
    let mut v2 = a.clone();
    v2.version = 2;
    store.append_version(&ctx.workspace, v2.clone()).await.unwrap();
    assert_eq!(store.append_version(&ctx.workspace, v2).await, Err(StoreError::Conflict));
}

#[tokio::test]
async fn the_list_hides_archived_agents_and_is_newest_first() {
    if !enabled() {
        return;
    }
    let svc = AgentsService::new(store().await, ModelCatalog::default_catalog());
    let ctx = ctx("list");
    let first = svc.create(&ctx, create("first")).await.unwrap();
    let second = svc.create(&ctx, create("second")).await.unwrap();
    let archived = svc.create(&ctx, create("archived")).await.unwrap();
    svc.archive(&ctx, archived.id.as_str()).await.unwrap();

    let ids = |list: Vec<agents::Agent>| list.into_iter().map(|a| a.id).collect::<Vec<_>>();
    let active = ids(svc.list(&ctx, &AgentFilter::default(), &page()).await.unwrap());
    assert_eq!(active, vec![second.id.clone(), first.id.clone()]);
    let all = ids(svc
        .list(&ctx, &AgentFilter { include_archived: true, ..Default::default() }, &page())
        .await
        .unwrap());
    assert_eq!(all, vec![archived.id, second.id.clone(), first.id]);
    let mut since_second = AgentFilter::default();
    since_second.created.gte = Some(second.created_at);
    assert_eq!(ids(svc.list(&ctx, &since_second, &page()).await.unwrap()), vec![second.id]);
}

#[tokio::test]
async fn an_update_needs_no_version_and_a_no_op_mints_none() {
    if !enabled() {
        return;
    }
    let svc = AgentsService::new(store().await, ModelCatalog::default_catalog());
    let ctx = ctx("upd");
    let a = svc.create(&ctx, create("u")).await.unwrap();
    let same = svc.update(&ctx, a.id.as_str(), update(serde_json::json!({"name": "u"}))).await.unwrap();
    assert_eq!(same.version, 1);
    let next = svc
        .update(&ctx, a.id.as_str(), update(serde_json::json!({"description": "d", "metadata": {"k": "v"}})))
        .await
        .unwrap();
    assert_eq!((next.version, next.config.description.as_deref()), (2, Some("d")));
    let cleared = svc
        .update(
            &ctx,
            a.id.as_str(),
            update(serde_json::json!({"description": null, "metadata": {"k": null}})),
        )
        .await
        .unwrap();
    assert_eq!((cleared.version, cleared.config.description, cleared.metadata.is_empty()), (3, None, true));
    let stale = svc.update(&ctx, a.id.as_str(), update(serde_json::json!({"version": 1, "name": "x"}))).await;
    assert!(stale.unwrap_err().message.starts_with("Concurrent modification detected"));
}

#[tokio::test]
async fn multiagent_references_resolve_to_concrete_versions() {
    if !enabled() {
        return;
    }
    let svc = AgentsService::new(store().await, ModelCatalog::default_catalog());
    let ctx = ctx("multi");
    let worker = svc.create(&ctx, create("worker")).await.unwrap();
    let req: CreateAgentRequest = serde_json::from_value(serde_json::json!({
        "name": "lead", "model": "claude-sonnet-4-6",
        "multiagent": {"type": "coordinator", "agents": [
            worker.id.as_str(), {"type": "self"}, {"type": "advisor", "model": "claude-opus-4-6"}]}
    }))
    .unwrap();
    let lead = svc.create(&ctx, req).await.unwrap();
    let Some(Multiagent::Coordinator { agents }) = &lead.config.multiagent else { panic!("coordinator") };
    assert!(matches!(&agents[0], RosterMember::Agent(r) if r.id == worker.id && r.version == 1));
    assert!(matches!(&agents[1], RosterMember::Agent(r) if r.id == lead.id && r.version == 1));
    assert!(matches!(&agents[2], RosterMember::Advisor(a) if a.model == "claude-opus-4-6"));

    let v2: UpdateAgentRequest = update(serde_json::json!({"multiagent": {"type": "multiagent_20261001",
        "subagents": {"type": "enabled", "inline_agents": {"type": "disabled"}, "predefined_agents": [worker.id.as_str()]}}}));
    let lead = svc.update(&ctx, lead.id.as_str(), v2).await.unwrap();
    let v = serde_json::to_value(&lead.config.multiagent).unwrap();
    assert_eq!(v["advisor"], serde_json::json!({"type": "disabled"}));
    assert_eq!(
        v["workflows"],
        serde_json::json!({"type": "enabled", "inline_agents": {"type": "enabled"}, "predefined_agents": []})
    );
    assert_eq!(v["subagents"]["predefined_agents"][0]["version"], 1);
    // A member left out keeps its stored value.
    let v3 = update(
        serde_json::json!({"multiagent": {"type": "multiagent_20261001", "workflows": {"type": "disabled"}}}),
    );
    let lead = svc.update(&ctx, lead.id.as_str(), v3).await.unwrap();
    let v = serde_json::to_value(&lead.config.multiagent).unwrap();
    assert_eq!(v["subagents"]["inline_agents"], serde_json::json!({"type": "disabled"}));
    assert_eq!(v["workflows"], serde_json::json!({"type": "disabled"}));
}
