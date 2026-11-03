use super::*;
use shared::{ApiError, Patch};

const SESSION_FIXTURE: &str = include_str!("../../../../../conformance/fixtures/session.create.json");

#[test]
fn fixture_roundtrip_is_lossless() {
    // The July capture predates the agent's `execution_identity` and `budget`.
    let mut original: serde_json::Value = serde_json::from_str(SESSION_FIXTURE).unwrap();
    original["agent"]["execution_identity"] = serde_json::json!({"type": "service_account"});
    original["budget"] = serde_json::Value::Null;
    let s: Session = serde_json::from_str(SESSION_FIXTURE).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), original);
}

#[test]
fn agent_ref_input_forms() {
    let a: AgentRefInput = serde_json::from_str("\"agent_01XdReZgC7eHpg9b9Nvrwdcm\"").unwrap();
    assert_eq!(a.id(), "agent_01XdReZgC7eHpg9b9Nvrwdcm");
    assert_eq!(a.version(), None);
    let b: AgentRefInput =
        serde_json::from_str(r#"{"type":"agent","id":"agent_01XdReZgC7eHpg9b9Nvrwdcm","version":3}"#)
            .unwrap();
    assert_eq!(b.version(), Some(3));
}

#[test]
fn repo_resource_token_is_never_serialized() {
    let input: SessionResourceInput = serde_json::from_str(
        r#"{"type":"github_repository","url":"https://github.com/o/r","authorization_token":"ghp_secret"}"#,
    )
    .unwrap();
    // The wire type has no token field at all — constructing it drops it.
    let wire = match input {
        SessionResourceInput::GithubRepository { url, mount_path, checkout, .. } => {
            SessionResource::GithubRepository {
                id: None,
                url,
                mount_path,
                checkout,
                created_at: None,
                updated_at: None,
            }
        }
        _ => panic!("wrong variant"),
    };
    let s = serde_json::to_string(&wire).unwrap();
    assert!(!s.contains("ghp_secret"));
    assert!(!s.contains("authorization_token"));
}

#[test]
fn outcome_result_terminality() {
    assert!(OutcomeResult::Satisfied.is_terminal());
    assert!(!OutcomeResult::NeedsRevision.is_terminal());
    assert!(OutcomeResult::MaxIterationsReached.is_terminal());
    assert!(OutcomeResult::Failed.is_terminal());
    assert!(OutcomeResult::Interrupted.is_terminal());
}

fn budgeted(max: u64, spent: Option<u64>) -> Session {
    let mut s: Session = serde_json::from_str(SESSION_FIXTURE).unwrap();
    s.budget = Some(Budget::Limit { max_list_cost: MonetaryAmount::usd(max) });
    s.usage.input_tokens = 100;
    s.usage.list_cost = spent.map(MonetaryAmount::usd);
    s
}

fn reason(e: &ApiError) -> &str {
    e.extra.get("reason").and_then(serde_json::Value::as_str).unwrap_or_default()
}

#[test]
fn a_budget_is_replaced_only_above_the_cost_so_far_and_never_re_added() {
    let limit = |n| Patch::Set(Budget::Limit { max_list_cost: MonetaryAmount::usd(n) });
    let mut s = budgeted(10, Some(12));
    assert_eq!(reason(&s.change_budget(limit(12)).unwrap_err()), "budget_not_raised");
    s.change_budget(limit(100)).unwrap();
    assert_eq!(s.budget, Some(Budget::Limit { max_list_cost: MonetaryAmount::usd(100) }));
    s.change_budget(Patch::Keep).unwrap();
    assert!(s.budget.is_some());
    s.change_budget(Patch::Clear).unwrap();
    assert_eq!(reason(&s.change_budget(limit(500)).unwrap_err()), "budget_create_only");
    let mut unpriced = budgeted(10, None);
    assert_eq!(reason(&unpriced.change_budget(limit(500)).unwrap_err()), "model_not_budgetable");
    let mut fresh = budgeted(10, None);
    fresh.usage.input_tokens = 0;
    fresh.change_budget(limit(1)).unwrap();
    let bad = Patch::Set(Budget::Limit {
        max_list_cost: MonetaryAmount { amount: "5.00".into(), currency: "USD".into() },
    });
    assert_eq!(fresh.change_budget(bad).unwrap_err().status, 400);
    fresh.status = SessionStatus::Terminated;
    assert_eq!(fresh.change_budget(Patch::Clear).unwrap_err().status, 409);
}
