//! The record store setting: Postgres, the only one there is.

use store_postgres::{PoolSettings, redacted};

use super::{ConfigError, number, selector, unknown};

/// `AGENTD_STORE`.
#[derive(Clone, PartialEq, Eq)]
pub enum StoreConfig {
    /// `postgres`, at `DATABASE_URL`.
    Postgres {
        /// The connection URL.
        url: String,
        /// `AGENTD_PG_READ_URL`: a read replica the events list reads, when set.
        read_url: Option<String>,
        /// `AGENTD_DB_POOL_SIZE` connections (default 16, S-8), each caller waiting
        /// at most `AGENTD_DB_POOL_WAIT_MS` (default 5000) for one, each
        /// statement running at most `AGENTD_DB_STATEMENT_TIMEOUT_MS` (default 30000).
        pool: PoolSettings,
        /// `AGENTD_PG_AUTH`: how each connection, to both URLs, logs in.
        auth: PgAuth,
    },
}

/// `AGENTD_PG_AUTH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgAuth {
    /// `password` (the default): the password in the URL, if any.
    Password,
    /// `aws_iam`: an RDS IAM token for each login, signed with the AWS
    /// credentials of the default chain.
    AwsIam,
}

/// Shows each connection string without its password (and a URL without
/// its query, where libpq also takes one).
impl std::fmt::Debug for StoreConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::Postgres { url, read_url, pool, auth } = self;
        f.debug_struct("Postgres")
            .field("url", &redacted(url))
            .field("read_url", &read_url.as_deref().map(redacted))
            .field("pool", pool)
            .field("auth", auth)
            .finish()
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
                read_url: get("AGENTD_PG_READ_URL").filter(|u| !u.is_empty()),
                pool: PoolSettings {
                    max_size: number(get, "AGENTD_DB_POOL_SIZE", 16)?,
                    wait: std::time::Duration::from_millis(number(get, "AGENTD_DB_POOL_WAIT_MS", 5000)?),
                    statement_timeout: std::time::Duration::from_millis(number(
                        get,
                        "AGENTD_DB_STATEMENT_TIMEOUT_MS",
                        30_000,
                    )?),
                },
                auth: match selector(get, "AGENTD_PG_AUTH", "password").as_str() {
                    "password" => PgAuth::Password,
                    "aws_iam" => PgAuth::AwsIam,
                    other => {
                        return Err(ConfigError {
                            var: "AGENTD_PG_AUTH",
                            reason: format!("unknown method `{other}`: use password or aws_iam"),
                        });
                    }
                },
            }),
            other => Err(unknown("AGENTD_STORE", other)),
        }
    }
}
