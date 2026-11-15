//! Reaching one server: its token, a stored session opened again, a fresh
//! connection stored, and its tools listed again.

use serde_json::Value;
use sessions::mcp::McpConnection;
use shared::BackendError;
use vaults::mcp::Refresh;
use vaults::types::CredentialAuthWire;

use super::super::client::{Failure, Rpc};
use super::super::errors::session_error;
use super::super::server;
use super::super::Mcp;
use super::Servers;

impl Servers {
    /// Server `i`'s bearer: none with no credential or a secret gone, the
    /// stored token while it holds, else a refreshed one (MC-7). A refresh
    /// another worker holds defers the step.
    pub(super) async fn token(&mut self, i: usize) -> Result<Result<Option<String>, Failure>, BackendError> {
        let Some(mcp) = self.mcp.clone() else { return Ok(Ok(None)) };
        let Some(cred) = self.list.get(i).and_then(|s| s.credential.clone()) else { return Ok(Ok(None)) };
        let Some(token) = mcp.vaults.mcp_token(&self.ws, &cred).await? else { return Ok(Ok(None)) };
        let refreshable = matches!(&cred.auth, CredentialAuthWire::McpOauth { refresh: Some(_), .. });
        if !token.expired || !refreshable {
            return Ok(Ok(Some(token.bearer.expose().to_owned())));
        }
        let Some(org) = self.org.clone() else { return Ok(Err(Failure::Refresh)) };
        match mcp.vaults.refresh_mcp_oauth(&self.ws, &org, &cred).await? {
            Refresh::Token(token, wire) => {
                if let Some(s) = self.list.get_mut(i) {
                    s.credential = Some(*wire);
                }
                Ok(Ok(Some(token.bearer.expose().to_owned())))
            }
            Refresh::Busy => {
                self.deferred = true;
                Ok(Err(Failure::Busy))
            }
            Refresh::Failed => Ok(Err(Failure::Refresh)),
        }
    }

    /// Open the stored session again, reading the credential's token.
    pub(super) async fn resume(&mut self, i: usize) -> Result<Result<(), Failure>, BackendError> {
        let bearer = match self.token(i).await? {
            Ok(bearer) => bearer,
            Err(Failure::Busy) => return Ok(Err(Failure::Busy)),
            Err(f) => {
                if let Some(s) = self.list.get_mut(i) {
                    s.failed = Some(f.clone());
                }
                return Ok(Err(f));
            }
        };
        let (Some(mcp), Some(s)) = (self.mcp.clone(), self.list.get_mut(i)) else {
            return Ok(Err(Failure::Unreachable));
        };
        let session = s.stored.as_ref().and_then(|c| c.mcp_session_id.clone());
        let opened = match s.allowed {
            true => Rpc::open(&mcp.guard, &s.url, bearer, session, mcp.timeout).await,
            false => Err(Failure::Unreachable),
        };
        match opened {
            Ok(rpc) => {
                s.rpc = Some(rpc);
                Ok(Ok(()))
            }
            Err(f) => {
                s.failed = Some(f.clone());
                Ok(Err(f))
            }
        }
    }

    /// Connect server `i` afresh and store what it lists; the
    /// `session.error` body when it fails.
    pub(super) async fn reach(&mut self, i: usize) -> Result<Result<(), Value>, BackendError> {
        let Some(mcp) = self.mcp.clone() else { return Ok(Ok(())) };
        let allowed = self.list.get(i).is_some_and(|s| s.allowed);
        let bearer = match allowed {
            true => self.token(i).await?,
            false => Err(Failure::Unreachable),
        };
        if bearer == Err(Failure::Busy) {
            return Ok(Ok(()));
        }
        let Some(s) = self.list.get_mut(i) else { return Ok(Ok(())) };
        let opened = match bearer {
            Ok(bearer) => connect(&mcp, &s.url, bearer).await,
            Err(f) => Err(f),
        };
        match opened {
            Ok((rpc, tools)) => {
                let row = McpConnection {
                    server_url: s.url.clone(),
                    mcp_session_id: rpc.session.clone(),
                    tools: Value::Array(tools),
                    credential_updated_at: s.credential.as_ref().map(|c| c.updated_at),
                };
                mcp.connections.save(&self.ws, &self.session, &row).await.map_err(BackendError::new)?;
                s.stored = Some(row);
                s.rpc = Some(rpc);
                s.failed = None;
                Ok(Ok(()))
            }
            Err(f) => {
                let error = session_error(&s.name, "initialize", &f);
                s.failed = Some(f);
                s.rpc = None;
                Ok(Err(error))
            }
        }
    }

    /// List server `i`'s tools again and store them (MC-16).
    pub(super) async fn relist(&mut self, i: usize) -> Result<(), BackendError> {
        let Some(mcp) = self.mcp.clone() else { return Ok(()) };
        let Some(s) = self.list.get_mut(i) else { return Ok(()) };
        let (Some(rpc), Some(stored)) = (s.rpc.as_mut(), s.stored.as_mut()) else { return Ok(()) };
        if let Ok(tools) = server::list(rpc).await {
            stored.tools = Value::Array(tools);
            stored.mcp_session_id = rpc.session.clone();
            mcp.connections.save(&self.ws, &self.session, stored).await.map_err(BackendError::new)?;
        }
        Ok(())
    }
}

async fn connect(mcp: &Mcp, url: &str, bearer: Option<String>) -> Result<(Rpc, Vec<Value>), Failure> {
    let mut rpc = Rpc::open(&mcp.guard, url, bearer, None, mcp.timeout).await?;
    server::initialize(&mut rpc).await?;
    let tools = server::list(&mut rpc).await?;
    Ok((rpc, tools))
}
