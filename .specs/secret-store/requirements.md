# Requirements: secret store

**Date:** 2026-11-17
**Format:** EARS (Easy Approach to Requirements Syntax). Each requirement uses one pattern:

| Pattern | Template |
|---|---|
| Ubiquitous | The `<system>` shall `<response>`. |
| Event-driven | When `<trigger>`, the `<system>` shall `<response>`. |
| State-driven | While `<state>`, the `<system>` shall `<response>`. |
| Unwanted behaviour | If `<condition>`, then the `<system>` shall `<response>`. |

**Systems:** *agentd* (the one binary and its CLI), *the Postgres store* (`PgSecretStore` over `vault_secrets`),
*the SSM store* (`AwsParameterStore`), *the change feed* and *the history consumer* (as in resource history).

**Terms:**
- **Master key ring:** `AGENTD_SECRET_KEYS`, `<id>:<hex>,…`, each key 32 bytes; the first seals, every one opens.
- **Value key:** the single-use key HKDF-SHA256 derives from a master key and a row's 32-byte salt, which seals one value.
- **Kind:** `env` for an `environment_variable` credential, `mcp` for every other.

**Build order:** spec → V20 and V21 `vault_secrets` (SS-9) → the cipher (SS-3 to SS-5, SS-7) → the store and reseal (SS-6,
SS-8, SS-13) → selection and the CLI (SS-1, SS-2, SS-10 to SS-12, SS-14) → the SSM adapter (SS-15 to SS-18) → docs,
stacks and CI, one PR (D14).

Source decisions: [decisions.md](decisions.md). Evidence: [dependencies.md](dependencies.md). The decision records:
[ADR 0001](adr/0001-postgres-secret-store.md) and [ADR 0002](adr/0002-salt-only-vault-secrets.md).

## Selection

| ID | Requirement |
|---|---|
| SS-1 | agentd shall take its secret store from `AGENTD_SECRETS` (`postgres`, `aws_sm` or `aws_ssm`, the names iron-proxy gives its sources (dependencies C6); `aws` also selects the Secrets Manager store), and shall use `postgres` when the variable is unset and SS-12 does not apply. |
| SS-2 | When `AGENTD_SECRETS` is `postgres`, agentd shall refuse to start unless `AGENTD_SECRET_KEYS` holds at least one `<id>:<hex>` key of exactly 32 bytes, naming the variable and the bad entry's position, never its text. |
| SS-10 | When `AGENTD_SECRETS` is `aws_sm`, agentd shall keep each credential as one Secrets Manager secret `<prefix>/<workspace>/<vault>/<credential>`, created under the customer-managed KMS key `AGENTD_SECRETS_KMS_KEY_ID` and tagged `workspace`, `vault` and `managed-by=agentd`, restore and write again a secret deleted inside its recovery window, and delete with the recovery window `AGENTD_SECRETS_RECOVERY_DAYS`. |
| SS-12 | While `AGENTD_SECRETS` is unset, agentd shall use the `aws_sm` store when `AGENTD_SECRETS_KMS_KEY_ID` is set and the `postgres` store otherwise; a set `AGENTD_SECRETS` wins, and an empty or blank value of either counts as unset. |

## The Postgres store

| ID | Requirement |
|---|---|
| SS-3 | When the Postgres store puts a secret, it shall encrypt the value with AES-256-GCM under a value key derived from the ring's first key and a new random 32-byte salt, and store the salt and the key's id with it, in one statement. |
| SS-4 | The Postgres store shall authenticate the ciphertext with associated data naming the workspace, vault, credential and kind, so that a row moved to another key or kind does not open. |
| SS-5 | When the Postgres store reads a secret, it shall open it with the ring key its row names, and shall return a backend error naming the credential key and key id when that key is absent or a tag fails. |
| SS-6 | When no row exists, the Postgres store shall return `NotFound` on read and succeed on delete. |
| SS-7 | The Postgres store shall never write a master key to the database, a log, an error or a metric, and shall zero every value key and plaintext buffer it allocates. |
| SS-8 | The Postgres store shall issue exactly one SQL statement for each put, get and delete, on the primary. |
| SS-9 | The change feed and the history consumer shall not capture `vault_secrets`. |

## Operations

| ID | Requirement |
|---|---|
| SS-11 | When `AGENTD_SECRETS` is `postgres` or `aws_ssm`, `agentd vault-secrets tag` shall change nothing and exit successfully, saying the store records the kind when it writes. |
| SS-13 | `agentd vault-secrets reseal` shall seal every row not under the ring's first key again under the first key with a new salt, print how many rows it moved and how many remain under another key, naming each remaining row by its ids and key id, and exit with failure while any remain. |
| SS-14 | When `AGENTD_SECRETS` is `aws_sm` or `aws_ssm`, `agentd vault-secrets reseal` shall change nothing and exit successfully, saying the store's KMS key does the encryption. |

## The SSM store

| ID | Requirement |
|---|---|
| SS-15 | When `AGENTD_SECRETS` is `aws_ssm`, agentd shall keep each credential as one SecureString parameter `/<prefix>/<workspace>/<vault>/<credential>`, encrypted under the customer-managed KMS key `AGENTD_SECRETS_KMS_KEY_ID` (checked at boot as the `aws_sm` store checks it), in the Intelligent-Tiering tier. |
| SS-16 | When the SSM store puts a new parameter, it shall create it with the tags the `aws_sm` store writes (`credential_type=environment_variable` included); when the parameter exists, it shall overwrite its value with the same `Type`, `KeyId` and `Tier` and then add the create's tags to it, and when tagging fails, it shall return the error and keep the value. |
| SS-17 | The SSM store shall read with decryption, return `NotFound` for a missing parameter, treat deleting a missing one as success, and name only the parameter and the AWS error code in an error. |
| SS-18 | When `AGENTD_SECRETS` is `aws_ssm`, agentd shall refuse to start when `AGENTD_SECRETS_RECOVERY_DAYS` is set or the prefix starts with `aws` or `ssm` in any letter case, or has more than 12 segments (SSM allows 15 levels; the key adds 3). |

## Statements per operation

| Operation | Statements |
|---|---|
| put | 1, on a plain checkout: an `INSERT ... ON CONFLICT (workspace_id, vault_id, credential_id) DO UPDATE` of the kind, key id, salt and sealed value |
| get | 1: a primary-key `SELECT` of the kind, key id, salt and sealed value, on the primary so a refresh reads its own write |
| delete | 1, on a plain checkout: a primary-key `DELETE`; 0 rows is success |
| reseal | pages of 500 rows not under the first key, by primary key, 1 `SELECT` each; then 1 guarded `UPDATE` per row (matching the key id and salt it read, so a put in between wins); then 1 `SELECT` of the rows still under another key |

## Rollout

- agentd is not deployed (runtime-parity D45): the PR merges on green CI and a local run against Postgres 16 and
  LocalStack with `ssm`.
- A deployment that sets `AGENTD_SECRETS_KMS_KEY_ID` stays on Secrets Manager with no change (SS-12). A new
  deployment sets `AGENTD_SECRET_KEYS` and runs V20 and V21.
- Master key rotation: add the new key second on every replica, move it first, run `agentd vault-secrets reseal`
  until it succeeds, then drop the old key (`docs/configuration.md`).

## Out of scope

- Moving existing secrets between stores.
- A KMS plugin for the Postgres store (D2), OpenBao (D5), and an egress swap that reads Postgres (D6).
- A read cache (D13).
