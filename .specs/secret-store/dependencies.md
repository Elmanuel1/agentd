# Secret store: dependencies

Each fact the design rests on, with its evidence. `VERIFIED` rows were read in the code or in the vendor's
documentation; `ASSUMED` rows name what reopens them.

## agentd today

| # | Fact | Status | Evidence |
|---|---|---|---|
| A1 | Every vault call site reaches secrets through the `SecretStore` port (`put`, `get`, `delete` by `SecretKey`); the secret is put before the credential row is written, and an orphan is cleaned up on failure. | VERIFIED | `shared/src/ports/secrets.rs`; `store-postgres/src/entities/vault_credentials/write.rs` |
| A2 | `read_secret` asks with a plain key; only `put` marks a key `environment_variable`. | VERIFIED | `store-postgres/src/entities/vaults/secret.rs` |
| A3 | `DieselPgPool::write()` runs a `set_config` statement before the write, for the history actor; `get()` runs none. Recycling is `Fast`, so a checkout runs no test query. | VERIFIED | `store-postgres/src/checkout.rs`, `pool.rs` |
| A4 | History tracks a table only through its `<table>_history_stamp` trigger and Debezium's `table.include.list`. | VERIFIED | migrations V16, V17; `docker/debezium/application.properties` |
| A5 | Migrations end at V17 on `main`, and the open durable-scheduler PR takes V18 and V19. | VERIFIED | `store-postgres/migrations`; Elmanuel1/agentd PR 69 |
| A6 | The loop reads at most one secret per MCP server per step. | VERIFIED | runtime-parity D27; `engine/src/mcp/servers.rs` |

## Cryptography

| # | Fact | Status | Evidence |
|---|---|---|---|
| B1 | AES-GCM with random 96-bit nonces allows at most 2^32 encryptions under one key. | VERIFIED | NIST SP 800-38D, section 8.3 |
| B2 | `aws-lc-rs` 1.19 offers HKDF-SHA256 (`hkdf::Salt::extract`, `Prk::expand`, `Okm::fill`) and `aead::RandomizedNonceKey` with AES-256-GCM, whose seal returns the random nonce it used. It is already in `Cargo.lock` through rustls, so the dependency adds no crate. | VERIFIED | `aws-lc-rs` 1.19.0 source, `hkdf.rs`, `aead/rand_nonce.rs`; `Cargo.lock` |
| B3 | Kubernetes KMS v2 derives a single-use data encryption key per write from a seed with a KDF to avoid the AES-GCM nonce limit; its local `aesgcm` provider must rotate every 200,000 writes. | VERIFIED | kubernetes.io, "Encrypting Confidential Data at Rest" |
| B4 | Grafana, Coder (`dbcrypt rotate`) and Airflow (`rotate-fernet-key`) keep secrets encrypted in their own database with a key list from configuration and an operator command that re-encrypts. | VERIFIED | each project's documentation |

## SSM Parameter Store

| # | Fact | Status | Evidence |
|---|---|---|---|
| C1 | A SecureString parameter is encrypted under the KMS key `KeyId` names; an overwrite without `KeyId` falls back to the AWS managed key `aws/ssm`. | VERIFIED | AWS Systems Manager API Reference, `PutParameter` |
| C2 | `PutParameter` takes tags only on create; with tags and `Overwrite` true it fails. A name beginning with `aws` or `ssm` in any case is reserved, and a name has at most 15 levels. | VERIFIED | AWS Systems Manager API Reference, `PutParameter`; "Working with parameter hierarchies" |
| C3 | Intelligent-Tiering stores a parameter as standard unless it needs advanced (a value over 4 KB); advanced holds 8 KB. | VERIFIED | AWS Systems Manager User Guide, "Managing parameter tiers" |
| C4 | `GetParameter` is 40 TPS per account and region by default, 10,000 with higher throughput; `PutParameter` is 3 TPS by default and 10 with higher throughput. | VERIFIED | AWS General Reference, Systems Manager endpoints and quotas |
| C5 | Parameter Store keeps a parameter's 100 most recent versions; `DeleteParameter` removes them all. | VERIFIED | AWS Systems Manager User Guide, "Working with parameter versions" |

## Load

| # | Fact | Status | Evidence |
|---|---|---|---|
| N1 | A sealed OAuth credential is about 1 KB; 10,000 credentials refreshed hourly is 2.8 puts a second. | ASSUMED | the fixtures; reopened by a load replay |
| N2 | At 10,000 concurrent sessions with one MCP step every 5 s the loop reads about 2,000 secrets a second, each one index lookup and two AES-GCM opens of under 2 KB. | ASSUMED | A6 and runtime-parity's sizing peak; reopened by the same replay |
