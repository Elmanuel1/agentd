//! [`SecretStore`] on Postgres: each credential's secret sealed in
//! `vault_secrets` under envelope encryption ([`envelope`]), with the master
//! keys only in agentd's environment. Every operation is one statement on a
//! plain checkout: the table is untracked, so it needs none of `write()`'s
//! writer settings.

pub(crate) mod envelope;
mod reseal;

pub use reseal::Resealed;

use async_trait::async_trait;
use diesel::upsert::excluded;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, SelectableHelper};
use diesel_async::RunQueryDsl;
use shared::hash::SecretKeys;
use shared::ports::secrets::{SecretError, SecretKey, SecretStore, SecretString};

use self::envelope::Sealed;
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
        envelope::open(&self.keys, key, row).inspect_err(|e| tracing::error!(error = %e, "vault secret"))
    }
}

#[async_trait]
impl SecretStore for PgSecretStore {
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError> {
        let sealed = envelope::seal(&self.keys, key, secret)?;
        let mut conn = self.conn().await?;
        diesel::insert_into(t::table)
            .values((
                t::workspace_id.eq(key.workspace()),
                t::vault_id.eq(key.vault()),
                t::credential_id.eq(key.credential()),
                t::environment_variable.eq(sealed.environment_variable),
                t::key_id.eq(&sealed.key_id),
                t::sealed_key.eq(&sealed.sealed_key),
                t::sealed_value.eq(&sealed.sealed_value),
            ))
            .on_conflict((t::workspace_id, t::vault_id, t::credential_id))
            .do_update()
            .set((
                t::environment_variable.eq(excluded(t::environment_variable)),
                t::key_id.eq(excluded(t::key_id)),
                t::sealed_key.eq(excluded(t::sealed_key)),
                t::sealed_value.eq(excluded(t::sealed_value)),
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
}
