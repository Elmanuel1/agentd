//! A credential's update, as live rotates one: the body's checks first, then
//! an archived credential is refused, then each sent field replaces its own,
//! and every other field keeps the value it was created with (VA-4, VA-5).

use shared::limits::validate_string_len;
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, Timestamp};

use crate::service::{map_store, parse_pair, VaultsService};
use crate::types::{
    ClientSecretUpdate, Credential, CredentialAuthInput, CredentialAuthUpdate, OauthRefreshInput,
    RefreshUpdate, TokenEndpointAuthInput, UpdateCredentialRequest,
};

impl VaultsService {
    /// `POST /v1/vaults/{vid}/credentials/{cid}`.
    pub async fn update_credential(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        raw_cred_id: &str,
        req: UpdateCredentialRequest,
    ) -> Result<Credential, ApiError> {
        let (vid, cid) = parse_pair(raw_vault_id, raw_cred_id)?;
        if let Some(name) = &req.display_name {
            validate_string_len("display_name", name, 1, usize::MAX)?;
        }
        let mut stored =
            self.credentials.get_credential(&ctx.workspace, &vid, &cid).await.map_err(map_store)?;
        if stored.wire.archived_at.is_some() {
            return Err(ApiError::invalid_request("Credential is archived."));
        }
        if let Some(update) = req.auth {
            let current =
                stored.auth.take().ok_or_else(|| ApiError::internal("the credential has no secret"))?;
            let rotated = rotate(current, update)?;
            stored.wire.auth = rotated.to_wire();
            stored.auth = Some(rotated);
        }
        let metadata = std::mem::take(&mut stored.wire.metadata).merged(req.metadata);
        metadata.validate(MetadataPolicy::capped())?;
        stored.wire.metadata = metadata;
        if let Some(name) = req.display_name {
            stored.wire.display_name = name;
        }
        stored.wire.updated_at = Timestamp::now();
        self.credentials.replace_credential(&ctx.workspace, &vid, stored.clone()).await.map_err(map_store)?;
        Ok(stored.wire)
    }
}

/// `current` with `update`'s fields; the type and the key never change.
fn rotate(
    current: CredentialAuthInput,
    update: CredentialAuthUpdate,
) -> Result<CredentialAuthInput, ApiError> {
    use CredentialAuthInput as Auth;
    use CredentialAuthUpdate as Update;
    match (current, update) {
        (
            Auth::McpOauth { mcp_server_url, access_token, expires_at, refresh },
            Update::McpOauth { access_token: token, expires_at: expiry, refresh: new_refresh },
        ) => {
            let refresh = match (refresh, new_refresh) {
                (refresh, None) => refresh,
                (None, Some(_)) => {
                    return Err(ApiError::invalid_request(
                        "auth.refresh: this credential was created without refresh capability",
                    ))
                }
                (Some(refresh), Some(new)) => Some(rotate_refresh(refresh, new)?),
            };
            Ok(Auth::McpOauth {
                mcp_server_url,
                access_token: token.unwrap_or(access_token),
                expires_at: expiry.or(expires_at),
                refresh,
            })
        }
        (Auth::StaticBearer { mcp_server_url, token }, Update::StaticBearer { token: new }) => {
            Ok(Auth::StaticBearer { mcp_server_url, token: new.unwrap_or(token) })
        }
        (
            Auth::EnvironmentVariable { secret_name, secret_value, networking, injection_location },
            Update::EnvironmentVariable {
                secret_value: value,
                networking: hosts,
                injection_location: places,
            },
        ) => Ok(Auth::EnvironmentVariable {
            secret_name,
            secret_value: value.unwrap_or(secret_value),
            networking: hosts.or(networking),
            injection_location: places.or(injection_location),
        }),
        _ => Err(ApiError::invalid_request("auth.type: does not match this credential's stored type")),
    }
}

fn rotate_refresh(current: OauthRefreshInput, update: RefreshUpdate) -> Result<OauthRefreshInput, ApiError> {
    let token_endpoint_auth = match update.token_endpoint_auth {
        None => current.token_endpoint_auth,
        Some(secret) => Some(rotate_client_secret(current.token_endpoint_auth, secret)?),
    };
    Ok(OauthRefreshInput {
        refresh_token: update.refresh_token.unwrap_or(current.refresh_token),
        scope: update.scope.or(current.scope),
        token_endpoint_auth,
        ..current
    })
}

/// The stored method with the new secret; a method without a secret takes none.
fn rotate_client_secret(
    current: Option<TokenEndpointAuthInput>,
    update: ClientSecretUpdate,
) -> Result<TokenEndpointAuthInput, ApiError> {
    if !matches!(update.method.as_str(), "client_secret_basic" | "client_secret_post") {
        return Err(ApiError::invalid_request(format!(
            "Failed to parse request: params.credential.auth.refresh.token_endpoint_auth.type: \"{}\" is not a valid value",
            update.method
        )));
    }
    match current {
        Some(TokenEndpointAuthInput::ClientSecretBasic { client_secret }) => Ok(TokenEndpointAuthInput::ClientSecretBasic {
            client_secret: update.client_secret.unwrap_or(client_secret),
        }),
        Some(TokenEndpointAuthInput::ClientSecretPost { client_secret }) => Ok(TokenEndpointAuthInput::ClientSecretPost {
            client_secret: update.client_secret.unwrap_or(client_secret),
        }),
        Some(TokenEndpointAuthInput::None) | None => Err(ApiError::invalid_request(
            "auth.refresh.token_endpoint_auth: client_secret does not match this credential's stored auth method",
        )),
    }
}

#[cfg(test)]
mod tests;
