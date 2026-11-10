//! A work secret's `environment_variables`: for a `cloud` session, one entry
//! per unarchived `environment_variable` credential in its vaults, named by
//! its `secret_name` and set to its placeholder, never its value; for a
//! `self_hosted` session, none, as on live.

use std::collections::BTreeMap;

use async_trait::async_trait;
use shared::{ApiError, AuthContext};

use crate::placeholder::{PlaceholderClaims, Placeholders};

/// One `environment_variable` credential a session's vaults hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    /// Its `secret_name`, the variable the sandbox sees.
    pub name: String,
    /// Its vault.
    pub vault: String,
    /// Its id.
    pub credential: String,
}

/// What the work API asks of sessions and vaults.
#[async_trait]
pub trait SessionVariables: Send + Sync {
    /// The unarchived `environment_variable` credentials of `session`'s
    /// vaults, one per name: the first vault in its `vault_ids` wins a name.
    async fn variables(&self, ctx: &AuthContext, session: &str) -> Result<Vec<Variable>, ApiError>;
}

/// Builds `environment_variables` for `cloud` hand-outs.
#[derive(Clone)]
pub struct CloudVariables {
    /// Lists a session's credentials.
    pub source: std::sync::Arc<dyn SessionVariables>,
    /// Signs their placeholders.
    pub placeholders: Placeholders,
}

impl std::fmt::Debug for CloudVariables {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CloudVariables")
    }
}

impl CloudVariables {
    /// `session`'s variables, each set to its placeholder.
    pub async fn of(&self, ctx: &AuthContext, session: &str) -> Result<BTreeMap<String, String>, ApiError> {
        let listed = self.source.variables(ctx, session).await?;
        Ok(listed
            .into_iter()
            .map(|v| {
                let claims = PlaceholderClaims {
                    session: session.to_owned(),
                    ws: ctx.workspace.as_str().to_owned(),
                    vault: v.vault,
                    credential: v.credential,
                };
                (v.name, self.placeholders.sign(&claims))
            })
            .collect())
    }
}
