-- vault_secrets: each value is AES-256-GCM under a single-use key that HKDF-SHA256 derives from a
-- master key and the row's 32-byte salt, so the row keeps the salt in place of a sealed key.
set local lock_timeout = '5s';
alter table vault_secrets
    drop column sealed_key,
    add column salt bytea not null check (octet_length(salt) = 32);
