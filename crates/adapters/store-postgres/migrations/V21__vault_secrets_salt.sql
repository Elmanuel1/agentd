-- vault_secrets: each value is AES-256-GCM under a single-use key that HKDF-SHA256 derives from a
-- master key and the row's 32-byte salt, so the row keeps the salt in place of a sealed key. A row
-- sealed the V20 way cannot open under it: the table is emptied first.
set local lock_timeout = '5s';
truncate vault_secrets;
alter table vault_secrets
    drop column sealed_key,
    add column salt bytea not null check (octet_length(salt) = 32);
