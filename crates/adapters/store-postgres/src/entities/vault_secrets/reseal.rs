//! `agentd vault-secrets reseal`: every row not under the master key ring's
//! first key, sealed again under the first key with a new salt.

use diesel::sql_types::{BigInt, Text};
use diesel::{ExpressionMethods, QueryDsl, QueryableByName};
use diesel_async::RunQueryDsl;
use shared::ports::secrets::{SecretError, SecretKey};

use super::cipher::{self, Sealed};
use super::{backend_error, PgSecretStore};
use crate::schema::vault_secrets as t;
use crate::PooledConnection;

/// Rows a reseal reads at a time.
const PAGE: i64 = 500;

/// What a reseal did: the rows it moved to the first key, and each row still
/// under another key, as `(workspace/vault/credential, key id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resealed {
    pub moved: usize,
    pub remaining: Vec<(String, String)>,
}

#[derive(QueryableByName)]
#[diesel(table_name = t)]
struct Row {
    workspace_id: String,
    vault_id: String,
    credential_id: String,
    #[diesel(embed)]
    sealed: Sealed,
}

impl PgSecretStore {
    /// Seal every row not under the first key again, under the first key with
    /// a new salt. A put between a row's read and its write wins: the write
    /// matches the sealed key it read. Rows that do not open stay, and are named.
    pub async fn reseal(&self) -> Result<Resealed, SecretError> {
        let first = self.keys.first_id().to_owned();
        let mut conn = self.conn().await?;
        let (mut moved, mut after) = (0, [String::new(), String::new(), String::new()]);
        loop {
            let page: Vec<Row> = diesel::sql_query(
                "SELECT workspace_id, vault_id, credential_id, environment_variable, key_id, salt,
                        sealed_value
                 FROM vault_secrets
                 WHERE key_id <> $1 AND (workspace_id, vault_id, credential_id) > ($2, $3, $4)
                 ORDER BY workspace_id, vault_id, credential_id LIMIT $5",
            )
            .bind::<Text, _>(&first)
            .bind::<Text, _>(&after[0])
            .bind::<Text, _>(&after[1])
            .bind::<Text, _>(&after[2])
            .bind::<BigInt, _>(PAGE)
            .load(&mut conn)
            .await
            .map_err(backend_error)?;
            let full = page.len() == PAGE as usize;
            if let Some(last) = page.last() {
                after = [last.workspace_id.clone(), last.vault_id.clone(), last.credential_id.clone()];
            }
            for row in page {
                moved += self.reseal_row(&mut conn, row).await?;
            }
            if !full {
                break;
            }
        }
        let left: Vec<(String, String, String, String)> = t::table
            .filter(t::key_id.ne(&first))
            .select((t::workspace_id, t::vault_id, t::credential_id, t::key_id))
            .order((t::workspace_id, t::vault_id, t::credential_id))
            .load(&mut conn)
            .await
            .map_err(backend_error)?;
        let remaining = left.into_iter().map(|(ws, vault, cred, id)| (format!("{ws}/{vault}/{cred}"), id));
        Ok(Resealed { moved, remaining: remaining.collect() })
    }

    /// 1 when `row` moved to the first key; 0 when its ids are not a key, it
    /// does not open, or a put won.
    async fn reseal_row(&self, conn: &mut PooledConnection, row: Row) -> Result<usize, SecretError> {
        let Ok(key) = SecretKey::new(&row.workspace_id, &row.vault_id, &row.credential_id) else {
            return Ok(0);
        };
        let read = row.sealed;
        let key = match read.environment_variable {
            true => key.environment_variable(),
            false => key,
        };
        let Ok(secret) = self.open(&key, &read) else { return Ok(0) };
        let next = cipher::seal(&self.keys, &key, &secret)?;
        diesel::update(t::table.find((key.workspace(), key.vault(), key.credential())))
            .filter(t::key_id.eq(&read.key_id))
            .filter(t::salt.eq(&read.salt))
            .set((t::key_id.eq(&next.key_id), t::salt.eq(&next.salt), t::sealed_value.eq(&next.sealed_value)))
            .execute(conn)
            .await
            .map_err(backend_error)
    }
}
