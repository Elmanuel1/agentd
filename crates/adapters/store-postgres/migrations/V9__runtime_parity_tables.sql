-- The runtime's coordination tables: work items, environment keys, the step queue, MCP
-- connections, schedules and webhook deliveries. Rows here are updated in place (claims,
-- leases, heartbeats); they are not resources, so they keep no revisions.

-- ── work_items: one per session, reused for every run; its id is the session id ──
create table work_items (
    workspace_id        text        not null,
    session_id          text        not null,
    org_id              text        not null,
    environment_id      text        not null,
    queue               text        not null,
    state               text        not null,
    metadata            jsonb       not null,
    created_at          timestamptz not null,
    acknowledged_at     timestamptz,
    started_at          timestamptz,
    latest_heartbeat_at timestamptz,
    last_heartbeat      text,
    lease_until         timestamptz,
    handed_out_at       timestamptz,
    stop_requested_at   timestamptz,
    stopped_at          timestamptz,
    actor               text        not null
        default coalesce(nullif(current_setting('agentd.actor', true), ''), 'unattributed'),
    request_id          text
        default nullif(current_setting('agentd.request_id', true), ''),
    primary key (workspace_id, session_id)
);
create index work_items_queue_idx
    on work_items (queue, created_at) where state = 'queued';
create index work_items_environment_idx
    on work_items (workspace_id, environment_id, created_at desc);
create index work_items_runtime_environment_idx
    on work_items (environment_id) where queue = 'runtime';

-- ── environment_keys: as api_keys, only the peppered digest is kept ──
create table environment_keys (
    environment_key_id text        primary key,
    org_id             text        not null,
    workspace_id       text        not null,
    environment_id     text        not null,
    name               text        not null,
    key_digest         bytea       not null unique check (octet_length(key_digest) = 32),
    partial_key_hint   text        not null,
    created_at         timestamptz not null,
    revoked_at         timestamptz
);

-- ── loop_steps: the step queue, one row per session with a step due or running ──
-- Every step updates and then deletes its row: pages keep free room for the updates,
-- and autovacuum runs long before the default 20% of dead rows.
create table loop_steps (
    workspace_id text        not null,
    session_id   text        not null,
    due_at       timestamptz not null default now(),
    wakes        bigint      not null default 0,
    lease_owner  text,
    lease_until  timestamptz,
    attempts     bigint      not null default 0,
    primary key (workspace_id, session_id)
) with (
    fillfactor = 70,
    autovacuum_vacuum_scale_factor = 0.01,
    autovacuum_analyze_scale_factor = 0.02,
    autovacuum_vacuum_cost_limit = 2000
);
create index loop_steps_due_idx on loop_steps (due_at) where lease_until is null;
create index loop_steps_lease_idx on loop_steps (lease_until);

-- ── mcp_connections: an MCP server's session id and tool list, per agentd session ──
create table mcp_connections (
    workspace_id          text        not null,
    session_id            text        not null,
    server_url            text        not null,
    mcp_session_id        text,
    tools                 jsonb       not null,
    credential_updated_at timestamptz,
    connected_at          timestamptz not null,
    actor                 text        not null
        default coalesce(nullif(current_setting('agentd.actor', true), ''), 'unattributed'),
    request_id            text
        default nullif(current_setting('agentd.request_id', true), ''),
    primary key (workspace_id, session_id, server_url)
);

-- ── schedules: deployment and dream schedules, fired by whichever replica claims them ──
create table schedules (
    target       text        primary key,
    workspace_id text        not null,
    spec         jsonb       not null,
    next_fire_at timestamptz not null,
    paused       boolean     not null default false
);
create index schedules_due_idx on schedules (next_fire_at) where not paused;

-- ── webhook_deliveries: one row per event and configured endpoint until delivered or dropped ──
-- body is text, not jsonb: each attempt signs and sends these exact bytes.
create table webhook_deliveries (
    webhook_id      text        not null,
    endpoint_key    text        not null,
    endpoint_url    text        not null,
    body            text        not null,
    attempt         bigint      not null default 0,
    next_attempt_at timestamptz not null,
    claimed_until   timestamptz,
    delivered_at    timestamptz,
    dropped_at      timestamptz,
    primary key (webhook_id, endpoint_key)
);
create index webhook_deliveries_due_idx
    on webhook_deliveries (next_attempt_at) where delivered_at is null and dropped_at is null;
