//! Cross-entity dependency glue — implements the ports each entity crate
//! declared, over the real services. This is the ONLY place entities are
//! wired to each other, keeping the crate graph a DAG.

use std::sync::Arc;

use agents::types::{Multiagent, RosterMember};
use agents::{AgentRef, AgentSnapshot, AgentsService};
use api_keys::ApiKeysStore;
use async_trait::async_trait;
use deployments::service::DeploymentDeps;
use deployments::types::{Deployment, InitialEvent};
use environments::EnvironmentsService;
use organizations::workspaces::WorkspaceKeys;
use sessions::service::SessionDeps;
use sessions::SessionsService;
use shared::{ApiError, AuthContext, OrgId, Timestamp, WorkspaceId};

/// SessionDeps over the real agents + environments services.
pub struct RealSessionDeps {
    /// Agents service.
    pub agents: Arc<AgentsService>,
    /// Environments service.
    pub environments: Arc<EnvironmentsService>,
}

impl std::fmt::Debug for RealSessionDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealSessionDeps").finish_non_exhaustive()
    }
}

#[async_trait]
impl SessionDeps for RealSessionDeps {
    async fn resolve_agent(
        &self,
        ctx: &AuthContext,
        agent_id: &str,
        version: Option<u64>,
    ) -> Result<AgentSnapshot, ApiError> {
        // Archive stamps the latest version, so it decides for every version.
        let latest = self.agents.get(ctx, agent_id, None).await?;
        if latest.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request(
                "agent is archived; new sessions cannot reference it",
            ));
        }
        let agent = match version {
            Some(v) if v != latest.version => self.agents.get(ctx, agent_id, Some(v)).await?,
            _ => latest,
        };
        let mut snapshot = agent.snapshot();
        // A session's snapshot carries each roster agent in full, as live
        // serves it, so the session runs the versions it started with.
        if let Some(Multiagent::Coordinator { agents }) = &mut snapshot.config.multiagent {
            for member in agents.iter_mut() {
                if let RosterMember::Agent(r) = member {
                    let full = self.agents.get(ctx, r.id.as_str(), Some(r.version)).await?;
                    *member = RosterMember::Snapshot(Box::new(full.snapshot()));
                }
            }
        }
        Ok(snapshot)
    }

    async fn resolve_environment(
        &self,
        ctx: &AuthContext,
        env_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        let env = self.environments.get(ctx, env_id).await?;
        if env.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request(
                "environment is archived; new sessions cannot reference it",
            ));
        }
        serde_json::to_value(&env.config).map_err(|e| ApiError::internal(e.to_string()))
    }
}

/// DeploymentDeps over the real agents/environments/sessions services.
pub struct RealDeploymentDeps {
    /// Agents.
    pub agents: Arc<AgentsService>,
    /// Environments.
    pub environments: Arc<EnvironmentsService>,
    /// Sessions (fires create sessions).
    pub sessions: Arc<SessionsService>,
}

impl std::fmt::Debug for RealDeploymentDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealDeploymentDeps").finish_non_exhaustive()
    }
}

#[async_trait]
impl DeploymentDeps for RealDeploymentDeps {
    async fn resolve_agent_ref(
        &self,
        ctx: &AuthContext,
        agent_id: &str,
        version: Option<u64>,
    ) -> Result<AgentRef, ApiError> {
        let agent = self.agents.get(ctx, agent_id, None).await?;
        if agent.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request(
                "agent is archived; new sessions cannot reference it",
            ));
        }
        let mut r = agent.reference();
        if let Some(v) = version {
            r.version = v;
        }
        Ok(r)
    }

    async fn check_environment(&self, ctx: &AuthContext, env_id: &str) -> Result<(), ApiError> {
        let env = self.environments.get(ctx, env_id).await?;
        if env.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("environment is archived"));
        }
        Ok(())
    }

    async fn create_session(&self, ctx: &AuthContext, deployment: &Deployment) -> Result<String, ApiError> {
        // Build the session create from the frozen deployment config.
        let mut req: sessions::types::CreateSessionRequest = serde_json::from_value(serde_json::json!({
            "agent": {
                "type": "agent",
                "id": deployment.agent.id.as_str(),
                "version": deployment.agent.version,
            },
            "environment_id": deployment.environment_id,
            "title": deployment.name,
            "vault_ids": deployment.vault_ids,
        }))
        .map_err(|e| ApiError::internal(e.to_string()))?;
        req.deployment_id = Some(deployment.id.as_str().to_owned());
        let session = self.sessions.create(ctx, req).await?;
        // Queue the deployment's initial events as the kickoff.
        let payloads: Vec<sessions::events::EventPayload> = deployment
            .initial_events
            .iter()
            .map(|e| match e {
                InitialEvent::UserMessage { content } => {
                    sessions::events::EventPayload::User(sessions::events::UserEvent::Message {
                        content: content.clone(),
                    })
                }
            })
            .collect();
        self.sessions.send_events(ctx, session.id.as_str(), payloads).await?;
        Ok(session.id.as_str().to_owned())
    }
}

/// WorkspaceKeys over the api-keys store: an archive revokes the workspace's
/// keys in one statement.
pub struct RealWorkspaceKeys {
    /// Workspace API keys.
    pub keys: Arc<dyn ApiKeysStore>,
}

impl std::fmt::Debug for RealWorkspaceKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealWorkspaceKeys").finish_non_exhaustive()
    }
}

#[async_trait]
impl WorkspaceKeys for RealWorkspaceKeys {
    async fn revoke_all(&self, org: &OrgId, workspace: &str, at: Timestamp) -> Result<(), ApiError> {
        let workspace = WorkspaceId::new(workspace);
        self.keys
            .revoke_workspace(org, &workspace, at)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(())
    }
}
