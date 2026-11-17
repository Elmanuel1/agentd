# Secret store: decisions

agentd keeps each vault credential's secret behind its `SecretStore` port. Postgres is the default store, sealed by
envelope encryption under master keys from the environment, so a small deployment runs on Postgres alone. AWS
Secrets Manager and SSM Parameter Store are the external adapters, chosen by a setting
([ADR 0001](adr/0001-postgres-secret-store.md)). Each row names who decided and when.

## Which store

| # | Decision | Source |
|---|---|---|
| D1 | `AGENTD_SECRETS` picks the store: `postgres`, `aws_sm` (Secrets Manager) or `aws_ssm` (Parameter Store), named as iron-proxy names its sources (dependencies C6); `aws` also selects the Secrets Manager store. Unset, it is `postgres`, unless `AGENTD_SECRETS_KMS_KEY_ID` is set, which picks `aws_sm`; an empty or blank value counts as unset. An AWS deployment keeps working with no change and moves to Postgres by dropping the KMS variable. | User, 2026-11-16 ("kms already infer aws. its not neutral"); the names, user, 2026-11-17 ("AGENTD_SECRETS=aws_sm, aws_ssm etc.") |
| D2 | The Postgres store is the default and has no KMS plugin. The external-KMS path is an AWS adapter, chosen whole. | User, 2026-11-16 (default, then external backend) |
| D3 | The Secrets Manager adapter keeps its calls, names, tags and recovery window. | User, 2026-11-17 (plan approved) |
| D4 | An SSM Parameter Store adapter keeps each credential as one SecureString parameter under the customer KMS key, in the Intelligent-Tiering tier, tagged as the Secrets Manager adapter tags. | User, 2026-11-17 ("no openbao for now. and ssm.") |
| D5 | OpenBao is out for now. | User, 2026-11-17 ("no openbao for now. and ssm.") |
| D6 | Our `cloud` runtime swaps `environment_variable` values at egress from Secrets Manager or SSM through iron-proxy, never from Postgres. A deployment on the Postgres store runs no egress swap. | User, 2026-11-17 ("this cloud is ours") |

## How Postgres seals

| # | Decision | Source |
|---|---|---|
| D7 | One table, `vault_secrets` (V20), keyed by workspace, vault and credential, with the kind, the wrapping key's id, the sealed data key and the sealed value. No timestamps, no writer columns, no foreign key: the secret is written before its credential row, as the port requires of every store. | User, 2026-11-17 ("Approve", the columns included) |
| D8 | Each put seals the value with AES-256-GCM under a fresh random 256-bit data key, and wraps the data key with AES-256-GCM under a single-use key that HKDF-SHA256 derives from the first master key and a fresh 32-byte salt. No key encrypts twice, so the AES-GCM nonce limit never applies. | User, 2026-11-17 (plan approved) |
| D9 | Both ciphertexts are bound to their workspace, vault, credential and kind through the associated data. The kind is read from the row, never from the caller's key. | User, 2026-11-17 (plan approved) |
| D10 | The master keys are `AGENTD_SECRET_KEYS`, a key ring `<id>:<hex>,…` of 32-byte keys: the first wraps, every one opens. No key leaves `shared::hash::SecretKeys`, which derives wrap keys only. A bad entry is named by its position, never its text, in every key ring. | User, 2026-11-17 (plan approved) |
| D11 | `vault_secrets` stays off the change feed and history: it is not in Debezium's include list and has no history trigger. | User, 2026-11-16 (the plan brief) |
| D12 | `agentd vault-secrets reseal` seals every row not under the first key again under a new data key and the first key, names what remains, and fails while any row remains. It re-seals in full rather than rewrapping, so an old backup plus a leaked old key never opens the current value. | User, 2026-11-16 ("reseal") |
| D13 | No read cache: a Postgres read is one primary-key lookup, and the loop reads once per server per step (runtime-parity D27). | User, 2026-11-17 (plan approved) |
| D14 | The secret store ships as one agentd PR of seven ordered commits, under 100 files, the spec included. | User, 2026-11-16 (one PR of ordered commits) |

Out of scope: [requirements.md](requirements.md#out-of-scope).
