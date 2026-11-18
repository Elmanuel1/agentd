# ADR 0002: Each vault secret is sealed under its write's own derived key, with no data key

Status: Accepted
Date: 2026-11-17
Deciders: the user (Elmanuel1)
Scope: the Postgres secret store's row format (`vault_secrets`, V21), its cipher and `agentd vault-secrets
reseal`; supersedes decisions 2 to 4 of [ADR 0001](0001-postgres-secret-store.md)

## Context

ADR 0001 sealed each value under a random per-row data key and sealed that data key under a key HKDF-SHA256
derives from a master key and a fresh salt. The derived key is already single-use: every write draws a new salt,
so no key encrypts twice, and `reseal` encrypts the value again under the first master key either way. The data
key adds a second key and a second seal per row and protects nothing the derived key does not.

## Decision

1. Each put seals the value with AES-256-GCM directly under the key HKDF-SHA256 derives from the first master key
   and a fresh 32-byte salt, with the info `agentd/vault-secret/value/v1`. The value is bound to the workspace,
   vault, credential and kind through the associated data, as before.
2. The row keeps `salt` (exactly 32 bytes), the master key's id and `sealed_value`. V21 replaces V20's
   `sealed_key` with `salt`.
3. The master keys are a key ring from `AGENTD_SECRET_KEYS`: the first seals, every one opens.
   `shared::hash::SecretKeys` derives value keys only, so no key can be read out.
4. `agentd vault-secrets reseal` moves every row to the first key with a new salt; its guarded update compares
   the key id and salt it read, so a put in between wins. Rotation is add, move first, reseal, drop.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Keep the per-row data key (ADR 0001) | A second key and a second seal per row for no gain. |
| Change V20 in place | V20 is on `main`; a database that applied it would fail Flyway's checksum. |

## Consequences

- One AES-GCM seal per write; the row is 32 bytes of salt plus the sealed value.
- A row sealed the V20 way does not open under the new cipher (V21 says how such rows are handled).
