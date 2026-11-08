//! `/v1/sessions/{id}/resources`: the files, repositories and memory stores a
//! session mounts.

use shared::{ApiError, AuthContext, Timestamp};

use super::{guard_writable, map_store, parse_id, SessionsService};
use crate::events::EventPayload;
use crate::types::{Session, SessionResource, SessionResourceId, SessionResourceInput};

impl SessionsService {
    /// `GET …/resources`.
    pub async fn resources(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vec<SessionResource>, ApiError> {
        Ok(self.get(ctx, raw_id).await?.resources)
    }

    /// Whether the session reads `file_id`: a file it mounts, or the rubric
    /// one of its outcomes names.
    pub async fn reads_file(&self, ctx: &AuthContext, raw_id: &str, file_id: &str) -> Result<bool, ApiError> {
        let session = self.get(ctx, raw_id).await?;
        if session.mounts(file_id) {
            return Ok(true);
        }
        let events = self.events.events(&ctx.workspace, &session.id).await.map_err(map_store)?;
        Ok(events
            .iter()
            .any(|e| matches!(&e.payload, EventPayload::User(u) if u.rubric_file() == Some(file_id))))
    }

    /// `POST …/resources`: attach a file.
    pub async fn add_resource(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        input: SessionResourceInput,
    ) -> Result<SessionResource, ApiError> {
        if !matches!(input, SessionResourceInput::File { .. }) {
            return Err(ApiError::invalid_request("type: only file resources can be added to a session"));
        }
        let resource = input.into_resource(Timestamp::now());
        self.change(ctx, raw_id, |s| {
            s.resources.push(resource.clone());
            Ok(resource.clone())
        })
        .await
    }

    /// `GET …/resources/{rid}`.
    pub async fn resource(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        rid: &str,
    ) -> Result<SessionResource, ApiError> {
        let session = self.get(ctx, raw_id).await?;
        let rid = parse_resource_id(rid)?;
        session.resources.into_iter().find(|r| r.id() == Some(&rid)).ok_or_else(not_found)
    }

    /// `POST …/resources/{rid}`: rotate a repository's token. Tokens are never
    /// stored or echoed, so the resource comes back as it was, re-stamped.
    pub async fn update_resource(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        rid: &str,
    ) -> Result<SessionResource, ApiError> {
        let rid = parse_resource_id(rid)?;
        self.change(ctx, raw_id, |s| {
            let resource = s.resources.iter_mut().find(|r| r.id() == Some(&rid)).ok_or_else(not_found)?;
            match resource {
                SessionResource::GithubRepository { updated_at, .. } => *updated_at = Some(Timestamp::now()),
                _ => {
                    return Err(ApiError::invalid_request(
                        "authorization_token: only github_repository resources take a token",
                    ))
                }
            }
            Ok(resource.clone())
        })
        .await
    }

    /// `DELETE …/resources/{rid}`.
    pub async fn delete_resource(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        rid: &str,
    ) -> Result<SessionResourceId, ApiError> {
        let rid = parse_resource_id(rid)?;
        self.change(ctx, raw_id, |s| {
            let at = s.resources.iter().position(|r| r.id() == Some(&rid)).ok_or_else(not_found)?;
            s.resources.remove(at);
            Ok(rid.clone())
        })
        .await
    }

    /// Load a writable session, apply `f`, and save it.
    async fn change<T>(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        f: impl FnOnce(&mut Session) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let id = parse_id(raw_id)?;
        let mut session = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        let read = session.clone();
        guard_writable(&session)?;
        let out = f(&mut session)?;
        session.updated_at = Timestamp::now();
        self.store.replace(&ctx.workspace, &read, session).await.map_err(map_store)?;
        Ok(out)
    }
}

fn parse_resource_id(raw: &str) -> Result<SessionResourceId, ApiError> {
    SessionResourceId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn not_found() -> ApiError {
    ApiError::not_found("session resource not found")
}
