//! A credential's update: which fields rotate, and the rules that keep the
//! rest as created.

use shared::{ApiError, AuthContext, Timestamp};

use crate::credentials::validate_auth;
use crate::service::{map_store, parse_pair, VaultsService};
use crate::types::{Credential, CredentialAuthInput, UpdateCredentialRequest};

impl VaultsService {
    /// `POST /v1/vaults/{vid}/credentials/{cid}` — rotation. The variant and
    /// unique key are immutable.
    pub async fn update_credential(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        raw_cred_id: &str,
        req: UpdateCredentialRequest,
    ) -> Result<Credential, ApiError> {
        let (vid, cid) = parse_pair(raw_vault_id, raw_cred_id)?;
        let mut stored =
            self.credentials.get_credential(&ctx.workspace, &vid, &cid).await.map_err(map_store)?;
        if stored.wire.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot update an archived credential"));
        }
        if let Some(new_auth) = req.auth {
            validate_auth(&new_auth)?;
            let old_key = stored.auth.as_ref().map(|a| a.unique_key().to_owned()).unwrap_or_default();
            if new_auth.unique_key() != old_key {
                return Err(ApiError::conflict_invalid_request(format!(
                    "credential keys are immutable: cannot change `{old_key}` to `{}`; archive and create a new credential",
                    new_auth.unique_key()
                )));
            }
            let same_variant = matches!(
                (&stored.auth, &new_auth),
                (Some(CredentialAuthInput::McpOauth { .. }), CredentialAuthInput::McpOauth { .. })
                    | (
                        Some(CredentialAuthInput::StaticBearer { .. }),
                        CredentialAuthInput::StaticBearer { .. }
                    )
                    | (
                        Some(CredentialAuthInput::EnvironmentVariable { .. }),
                        CredentialAuthInput::EnvironmentVariable { .. }
                    )
            );
            if !same_variant {
                return Err(ApiError::conflict_invalid_request(
                    "credential type is immutable; archive and create a new credential",
                ));
            }
            stored.wire.auth = new_auth.to_wire();
            stored.auth = Some(new_auth);
        }
        if let Some(n) = req.display_name {
            stored.wire.display_name = n;
        }
        stored.wire.updated_at = Timestamp::now();
        self.credentials.replace_credential(&ctx.workspace, &vid, stored.clone()).await.map_err(map_store)?;
        Ok(stored.wire)
    }
}
