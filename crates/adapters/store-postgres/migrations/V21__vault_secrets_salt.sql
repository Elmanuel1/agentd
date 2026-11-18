-- vault_secrets: each value is AES-256-GCM under a single-use key that HKDF-SHA256 derives from a
-- master key and the row's 32-byte salt, so the row keeps the salt in place of a sealed key. A row
-- sealed the V20 way cannot open under it, so the migration refuses while any row exists.
set local lock_timeout = '5s';
do $$
begin
    perform 1 from vault_secrets limit 1;
    if found then
        raise exception 'vault_secrets has rows sealed the V20 way, which this version cannot open: delete those vault credentials, run the migration again, then create them again';
    end if;
end
$$;
alter table vault_secrets
    drop column sealed_key,
    add column salt bytea not null check (octet_length(salt) = 32);
