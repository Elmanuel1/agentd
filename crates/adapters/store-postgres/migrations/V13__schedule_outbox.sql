-- schedule_outbox: each change to a schedule (upsert, pause, unpause, delete), written in the
-- transaction of the deployment write that causes it, and relayed in order to the scheduler sink,
-- which removes each row once applied.
create table schedule_outbox (
    seq        bigserial   primary key,
    target     text        not null,
    change     jsonb       not null,
    created_at timestamptz not null default now(),
    actor      text        not null
        default coalesce(nullif(current_setting('agentd.actor', true), ''), 'unattributed'),
    request_id text
        default nullif(current_setting('agentd.request_id', true), '')
);
