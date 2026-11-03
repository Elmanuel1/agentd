use std::time::Duration;

use shared::{AuthContext, OrgId, WorkspaceId};

use crate::service::WorkScope;
use crate::{WorkService, WorkState, WorkTokens};

fn ctx(ws: &str) -> AuthContext {
    AuthContext { org: OrgId::new("org_1"), workspace: WorkspaceId::new(ws), api_key_id: "apikey_1".into() }
}

#[tokio::test]
async fn a_hosted_work_item_runs_through_ack_heartbeat_and_stop() {
    let svc = WorkService::new(WorkTokens::new(b"k".to_vec()));
    let (work, secret) = svc.issue(&ctx("wrkspc_1"), "env_1", "sesn_1", true);
    assert!(!secret.is_empty());
    let id = work.id.as_str();
    assert!(svc.poll(&ctx("wrkspc_1"), "env_1", Duration::ZERO).await.is_none(), "hosted work is not polled");
    assert_eq!(svc.ack(&ctx("wrkspc_1"), None, "env_1", id).unwrap().state, WorkState::Starting);
    let first = svc.heartbeat(&ctx("wrkspc_1"), None, "env_1", id, Some("NO_HEARTBEAT"), None).unwrap();
    assert!(first.lease_extended && first.state == WorkState::Active && first.ttl_seconds == 90);
    let stale = svc.heartbeat(&ctx("wrkspc_1"), None, "env_1", id, Some("NO_HEARTBEAT"), None).unwrap_err();
    assert_eq!(stale.status, 412);
    let next =
        svc.heartbeat(&ctx("wrkspc_1"), None, "env_1", id, Some(&first.last_heartbeat), Some(30)).unwrap();
    assert_eq!(next.ttl_seconds, 30);
    assert_eq!(svc.stop(&ctx("wrkspc_1"), None, "env_1", id, false).unwrap().state, WorkState::Stopping);
    let after = svc.heartbeat(&ctx("wrkspc_1"), None, "env_1", id, Some(&next.last_heartbeat), None).unwrap();
    assert!(!after.lease_extended && after.state == WorkState::Stopping);
}

#[tokio::test]
async fn a_self_hosted_poll_hands_out_queued_work_with_its_secret() {
    let svc = WorkService::new(WorkTokens::new(b"k".to_vec()));
    let (work, _) = svc.issue(&ctx("wrkspc_1"), "env_1", "sesn_1", false);
    let polled = svc.poll(&ctx("wrkspc_1"), "env_1", Duration::from_millis(10)).await.unwrap();
    assert_eq!(polled.id, work.id);
    assert!(polled.secret.is_some());
    assert!(svc.retrieve(&ctx("wrkspc_1"), None, "env_1", work.id.as_str()).unwrap().secret.is_none());
    assert!(
        svc.poll(&ctx("wrkspc_1"), "env_1", Duration::ZERO).await.is_none(),
        "handed out, not yet reclaimable"
    );
    assert_eq!(svc.stats(&ctx("wrkspc_1"), "env_1").pending, 1);
}

#[tokio::test]
async fn work_is_invisible_outside_its_workspace_and_to_other_tokens() {
    let svc = WorkService::new(WorkTokens::new(b"k".to_vec()));
    let (work, _) = svc.issue(&ctx("wrkspc_1"), "env_1", "sesn_1", true);
    let (other, _) = svc.issue(&ctx("wrkspc_1"), "env_1", "sesn_2", true);
    assert_eq!(svc.retrieve(&ctx("wrkspc_2"), None, "env_1", work.id.as_str()).unwrap_err().status, 404);
    assert_eq!(svc.retrieve(&ctx("wrkspc_1"), None, "env_2", work.id.as_str()).unwrap_err().status, 404);
    let claims = crate::WorkClaims {
        work: work.id.as_str().into(),
        session: "sesn_1".into(),
        ws: "wrkspc_1".into(),
        org: "org_1".into(),
    };
    let secret: serde_json::Value =
        serde_json::from_slice(&base64_decode(&svc.tokens().secret(&claims))).unwrap();
    let claims = svc.tokens().verify(secret["sessions_token"].as_str().unwrap()).unwrap();
    let scope = WorkScope(claims);
    assert!(svc.retrieve(&ctx("wrkspc_1"), Some(&scope), "env_1", work.id.as_str()).is_ok());
    assert_eq!(
        svc.retrieve(&ctx("wrkspc_1"), Some(&scope), "env_1", other.id.as_str()).unwrap_err().status,
        404
    );
}

fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).unwrap()
}
