use super::*;

#[tokio::test]
async fn fake_scheduler_lifecycle() {
    let s = FakeScheduler::new();
    let target = FireTarget::new("deployment", "depl_01x");
    s.upsert(ScheduleSpec {
        cron: "0 0 * * *".into(),
        timezone: "UTC".into(),
        target: target.clone(),
        tenant: tenant(),
    })
    .await
    .unwrap();
    assert!(!s.describe(&target).await.unwrap().paused);
    s.pause(&target).await.unwrap();
    assert!(s.describe(&target).await.unwrap().paused);
    s.unpause(&target).await.unwrap();
    assert!(!s.describe(&target).await.unwrap().paused);
    s.delete(&target).await.unwrap();
    assert!(matches!(s.describe(&target).await, Err(SchedulerError::Unknown(_))));
}

#[tokio::test]
async fn fake_scheduler_validates_cron_via_shared_engine() {
    let s = FakeScheduler::new();
    let bad = s
        .upsert(ScheduleSpec {
            cron: "99 * * * *".into(),
            timezone: "UTC".into(),
            target: FireTarget::new("dream", "d1"),
            tenant: tenant(),
        })
        .await;
    assert!(matches!(bad, Err(SchedulerError::InvalidSpec(_))));
}

fn tenant() -> TenantRef {
    TenantRef { org_id: "org_t".into(), workspace_id: "wrkspc_01t".into(), api_key_id: "apikey_01t".into() }
}
