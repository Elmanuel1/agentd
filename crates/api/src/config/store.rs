//! The record store setting: Postgres, the only one there is.

use store_postgres::PoolSettings;

use super::{number, selector, unknown, ConfigError};

/// `AGENTD_STORE`.
#[derive(Clone, PartialEq, Eq)]
pub enum StoreConfig {
    /// `postgres`, at `DATABASE_URL`.
    Postgres {
        /// The connection URL.
        url: String,
        /// `AGENTD_DB_POOL_SIZE` connections (default 10), each caller waiting
        /// at most `AGENTD_DB_POOL_WAIT_MS` (default 5000) for one.
        pool: PoolSettings,
    },
}

/// Shows the URL without its password or its query, where libpq also takes one.
impl std::fmt::Debug for StoreConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::Postgres { url, pool } = self;
        let url = url.split('?').next().unwrap_or_default();
        let shown = match url.rsplit_once('@') {
            Some((credentials, rest)) => {
                let user = credentials.split_once(':').map_or(credentials, |(user, _)| user);
                format!("{user}:***@{rest}")
            }
            None => url.to_owned(),
        };
        f.debug_struct("Postgres").field("url", &shown).field("pool", pool).finish()
    }
}

impl StoreConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        match selector(get, "AGENTD_STORE", "postgres").as_str() {
            "postgres" => Ok(Self::Postgres {
                url: get("DATABASE_URL").filter(|u| !u.is_empty()).ok_or_else(|| ConfigError {
                    var: "DATABASE_URL",
                    reason: "required: the Postgres connection URL".into(),
                })?,
                pool: PoolSettings {
                    max_size: number(get, "AGENTD_DB_POOL_SIZE", 10)?,
                    wait: std::time::Duration::from_millis(number(get, "AGENTD_DB_POOL_WAIT_MS", 5000)?),
                },
            }),
            other => Err(unknown("AGENTD_STORE", other)),
        }
    }
}
