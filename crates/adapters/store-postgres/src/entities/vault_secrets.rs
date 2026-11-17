//! [`SecretStore`](shared::ports::secrets::SecretStore) on Postgres: each
//! credential's secret sealed in `vault_secrets` under envelope encryption
//! ([`envelope`]), with the master keys only in agentd's environment.

pub(crate) mod envelope;
