use async_trait::async_trait;
use parking_lot::Mutex;
use shared::OrgId;

use super::*;
use crate::workspaces::types::{AllowedGeos, InferenceGeo};

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

fn admin(org: &str) -> AdminContext {
    AdminContext { org: OrgId::new(org), admin_key_id: "adminkey_t".into() }
}

fn service() -> WorkspacesService {
    WorkspacesService::new(Arc::new(MemoryWorkspaces::default()))
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
