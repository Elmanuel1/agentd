//! The model ids agents may name beyond Anthropic's.

use super::config;

#[test]
fn agentd_models_takes_only_vendor_model_ids() {
    let models = config(&[("AGENTD_MODELS", "openai/gpt-6-luna, mock/scripted")]).unwrap().extra_models;
    assert_eq!(models, ["openai/gpt-6-luna", "mock/scripted"]);
    let e = config(&[("AGENTD_MODELS", "openai/gpt-6-luna,claude-haiku-5-5")]).unwrap_err();
    assert_eq!(e.var, "AGENTD_MODELS");
    assert_eq!(e.reason, "`claude-haiku-5-5` is not a `vendor/model` id; Anthropic's models need no entry");
}
