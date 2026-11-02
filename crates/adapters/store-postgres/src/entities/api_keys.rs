//! API keys on Postgres: the [`KeySource`] every request resolves through,
//! and the create, list and revoke the `agentd keys` command runs. Only a
//! peppered digest of each key is stored ([`material`]); the key is returned
//! once, by [`PgApiKeys::create`]. Nothing is cached, so a revoke holds from
//! the next request.

mod material;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, SelectableHelper};
use diesel_async::RunQueryDsl;
use shared::charset::is_id_charset;
use shared::ports::keys::{KeyError, KeyPepper, KeySource};
use shared::ports::secrets::{ExposeSecret, SecretString};
use shared::{AuthContext, Id, OrgId, Resource, WorkspaceId};

use crate::schema::api_keys;
use crate::PgBackend;

/// Marker for `apikey_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct ApiKeyResource;
impl Resource for ApiKeyResource {
    const PREFIX: &'static str = "apikey";
    const LABEL: &'static str = "API key";
}

/// One API key, without its secret.
#[derive(Debug, Clone, PartialEq, Eq, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = api_keys)]
pub struct ApiKeyRecord {
    /// `apikey_…`.
    #[diesel(column_name = api_key_id)]
    pub id: String,
    /// The organization it acts for.
    #[diesel(column_name = org_id)]
    pub org: String,
    /// The workspace it acts in.
    #[diesel(column_name = workspace_id)]
    pub workspace: String,
    /// A label for people.
    pub name: String,
    /// Its first 16 and last 4 characters.
    pub partial_key_hint: String,
    /// When it was created.
    pub created_at: DateTime<Utc>,
    /// When it was revoked; it resolves to no one from then on.
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = api_keys)]
struct NewApiKey<'a> {
    api_key_id: &'a str,
    org_id: &'a str,
    workspace_id: &'a str,
    name: &'a str,
    key_digest: &'a [u8],
    partial_key_hint: &'a str,
    created_at: DateTime<Utc>,
}

/// API keys in Postgres, digested with one pepper.
#[derive(Clone)]
pub struct PgApiKeys {
    backend: PgBackend,
    pepper: Arc<KeyPepper>,
}

impl std::fmt::Debug for PgApiKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgApiKeys").finish_non_exhaustive()
    }
}

fn store_error(e: impl std::fmt::Display) -> KeyError {
    KeyError(e.to_string())
}

impl PgApiKeys {
    /// Keys in `backend`, digested with `pepper`.
    pub fn new(backend: PgBackend, pepper: Arc<KeyPepper>) -> Self {
        Self { backend, pepper }
    }

    /// A new key for `org` and `workspace`, both in the id charset. The key is
    /// in the return value only: it is not stored and cannot be read again.
    pub async fn create(
        &self,
        org: &str,
        workspace: &str,
        name: &str,
    ) -> Result<(ApiKeyRecord, SecretString), KeyError> {
        if !is_id_charset(org) || !is_id_charset(workspace) {
            return Err(KeyError("the org and workspace ids must be in the id charset".into()));
        }
        if name.is_empty() || name.chars().count() > 255 || name.chars().any(char::is_control) {
            return Err(KeyError(
                "the name must be 1 to 255 characters, none of them control characters".into(),
            ));
        }
        let key = material::generate();
        let digest = material::digest(&self.pepper, key.expose_secret())
            .ok_or_else(|| KeyError("the pepper cannot key a digest".into()))?;
        let id = Id::<ApiKeyResource>::generate();
        let row = NewApiKey {
            api_key_id: id.as_str(),
            org_id: org,
            workspace_id: workspace,
            name,
            key_digest: &digest,
            partial_key_hint: &material::hint(key.expose_secret()),
            created_at: Utc::now(),
        };
        let mut conn = self.backend.diesel.get().await.map_err(store_error)?;
        let record = diesel::insert_into(api_keys::table)
            .values(&row)
            .returning(ApiKeyRecord::as_returning())
            .get_result(&mut conn)
            .await
            .map_err(store_error)?;
        Ok((record, key))
    }

    /// Every key, oldest first.
    pub async fn list(&self) -> Result<Vec<ApiKeyRecord>, KeyError> {
        let mut conn = self.backend.diesel.get().await.map_err(store_error)?;
        api_keys::table
            .order(api_keys::created_at.asc())
            .select(ApiKeyRecord::as_select())
            .load(&mut conn)
            .await
            .map_err(store_error)
    }

    /// Revoke `id`; `false` when no active key has it.
    pub async fn revoke(&self, id: &str) -> Result<bool, KeyError> {
        let mut conn = self.backend.diesel.get().await.map_err(store_error)?;
        let revoked = diesel::update(
            api_keys::table.filter(api_keys::api_key_id.eq(id)).filter(api_keys::revoked_at.is_null()),
        )
        .set(api_keys::revoked_at.eq(Utc::now()))
        .execute(&mut conn)
        .await
        .map_err(store_error)?;
        Ok(revoked == 1)
    }
}

#[async_trait]
impl KeySource for PgApiKeys {
    async fn resolve(&self, key: &str) -> Result<Option<AuthContext>, KeyError> {
        if !material::well_formed(key) {
            return Ok(None);
        }
        let Some(digest) = material::digest(&self.pepper, key) else {
            return Ok(None);
        };
        let mut conn = self.backend.diesel.get().await.map_err(store_error)?;
        let row: Option<(String, String, String, Vec<u8>)> = api_keys::table
            .filter(api_keys::key_digest.eq(&digest))
            .filter(api_keys::revoked_at.is_null())
            .select((api_keys::org_id, api_keys::workspace_id, api_keys::api_key_id, api_keys::key_digest))
            .first(&mut conn)
            .await
            .optional()
            .map_err(store_error)?;
        // The index found the row; the constant-time check decides it.
        Ok(row.filter(|(.., stored)| material::matches(&self.pepper, key, stored)).map(
            |(org, workspace, api_key_id, _)| AuthContext {
                org: OrgId::new(org),
                workspace: WorkspaceId::new(workspace),
                api_key_id,
            },
        ))
    }
}
