//! The keys from the environment.

use shared::ports::keys::material::{self, KeyKind};
use shared::ports::secrets::ExposeSecret;

use super::config;

#[test]
fn the_peppers_are_required_and_must_be_a_key_ring() {
    assert_eq!(config(&[("AGENTD_API_KEY_PEPPERS", "")]).unwrap_err().var, "AGENTD_API_KEY_PEPPERS");
    assert_eq!(config(&[("AGENTD_API_KEY_PEPPERS", "p1:short")]).unwrap_err().var, "AGENTD_API_KEY_PEPPERS");
}

#[test]
fn runtime_keys_are_read_in_order_and_a_malformed_one_is_refused() {
    let (a, b) = (material::generate(KeyKind::Environment), material::generate(KeyKind::Environment));
    let both = format!("{}, {}", a.expose_secret(), b.expose_secret());
    let keys = config(&[("AGENTD_RUNTIME_KEYS", &both)]).unwrap().keys;
    let read: Vec<&str> = keys.runtime.iter().map(|k| k.expose_secret()).collect();
    assert_eq!(read, [a.expose_secret(), b.expose_secret()]);
    let workspace = material::generate(KeyKind::Workspace);
    let err = config(&[("AGENTD_RUNTIME_KEYS", workspace.expose_secret())]).unwrap_err();
    assert_eq!(err.var, "AGENTD_RUNTIME_KEYS");
}

#[test]
fn model_keys_are_read_and_no_key_reaches_debug() {
    let c = config(&[
        ("AGENTD_ANTHROPIC_API_KEY", "sk-ant-model-secret"),
        ("AGENTD_OPENROUTER_API_KEY", " sk-or-model-secret "),
    ])
    .unwrap();
    assert_eq!(c.keys.anthropic.as_ref().map(|k| k.expose_secret()), Some("sk-ant-model-secret"));
    assert_eq!(c.keys.openrouter.as_ref().map(|k| k.expose_secret()), Some("sk-or-model-secret"));
    let shown = format!("{c:?}");
    assert!(!shown.contains("model-secret") && !shown.contains("202122"), "{shown}");
}
