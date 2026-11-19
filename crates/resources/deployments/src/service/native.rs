//! What adapters implement for the ports of the parent module; each comes with its `dyn` trait.

use super::*;

/// What an adapter implements to provide [`DeploymentDeps`]; the methods are documented there.
pub trait NativeDeploymentDeps: Send + Sync {
    fn resolve_agent_ref(
        &self,
        ctx: &AuthContext,
        agent_id: &str,
        version: Option<u64>,
    ) -> impl Future<Output = Result<AgentRef, ApiError>> + Send;
    fn check_environment(
        &self,
        ctx: &AuthContext,
        env_id: &str,
    ) -> impl Future<Output = Result<(), ApiError>> + Send;
    fn create_session(
        &self,
        ctx: &AuthContext,
        deployment: &Deployment,
        key: Option<&str>,
    ) -> impl Future<Output = Result<String, sessions::service::FireRefusal>> + Send;
}

impl<T: NativeDeploymentDeps> DeploymentDeps for T {
    fn resolve_agent_ref<'a>(
        &'a self,
        ctx: &'a AuthContext,
        agent_id: &'a str,
        version: Option<u64>,
    ) -> BoxFuture<'a, Result<AgentRef, ApiError>> {
        Box::pin(NativeDeploymentDeps::resolve_agent_ref(self, ctx, agent_id, version))
    }
    fn check_environment<'a>(
        &'a self,
        ctx: &'a AuthContext,
        env_id: &'a str,
    ) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(NativeDeploymentDeps::check_environment(self, ctx, env_id))
    }
    fn create_session<'a>(
        &'a self,
        ctx: &'a AuthContext,
        deployment: &'a Deployment,
        key: Option<&'a str>,
    ) -> BoxFuture<'a, Result<String, sessions::service::FireRefusal>> {
        Box::pin(NativeDeploymentDeps::create_session(self, ctx, deployment, key))
    }
}
