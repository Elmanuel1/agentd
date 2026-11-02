-- API keys. Only an HMAC-SHA256 digest of each key is kept, keyed with a
-- pepper held in Secrets Manager; the key itself is shown once at creation and
-- never stored. A request finds its key by the digest's unique index.
create table api_keys (
    api_key_id       text        primary key,
    org_id           text        not null,
    workspace_id     text        not null,
    name             text        not null,
    key_digest       bytea       not null unique check (octet_length(key_digest) = 32),
    partial_key_hint text        not null,
    created_at       timestamptz not null,
    revoked_at       timestamptz
);
