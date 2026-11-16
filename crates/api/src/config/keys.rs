//! The keys agentd holds from its environment: the peppers every API key
//! digest is keyed with, its runtime keys and the model providers' keys. How
//! the deployment fills these variables (a secrets manager, a task
//! definition) is the deployment's concern.

use shared::hash::KeyRing;
use shared::ports::keys::material::{self, KeyKind};
use shared::ports::secrets::{ExposeSecret, SecretString};

use super::ConfigError;

/// Keys from the environment.
#[derive(Clone)]
pub struct KeysConfig {
    /// `AGENTD_API_KEY_PEPPERS` (required): `<id>:<hex>,…`. The first keys
    /// every new digest; a key stored under another is found under it and
    /// moved to the first.
    pub peppers: KeyRing,
    /// `AGENTD_RUNTIME_KEYS`: agentd's runtime keys (`sk-ant-oat01-…`),
    /// comma separated, the current one first. Each reaches the runtime
    /// queue, which the poller of `cloud` environments polls with the first.
    pub runtime: Vec<SecretString>,
    /// `AGENTD_ANTHROPIC_API_KEY`: the loop's key for the Messages API.
    pub anthropic: Option<SecretString>,
    /// `AGENTD_OPENROUTER_API_KEY`: the loop's key for the OpenAI-compatible
    /// endpoint at `AGENTD_MODEL_BASE_URL`.
    pub openrouter: Option<SecretString>,
}

impl std::fmt::Debug for KeysConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeysConfig")
            .field("peppers", &self.peppers)
            .field("runtime", &self.runtime.len())
            .field("anthropic", &self.anthropic.is_some())
            .field("openrouter", &self.openrouter.is_some())
            .finish()
    }
}

fn same(a: &[SecretString], b: &[SecretString]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.expose_secret() == b.expose_secret())
}

impl PartialEq for KeysConfig {
    fn eq(&self, other: &Self) -> bool {
        self.peppers == other.peppers
            && same(&self.runtime, &other.runtime)
            && same(self.anthropic.as_slice(), other.anthropic.as_slice())
            && same(self.openrouter.as_slice(), other.openrouter.as_slice())
    }
}

impl Eq for KeysConfig {}

impl KeysConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let var = "AGENTD_API_KEY_PEPPERS";
        let raw = get(var).filter(|v| !v.is_empty());
        let raw =
            raw.ok_or_else(|| ConfigError { var, reason: "required: a key ring `<id>:<hex>,…`".into() })?;
        let peppers = KeyRing::parse(&raw).map_err(|e| ConfigError { var, reason: e.to_string() })?;
        let runtime: Vec<SecretString> = get("AGENTD_RUNTIME_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(SecretString::from)
            .collect();
        if runtime.iter().any(|k| !material::well_formed(KeyKind::Environment, k.expose_secret())) {
            let reason = "each key must be an `sk-ant-oat01-` environment key".into();
            return Err(ConfigError { var: "AGENTD_RUNTIME_KEYS", reason });
        }
        let key = |var: &str| get(var).filter(|v| !v.trim().is_empty()).map(|v| SecretString::from(v.trim()));
        Ok(Self {
            peppers,
            runtime,
            anthropic: key("AGENTD_ANTHROPIC_API_KEY"),
            openrouter: key("AGENTD_OPENROUTER_API_KEY"),
        })
    }
}
