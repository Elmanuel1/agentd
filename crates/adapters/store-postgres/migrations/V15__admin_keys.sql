-- admin_keys: organization admin keys, each stored as its peppered HMAC-SHA256 digest, never the
-- key. An admin key reaches only the /v1/organizations routes of its organization.
create table admin_keys (
    admin_key_id     text        primary key,
    org_id           text        not null,
    name             text        not null,
    key_digest       bytea       not null unique check (octet_length(key_digest) = 32),
    partial_key_hint text        not null,
    created_at       timestamptz not null,
    revoked_at       timestamptz
);
