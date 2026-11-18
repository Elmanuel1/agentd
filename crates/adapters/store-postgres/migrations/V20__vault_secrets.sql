-- vault_secrets: each vault credential's sealed material when AGENTD_SECRETS=postgres. Values are
-- AES-256-GCM under a per-write data key; the master keys live only in AGENTD_SECRET_KEYS.
-- Untracked: no writer columns, no history trigger, not in the change feed's include list.
set local lock_timeout = '5s';
create table vault_secrets (
    workspace_id         text    not null,
    vault_id             text    not null,
    credential_id        text    not null,
    environment_variable boolean not null,
    key_id               text    not null,
    sealed_key           bytea   not null check (octet_length(sealed_key) = 92),
    sealed_value         bytea   not null check (octet_length(sealed_value) >= 28),
    primary key (workspace_id, vault_id, credential_id)
);
