//! [`SecretStore`] on Postgres: each credential's secret sealed in
//! `vault_secrets` under a single-use key per write ([`cipher`]), with the master
//! keys only in agentd's environment. Every operation is one statement on a
//! plain checkout: the table is untracked, so it needs none of `write()`'s
//! writer settings.

pub(crate) mod cipher;
mod reseal;

pub use reseal::Resealed;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::sql_types::{BigInt, Text, Timestamptz};
use diesel::upsert::excluded;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, SelectableHelper};
use diesel_async::RunQueryDsl;
use shared::Timestamp;
use shared::hash::SecretKeys;
use shared::ports::secrets::{Page, SecretError, SecretKey, SecretStore, SecretString, Stored};

use self::cipher::Sealed;
use crate::schema::vault_secrets as t;
use crate::{PgBackend, PooledConnection};

/// Vault secrets in Postgres, sealed under `keys`.
#[derive(Debug, Clone)]
pub struct PgSecretStore {
    backend: PgBackend,
    keys: SecretKeys,
}

fn backend_error(e: impl std::fmt::Display) -> SecretError {
    SecretError::Backend(format!("vault_secrets: {e}"))
}

impl PgSecretStore {
    /// Over `backend`, sealing under `keys`.
    pub fn new(backend: PgBackend, keys: SecretKeys) -> Self {
        Self { backend, keys }
    }

    async fn conn(&self) -> Result<PooledConnection, SecretError> {
        self.backend.diesel.get().await.map_err(backend_error)
    }

    /// `row` opened for `key`; a row that does not open needs a person (a key
    /// left the ring, or the row was altered), so it is logged at `ERROR`.
    fn open(&self, key: &SecretKey, row: &Sealed) -> Result<SecretString, SecretError> {
        cipher::open(&self.keys, key, row).inspect_err(|e| tracing::error!(error = %e, "vault secret"))
    }
}

/// A listing's page size.
const PAGE: i64 = 1000;

#[derive(diesel::QueryableByName)]
struct Listed {
    #[diesel(sql_type = Text)]
    workspace_id: String,
    #[diesel(sql_type = Text)]
    vault_id: String,
    #[diesel(sql_type = Text)]
    credential_id: String,
    #[diesel(sql_type = Timestamptz)]
    written_at: DateTime<Utc>,
}

#[async_trait]
impl SecretStore for PgSecretStore {
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError> {
        let sealed = cipher::seal(&self.keys, key, secret)?;
        let mut conn = self.conn().await?;
        diesel::insert_into(t::table)
            .values((
                t::workspace_id.eq(key.workspace()),
                t::vault_id.eq(key.vault()),
                t::credential_id.eq(key.credential()),
                t::environment_variable.eq(sealed.environment_variable),
                t::key_id.eq(&sealed.key_id),
                t::salt.eq(&sealed.salt),
                t::sealed_value.eq(&sealed.sealed_value),
            ))
            .on_conflict((t::workspace_id, t::vault_id, t::credential_id))
            .do_update()
            .set((
                t::environment_variable.eq(excluded(t::environment_variable)),
                t::key_id.eq(excluded(t::key_id)),
                t::salt.eq(excluded(t::salt)),
                t::sealed_value.eq(excluded(t::sealed_value)),
                t::written_at.eq(excluded(t::written_at)),
            ))
            .execute(&mut conn)
            .await
            .map_err(backend_error)?;
        Ok(())
    }

    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        let mut conn = self.conn().await?;
        let row = t::table
            .find((key.workspace(), key.vault(), key.credential()))
            .select(Sealed::as_select())
            .first(&mut conn)
            .await
            .optional()
            .map_err(backend_error)?;
        let row = row.ok_or_else(|| SecretError::NotFound(key.to_string()))?;
        self.open(key, &row)
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        let mut conn = self.conn().await?;
        diesel::delete(t::table.find((key.workspace(), key.vault(), key.credential())))
            .execute(&mut conn)
            .await
            .map_err(backend_error)?;
        Ok(())
    }

    async fn list(&self, after: Option<String>) -> Result<Page<SecretKey>, SecretError> {
        let after = after.unwrap_or_else(|| "//".to_owned());
        let Some([workspace, vault, credential]) = after.splitn(3, '/').collect::<Vec<_>>().try_into().ok()
        else {
            return Err(SecretError::InvalidKey("a page starts after workspace/vault/credential".into()));
        };
        let mut conn = self.conn().await?;
        let rows: Vec<Listed> = diesel::sql_query(
            "SELECT workspace_id, vault_id, credential_id, written_at FROM vault_secrets
             WHERE (workspace_id, vault_id, credential_id) > ($1, $2, $3)
             ORDER BY workspace_id, vault_id, credential_id LIMIT $4",
        )
        .bind::<Text, _>(workspace)
        .bind::<Text, _>(vault)
        .bind::<Text, _>(credential)
        .bind::<BigInt, _>(PAGE)
        .load(&mut conn)
        .await
        .map_err(backend_error)?;
        let full = rows.len() == usize::try_from(PAGE).unwrap_or(usize::MAX);
        let next = rows
            .last()
            .filter(|_| full)
            .map(|r| format!("{}/{}/{}", r.workspace_id, r.vault_id, r.credential_id));
        let items = rows
            .into_iter()
            .filter_map(|r| {
                let key = SecretKey::new(&r.workspace_id, &r.vault_id, &r.credential_id).ok()?;
                Some(Stored { key, written_at: Timestamp::from_datetime(r.written_at) })
            })
            .collect();
        Ok(Page { items, next })
    }
}
