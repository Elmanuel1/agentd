-- webhook_endpoint_states: each configured endpoint's health, shared by every replica. failing_since
-- is when its current run of failed attempts began; disabled_at and disabled_reason (`redirect`,
-- `private_address` or `failing`, mapped by the sender) say it gets nothing until it is enabled again.
-- config_digest is the SHA-256 of its URL and secret, so a changed endpoint is enabled again at boot.
-- Endpoint ids are unique across organizations, so the key alone identifies the row.
set local lock_timeout = '5s';
create table webhook_endpoint_states (
    endpoint_key    text primary key,
    org_id          text not null,
    failing_since   timestamptz,
    disabled_at     timestamptz,
    disabled_reason text,
    config_digest   text not null
);
-- Each delivery records its last answer, and claim_seq is its claim token: every claim raises it by
-- one and nothing lowers it, so a write from a replaced claim matches no row.
alter table webhook_deliveries
    add column last_status smallint,
    add column last_error  text,
    add column claim_seq   bigint not null default 0;
