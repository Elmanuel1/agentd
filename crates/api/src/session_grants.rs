//! What a session grants the worker serving it, read from the session: the
//! files it mounts, its agents' skills, the memory stores it attaches and
//! how, and the `environment_variable` credentials of its vaults. Each read
//! is one session read; the credentials add one read of the vaults'
//! credential rows and never a secret.

use std::sync::Arc;

use agents::types::{Multiagent, RosterMember};
use agents::AgentSnapshot;
use async_trait::async_trait;
use files::SessionFiles;
use sessions::types::{MemoryAccess, Session, SessionId, SessionResource};
use sessions::SessionsStore;
use shared::{ApiError, AuthContext};
use vaults::egress::VaultCredentials;
use work::{SessionVariables, Variable};

use crate::edge::reach::{Grants, SessionGrants};

/// The session-side ports over the session rows and the vaults' credentials.
#[derive(Clone)]
pub struct SessionReach {
    /// Session rows.
    pub sessions: Arc<dyn SessionsStore>,
    /// The vaults' credential rows.
    pub vaults: VaultCredentials,
}

impl std::fmt::Debug for SessionReach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionReach")
    }
}

impl SessionReach {
    async fn session(&self, ctx: &AuthContext, session: &str) -> Result<Session, ApiError> {
        let not_found = || ApiError::not_found("session not found");
        let id = SessionId::parse(session).map_err(|_| not_found())?;
        self.sessions.get(&ctx.workspace, &id).await.map_err(|e| match e {
            sessions::store::StoreError::NotFound => not_found(),
            other => ApiError::internal(other.to_string()),
        })
    }
}

/// The skills of `agent` and of every agent on its roster.
fn skills(agent: &AgentSnapshot) -> Vec<String> {
    let mut out: Vec<String> = agent.config.skills.iter().map(|s| s.skill_id().to_owned()).collect();
    if let Some(Multiagent::Coordinator { agents }) = &agent.config.multiagent {
        for member in agents {
            if let RosterMember::Snapshot(roster) = member {
                out.extend(skills(roster));
            }
        }
    }
    out
}

#[async_trait]
impl SessionGrants for SessionReach {
    async fn grants(&self, ctx: &AuthContext, session: &str) -> Result<Grants, ApiError> {
        let s = self.session(ctx, session).await?;
        let memory_stores = s
            .resources
            .iter()
            .filter_map(|r| match r {
                SessionResource::MemoryStore { memory_store_id, access, .. } => {
                    Some((memory_store_id.clone(), *access != Some(MemoryAccess::ReadOnly)))
                }
                _ => None,
            })
            .collect();
        Ok(Grants { skills: skills(&s.agent), memory_stores })
    }
}

#[async_trait]
impl SessionFiles for SessionReach {
    async fn mounts(&self, ctx: &AuthContext, session: &str) -> Result<Vec<String>, ApiError> {
        let s = self.session(ctx, session).await?;
        Ok(s.resources
            .iter()
            .filter_map(|r| match r {
                SessionResource::File { file_id, .. } => Some(file_id.clone()),
                _ => None,
            })
            .collect())
    }
}

#[async_trait]
impl SessionVariables for SessionReach {
    async fn variables(&self, ctx: &AuthContext, session: &str) -> Result<Vec<Variable>, ApiError> {
        let s = self.session(ctx, session).await?;
        let listed = self.vaults.environment_credentials(&ctx.workspace, &s.vault_ids).await?;
        Ok(listed
            .into_iter()
            .map(|c| Variable { name: c.name, vault: c.vault_id, credential: c.credential_id })
            .collect())
    }
}
