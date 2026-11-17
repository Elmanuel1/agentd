# ADR 0001: Vault secrets default to Postgres, sealed under master keys, with AWS stores as adapters

Status: Accepted
Date: 2026-11-17
Deciders: the user (Elmanuel1)
Scope: agentd's `SecretStore` port, a new `vault_secrets` table, the `secrets-aws` adapter crate and the
`agentd vault-secrets` CLI; the wire does not change

## Context

agentd keeps each vault credential's secret behind its `SecretStore` port. The only store is AWS Secrets Manager
under a customer KMS key, so AWS is mandatory even for a small deployment that runs everything else on Postgres.
The user's policy is one self-contained default behind each port, with external services as optional adapters.

The `cloud` runtime is ours and swaps `environment_variable` values at egress through iron-proxy, which reads
Secrets Manager or SSM Parameter Store, not Postgres.

## Decision

1. `AGENTD_SECRETS` picks the store: `postgres` (the default), `aws_sm` or `aws_ssm`, as iron-proxy names its
   sources (`aws` also selects the Secrets Manager store). A set `AGENTD_SECRETS_KMS_KEY_ID` picks `aws_sm` while
   `AGENTD_SECRETS` is unset, so an AWS deployment keeps working unchanged.
2. The Postgres store keeps each secret in `vault_secrets`, sealed with AES-256-GCM under a single-use key that
   HKDF-SHA256 derives from the first master key and a fresh 32-byte salt per put. The row keeps the salt and the
   master key's id. The value is bound to the credential and its kind through the associated data.
3. The master keys are a key ring from `AGENTD_SECRET_KEYS`: the first seals, every one opens. A new type,
   `shared::hash::SecretKeys`, holds them and derives value keys only, so no key can be read out.
4. `agentd vault-secrets reseal` moves every row to the first key with a new salt; rotation is add, move first,
   reseal, drop.
5. An SSM Parameter Store adapter sits beside the Secrets Manager one in `secrets-aws`, sharing its KMS key check,
   error mapping and tags.
6. `vault_secrets` is not tracked by the change feed or history.

## Alternatives considered

| Alternative | Why not |
|---|---|
| One AES-GCM key for every value (Kubernetes `aesgcm`) | Needs rotation every 200,000 writes to stay inside the nonce limit. |
| A KMS plugin in the Postgres store | Mixes the default with an external service; the external path is an AWS adapter, chosen whole. |
| A random data key per row, wrapped under the derived key (envelope encryption) | A second key and a second seal per row for no gain: the derived key is already single-use, and reseal encrypts the value again either way. |
| Supabase Vault, `pgsodium` | Needs a Postgres extension most managed databases lack. |
| AWS Encryption SDK | Pulls the AWS SDK into the default store. |
| OpenBao | Another service to run; out for now. |
| The secret in the credential's own transaction | Gives the port a Postgres transaction the AWS adapters cannot take. |

## Consequences

- A small deployment needs no AWS account for secrets; the master keys are the one thing to keep safe and backed
  up apart from the database.
- A row that does not open (a key dropped from the ring, a row altered) is a backend error logged at `ERROR`.
- `cloud` egress needs an AWS store; a Postgres deployment has no egress swap.
- SSM's put quota (3 TPS by default) fits about 3,000 hourly refreshed OAuth credentials; larger deployments turn on
  higher throughput or use Secrets Manager.
