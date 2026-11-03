use async_trait::async_trait;
use parking_lot::Mutex;
use shared::OrgId;

use super::*;
use crate::workspaces::types::{AllowedGeos, InferenceGeo};

fn update(json: serde_json::Value) -> UpdateWorkspaceRequest {
    serde_json::from_value(json).unwrap()
}

/// Workspaces held in memory, by org.
#[derive(Default)]
struct MemoryWorkspaces(Mutex<Vec<(OrgId, Workspace)>>);

#[async_trait]
impl WorkspacesStore for MemoryWorkspaces {
    async fn insert(&self, org: &OrgId, workspace: &Workspace) -> Result<(), StoreError> {
        self.0.lock().push((org.clone(), workspace.clone()));
        Ok(())
    }

    async fn get(&self, org: &OrgId, id: &str) -> Result<Workspace, StoreError> {
        let rows = self.0.lock();
        rows.iter().find(|(o, w)| o == org && w.id == id).map(|(_, w)| w.clone()).ok_or(StoreError::NotFound)
    }

    async fn replace(&self, org: &OrgId, workspace: &Workspace) -> Result<(), StoreError> {
        let mut rows = self.0.lock();
        let row =
            rows.iter_mut().find(|(o, w)| o == org && w.id == workspace.id).ok_or(StoreError::NotFound)?;
        row.1 = workspace.clone();
        Ok(())
    }

    async fn list(
        &self,
        org: &OrgId,
        include_archived: bool,
        _cursor: &IdCursor,
    ) -> Result<(Vec<Workspace>, bool), StoreError> {
        let rows = self.0.lock();
        let mine = rows.iter().filter(|(o, w)| o == org && (include_archived || w.archived_at.is_none()));
        Ok((mine.map(|(_, w)| w.clone()).collect(), false))
    }
}

/// The workspaces whose keys were revoked, in order.
#[derive(Default)]
struct RevokedKeys(Mutex<Vec<String>>);

#[async_trait]
impl WorkspaceKeys for RevokedKeys {
    async fn revoke_all(&self, _org: &OrgId, workspace: &str, _at: Timestamp) -> Result<(), ApiError> {
        self.0.lock().push(workspace.to_owned());
        Ok(())
    }
}

fn admin(org: &str) -> AdminContext {
    AdminContext { org: OrgId::new(org), admin_key_id: "adminkey_t".into() }
}

fn service() -> WorkspacesService {
    revoking().0
}

fn revoking() -> (WorkspacesService, Arc<RevokedKeys>) {
    let keys = Arc::new(RevokedKeys::default());
    (WorkspacesService::new(Arc::new(MemoryWorkspaces::default()), keys.clone()), keys)
}

fn create(json: serde_json::Value) -> CreateWorkspaceRequest {
    serde_json::from_value(json).unwrap()
}

#[tokio::test]
async fn a_workspace_gets_the_documented_defaults() {
    let ws =
        service().create(&admin("org_a"), create(serde_json::json!({"name": "Production"}))).await.unwrap();
    let wire = serde_json::to_value(&ws).unwrap();
    assert!(ws.id.starts_with("wrkspc_01"));
    assert_eq!(wire["type"], "workspace");
    assert_eq!(wire["archived_at"], serde_json::Value::Null);
    assert_eq!(wire["external_key_id"], serde_json::Value::Null);
    assert_eq!(wire["tags"], serde_json::json!({}));
    assert_eq!(
        wire["data_residency"],
        serde_json::json!({
            "allowed_inference_geos": "unrestricted", "default_inference_geo": "global", "workspace_geo": "us"
        })
    );
    assert_eq!(ws.display_color, DEFAULT_COLOR);
}

#[tokio::test]
async fn a_workspace_is_seen_only_by_its_organization() {
    let svc = service();
    let ws = svc.create(&admin("org_a"), create(serde_json::json!({"name": "a"}))).await.unwrap();
    assert_eq!(svc.get(&admin("org_a"), &ws.id).await.unwrap(), ws);
    let err = svc.get(&admin("org_b"), &ws.id).await.unwrap_err();
    assert_eq!(err.status, 404);
    assert!(svc
        .list(&admin("org_b"), true, &IdCursor::from_params(&Default::default()).unwrap())
        .await
        .unwrap()
        .0
        .is_empty());
}

#[tokio::test]
async fn residency_colour_tags_and_name_are_checked() {
    let svc = service();
    for body in [
        serde_json::json!({}),
        serde_json::json!({"name": ""}),
        serde_json::json!({"name": "n".repeat(256)}),
        serde_json::json!({"name": "a", "display_color": "red"}),
        serde_json::json!({"name": "a", "display_color": "#12345g"}),
        serde_json::json!({"name": "a", "tags": {"anthropic_x": "1"}}),
        serde_json::json!({"name": "a", "data_residency": {"allowed_inference_geos": ["us"]}}),
    ] {
        assert!(svc.create(&admin("org_a"), create(body.clone())).await.is_err(), "{body}");
    }
    let ws = svc
        .create(
            &admin("org_a"),
            create(serde_json::json!({
                "name": "a", "display_color": "#A1b2C3", "tags": {"team": "x"},
                "data_residency": {"allowed_inference_geos": ["us"], "default_inference_geo": "us"}
            })),
        )
        .await
        .unwrap();
    assert_eq!(ws.data_residency.allowed_inference_geos, AllowedGeos::Only(vec![InferenceGeo::Us]));
    assert_eq!(ws.display_color, "#A1b2C3");
}

#[test]
fn unknown_fields_and_geos_do_not_parse() {
    for body in [
        serde_json::json!({"name": "a", "colour": "#000000"}),
        serde_json::json!({"name": "a", "data_residency": {"workspace_geo": "eu"}}),
        serde_json::json!({"name": "a", "data_residency": {"allowed_inference_geos": "anywhere"}}),
    ] {
        assert!(serde_json::from_value::<CreateWorkspaceRequest>(body.clone()).is_err(), "{body}");
    }
}

#[tokio::test]
async fn an_update_changes_only_what_it_names_and_merges_tags() {
    let svc = service();
    let org = admin("org_a");
    let body = serde_json::json!({"name": "a", "tags": {"team": "x", "env": "prod"}});
    let ws = svc.create(&org, create(body)).await.unwrap();
    let body = serde_json::json!({"name": "b", "tags": {"env": null, "owner": "y"}});
    let updated = svc.update(&org, &ws.id, update(body)).await.unwrap();
    assert_eq!(updated.name, "b");
    assert_eq!(updated.display_color, ws.display_color);
    assert_eq!(serde_json::to_value(&updated.tags).unwrap(), serde_json::json!({"team": "x", "owner": "y"}));
    assert_eq!(svc.get(&org, &ws.id).await.unwrap(), updated);
    let body = serde_json::json!({"data_residency": {"allowed_inference_geos": ["us"], "default_inference_geo": "us"}});
    let us = svc.update(&org, &ws.id, update(body)).await.unwrap();
    assert_eq!(us.data_residency.default_inference_geo, InferenceGeo::Us);
    assert_eq!(us.created_at, ws.created_at);
}

#[tokio::test]
async fn an_update_is_checked_like_a_create() {
    let svc = service();
    let org = admin("org_a");
    let ws = svc.create(&org, create(serde_json::json!({"name": "a"}))).await.unwrap();
    for body in [
        serde_json::json!({"name": ""}),
        serde_json::json!({"display_color": "#fff"}),
        serde_json::json!({"tags": {"anthropic": "x"}}),
        serde_json::json!({"data_residency": {"allowed_inference_geos": ["us"]}}),
    ] {
        assert!(svc.update(&org, &ws.id, update(body.clone())).await.is_err(), "{body}");
    }
    assert_eq!(svc.get(&org, &ws.id).await.unwrap(), ws, "a refused update changes nothing");
    assert!(serde_json::from_value::<UpdateWorkspaceRequest>(
        serde_json::json!({"data_residency": {"workspace_geo": "us"}})
    )
    .is_err());
    assert_eq!(
        svc.update(&admin("org_b"), &ws.id, update(serde_json::json!({}))).await.unwrap_err().status,
        404
    );
}

#[tokio::test]
async fn the_external_key_is_set_once() {
    let svc = service();
    let org = admin("org_a");
    let ws = svc.create(&org, create(serde_json::json!({"name": "a"}))).await.unwrap();
    let set =
        svc.update(&org, &ws.id, update(serde_json::json!({"external_key_id": "ekey_1"}))).await.unwrap();
    assert_eq!(set.external_key_id.as_deref(), Some("ekey_1"));
    assert!(svc.update(&org, &ws.id, update(serde_json::json!({"external_key_id": "ekey_1"}))).await.is_ok());
    assert!(svc
        .update(&org, &ws.id, update(serde_json::json!({"external_key_id": "ekey_2"})))
        .await
        .is_err());
}

#[tokio::test]
async fn an_archive_is_one_way_and_revokes_the_workspaces_keys() {
    let (svc, revoked) = revoking();
    let org = admin("org_a");
    let ws = svc.create(&org, create(serde_json::json!({"name": "a"}))).await.unwrap();
    let archived = svc.archive(&org, &ws.id).await.unwrap();
    let at = archived.archived_at.unwrap();
    assert_eq!(svc.get(&org, &ws.id).await.unwrap(), archived);
    let again = svc.archive(&org, &ws.id).await.unwrap();
    assert_eq!(again.archived_at, Some(at), "archiving again keeps the first time");
    assert_eq!(*revoked.0.lock(), [ws.id.clone(), ws.id.clone()], "each archive revokes what is left");
    let cursor = IdCursor::from_params(&Default::default()).unwrap();
    assert!(svc.list(&org, false, &cursor).await.unwrap().0.is_empty());
    assert_eq!(svc.list(&org, true, &cursor).await.unwrap().0.len(), 1);
    assert_eq!(svc.archive(&admin("org_b"), &ws.id).await.unwrap_err().status, 404);
    assert!(revoked.0.lock().len() == 2, "a foreign archive revokes nothing");
}
