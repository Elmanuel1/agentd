use std::sync::Arc;

use shared::OrgId;
use shared::ports::secrets::SecretKey;
use shared::ports::{FakeScheduler, FireTarget, ScheduleSpec, SchedulerPort, TenantRef};

use super::{Deleted, OrganizationDeletion};
use crate::registry::{Removed, StoreError};

mod fakes;
use fakes::{Blobs, Down, Rows, Secrets};

fn removed(blobs: &[&str]) -> Removed {
    Removed {
        workspaces: vec!["wrkspc_a".into()],
        rows: vec![("credentials", 2)],
        secrets: vec![
            SecretKey::new("wrkspc_a", "vlt_a", "vcrd_a").unwrap(),
            SecretKey::new("wrkspc_a", "vlt_a", "vcrd_b").unwrap(),
        ],
        blobs: blobs.iter().map(|b| (*b).to_owned()).collect(),
        dreams: vec![FireTarget::new("dream", "drm_a"), FireTarget::new("dream", "drm_b")],
    }
}

async fn scheduled(target: &FireTarget) -> Arc<FakeScheduler> {
    let scheduler = Arc::new(FakeScheduler::new());
    let spec = ScheduleSpec {
        target: target.clone(),
        cron: "0 * * * *".into(),
        timezone: "UTC".into(),
        tenant: TenantRef {
            org_id: "org_a".into(),
            workspace_id: "wrkspc_a".into(),
            api_key_id: "apikey_a".into(),
        },
    };
    scheduler.upsert(spec).await.unwrap();
    scheduler
}

#[tokio::test]
async fn a_delete_removes_the_secrets_blobs_and_dream_schedules_its_rows_named() {
    let (secrets, blobs) = (Arc::new(Secrets::default()), Arc::new(Blobs::default()));
    let scheduler = scheduled(&FireTarget::new("dream", "drm_a")).await;
    let removed = removed(&["wrkspc_a/file_a", "gone"]);
    let deletion = OrganizationDeletion::new(
        Arc::new(Rows(removed.clone())),
        secrets.clone(),
        blobs.clone(),
        scheduler.clone(),
    );
    let deleted = deletion.delete(&OrgId::new("org_a")).await.unwrap();
    let gone =
        ["secret wrkspc_a/vlt_a/vcrd_a", "secret wrkspc_a/vlt_a/vcrd_b", "blob wrkspc_a/file_a", "blob gone"]
            .into_iter()
            .chain(["schedule dream/drm_a", "schedule dream/drm_b"])
            .map(str::to_owned)
            .collect();
    assert_eq!(deleted, Deleted { removed, gone, left: vec![] });
    assert_eq!(*secrets.deleted.lock(), ["wrkspc_a/vlt_a/vcrd_a", "wrkspc_a/vlt_a/vcrd_b"]);
    assert_eq!(*blobs.deleted.lock(), ["wrkspc_a/file_a"]);
    assert!(scheduler.registered().is_empty(), "the dream's schedule is gone; one already gone counts");
}

#[tokio::test]
async fn what_could_not_be_removed_is_named_and_the_rest_still_goes() {
    let secrets = Arc::new(Secrets { refused: Some("wrkspc_a/vlt_a/vcrd_a".into()), ..Secrets::default() });
    let blobs = Arc::new(Blobs::default());
    let deletion = OrganizationDeletion::new(
        Arc::new(Rows(removed(&["refused", "wrkspc_a/file_b"]))),
        secrets.clone(),
        blobs.clone(),
        Arc::new(FakeScheduler::new()),
    );
    let deleted = deletion.delete(&OrgId::new("org_a")).await.unwrap();
    assert_eq!(
        deleted.left,
        ["secret wrkspc_a/vlt_a/vcrd_a: secret backend: refused", "blob refused: blob backend: refused"]
    );
    assert_eq!(
        deleted.gone,
        [
            "secret wrkspc_a/vlt_a/vcrd_b",
            "blob wrkspc_a/file_b",
            "schedule dream/drm_a",
            "schedule dream/drm_b"
        ]
    );
    assert_eq!(*secrets.deleted.lock(), ["wrkspc_a/vlt_a/vcrd_b"]);
    assert_eq!(*blobs.deleted.lock(), ["wrkspc_a/file_b"]);
}

#[tokio::test]
async fn a_dream_schedule_the_scheduler_cannot_delete_is_named() {
    let deletion = OrganizationDeletion::new(
        Arc::new(Rows(removed(&[]))),
        Arc::new(Secrets::default()),
        Arc::new(Blobs::default()),
        Arc::new(Down),
    );
    let deleted = deletion.delete(&OrgId::new("org_a")).await.unwrap();
    assert_eq!(
        deleted.left,
        [
            "schedule dream/drm_a: scheduler unavailable: down",
            "schedule dream/drm_b: scheduler unavailable: down"
        ]
    );
    assert_eq!(deleted.gone, ["secret wrkspc_a/vlt_a/vcrd_a", "secret wrkspc_a/vlt_a/vcrd_b"]);
}

#[tokio::test]
async fn an_unknown_organization_removes_nothing() {
    let secrets = Arc::new(Secrets::default());
    let deletion = OrganizationDeletion::new(
        Arc::new(Rows(removed(&["wrkspc_a/file_a"]))),
        secrets.clone(),
        Arc::new(Blobs::default()),
        Arc::new(FakeScheduler::new()),
    );
    let err = deletion.delete(&OrgId::new("org_gone")).await.unwrap_err();
    assert!(matches!(err, StoreError::NotFound(id) if id == "org_gone"));
    assert!(secrets.deleted.lock().is_empty());
}

#[test]
fn a_new_id_is_the_one_given_or_a_generated_org_id() {
    assert_eq!(crate::registry::new_id(Some("org_sdk")).as_str(), "org_sdk");
    let generated = crate::registry::new_id(None);
    assert!(generated.as_str().starts_with("org_01"), "{}", generated.as_str());
}
