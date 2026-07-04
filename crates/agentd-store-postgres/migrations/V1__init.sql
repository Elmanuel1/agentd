-- agentd control-plane schema V1.
--
-- Hybrid layout: the exact wire object is kept in `doc` (jsonb) for perfect
-- fidelity, while the fields a dashboard filters/sorts/searches on are lifted
-- into real columns with indexes. Dashboards ("list my agents, newest first,
-- only active, matching a name") are plain SQL — the reason we're on Postgres.

-- ── agents (versioned) ──────────────────────────────────────────────────
create table agents (
    workspace_id text        not null,
    agent_id     text        not null,
    version      bigint      not null,
    name         text        not null,
    archived_at  timestamptz,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (workspace_id, agent_id, version)
);
create index agents_ws_created_idx on agents (workspace_id, created_at desc);
create index agents_ws_name_idx    on agents (workspace_id, name);

-- ── deployments (+ runs + the exactly-once claim) ───────────────────────
create table deployments (
    workspace_id  text        not null,
    deployment_id text        not null,
    name          text        not null,
    status        text        not null,
    archived_at   timestamptz,
    created_at    timestamptz not null,
    doc           jsonb       not null,
    primary key (workspace_id, deployment_id)
);
create index deployments_ws_created_idx on deployments (workspace_id, created_at desc);
create index deployments_ws_status_idx  on deployments (workspace_id, status);

create table deployment_runs (
    workspace_id  text        not null,
    deployment_id text        not null,
    run_id        text        not null,
    created_at    timestamptz not null,
    has_error     boolean     not null,
    doc           jsonb       not null,
    primary key (workspace_id, deployment_id, run_id)
);
create index deployment_runs_lookup_idx
    on deployment_runs (workspace_id, deployment_id, created_at desc);

-- The primary key IS the cross-replica exactly-once guarantee: a second
-- replica inserting the same (deployment, scheduled_at) hits the constraint
-- and `on conflict do nothing` reports zero rows — only the winner fires.
create table deployment_claims (
    workspace_id  text        not null,
    deployment_id text        not null,
    scheduled_at  text        not null,
    claimed_at    timestamptz not null default now(),
    primary key (workspace_id, deployment_id, scheduled_at)
);

-- ── files (metadata + bytes) ────────────────────────────────────────────
-- Bytes live in a bytea column for a pure-Postgres story; swap to S3 for
-- large-object workloads without touching the trait.
create table files (
    workspace_id text        not null,
    file_id      text        not null,
    filename     text        not null,
    scope_id     text,
    size_bytes   bigint      not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    bytes        bytea       not null,
    primary key (workspace_id, file_id)
);
create index files_ws_scope_idx on files (workspace_id, scope_id);
