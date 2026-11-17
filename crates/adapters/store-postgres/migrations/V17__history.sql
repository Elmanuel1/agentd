-- Resource history: each tracked table's every change, built by agentd's history consumer from the
-- change feed (ADR resource-history 0001). history.<table> has the live table's columns with the same
-- types, the change's own actor and request id among them, then where the change sits in the log. The
-- key is the live key plus the lsn, so a change delivered twice is recorded once.
--
-- A migration that adds a column to a tracked table adds it to its history table, and a new tracked
-- table gets a history table and joins history.changes, in the same migration.
create schema history;

create table history.agents (
    workspace_id text,
    agent_id     text,
    version      bigint,
    name         text,
    archived_at  timestamptz,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, agent_id, version, lsn)
);
create index agents_actor_idx on history.agents (actor, committed_at);
create index agents_request_idx on history.agents (request_id);

create table history.deployments (
    workspace_id  text,
    deployment_id text,
    name          text,
    status        text,
    archived_at   timestamptz,
    created_at    timestamptz,
    doc           jsonb,
    actor         text,
    request_id    text,
    lsn           pg_lsn      not null,
    tx_id         bigint,
    op            text        not null,
    committed_at  timestamptz not null,
    primary key (workspace_id, deployment_id, lsn)
);
create index deployments_actor_idx on history.deployments (actor, committed_at);
create index deployments_request_idx on history.deployments (request_id);

create table history.deployment_runs (
    workspace_id  text,
    deployment_id text,
    run_id        text,
    created_at    timestamptz,
    has_error     boolean,
    doc           jsonb,
    actor         text,
    request_id    text,
    lsn           pg_lsn      not null,
    tx_id         bigint,
    op            text        not null,
    committed_at  timestamptz not null,
    primary key (workspace_id, deployment_id, run_id, lsn)
);
create index deployment_runs_actor_idx on history.deployment_runs (actor, committed_at);
create index deployment_runs_request_idx on history.deployment_runs (request_id);

create table history.files (
    workspace_id text,
    file_id      text,
    filename     text,
    scope_id     text,
    size_bytes   bigint,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, file_id, lsn)
);
create index files_actor_idx on history.files (actor, committed_at);
create index files_request_idx on history.files (request_id);

create table history.vaults (
    workspace_id text,
    vault_id     text,
    display_name text,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, vault_id, lsn)
);
create index vaults_actor_idx on history.vaults (actor, committed_at);
create index vaults_request_idx on history.vaults (request_id);

create table history.credentials (
    workspace_id  text,
    vault_id      text,
    credential_id text,
    display_name  text,
    has_secret    boolean,
    created_at    timestamptz,
    doc           jsonb,
    actor         text,
    request_id    text,
    lsn           pg_lsn      not null,
    tx_id         bigint,
    op            text        not null,
    committed_at  timestamptz not null,
    primary key (workspace_id, vault_id, credential_id, lsn)
);
create index credentials_actor_idx on history.credentials (actor, committed_at);
create index credentials_request_idx on history.credentials (request_id);

create table history.skills (
    workspace_id text,
    skill_id     text,
    display_name text,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, skill_id, lsn)
);
create index skills_actor_idx on history.skills (actor, committed_at);
create index skills_request_idx on history.skills (request_id);

create table history.skill_versions (
    workspace_id text,
    skill_id     text,
    version      bigint,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, skill_id, version, lsn)
);
create index skill_versions_actor_idx on history.skill_versions (actor, committed_at);
create index skill_versions_request_idx on history.skill_versions (request_id);

create table history.memory_stores (
    workspace_id text,
    store_id     text,
    name         text,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, store_id, lsn)
);
create index memory_stores_actor_idx on history.memory_stores (actor, committed_at);
create index memory_stores_request_idx on history.memory_stores (request_id);

create table history.memories (
    workspace_id    text,
    store_id        text,
    memory_id       text,
    path            text,
    head_version_id text,
    deleted         boolean,
    created_at      timestamptz,
    doc             jsonb,
    actor           text,
    request_id      text,
    lsn             pg_lsn      not null,
    tx_id           bigint,
    op              text        not null,
    committed_at    timestamptz not null,
    primary key (workspace_id, store_id, memory_id, lsn)
);
create index memories_actor_idx on history.memories (actor, committed_at);
create index memories_request_idx on history.memories (request_id);

create table history.memory_versions (
    workspace_id text,
    store_id     text,
    memory_id    text,
    version_id   text,
    seq          bigint,
    has_blob     boolean,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, store_id, version_id, lsn)
);
create index memory_versions_actor_idx on history.memory_versions (actor, committed_at);
create index memory_versions_request_idx on history.memory_versions (request_id);

create table history.sessions (
    workspace_id text,
    session_id   text,
    status       text,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, session_id, lsn)
);
create index sessions_actor_idx on history.sessions (actor, committed_at);
create index sessions_request_idx on history.sessions (request_id);

create table history.dreams (
    workspace_id text,
    dream_id     text,
    name         text,
    status       text,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, dream_id, lsn)
);
create index dreams_actor_idx on history.dreams (actor, committed_at);
create index dreams_request_idx on history.dreams (request_id);

create table history.dream_runs (
    workspace_id text,
    dream_id     text,
    run_id       text,
    seq          bigint,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (workspace_id, dream_id, run_id, lsn)
);
create index dream_runs_actor_idx on history.dream_runs (actor, committed_at);
create index dream_runs_request_idx on history.dream_runs (request_id);

create table history.api_keys (
    api_key_id       text,
    org_id           text,
    workspace_id     text,
    name             text,
    key_digest       bytea,
    partial_key_hint text,
    created_at       timestamptz,
    revoked_at       timestamptz,
    actor            text,
    request_id       text,
    lsn              pg_lsn      not null,
    tx_id            bigint,
    op               text        not null,
    committed_at     timestamptz not null,
    primary key (api_key_id, lsn)
);
create index api_keys_actor_idx on history.api_keys (actor, committed_at);
create index api_keys_request_idx on history.api_keys (request_id);

create table history.workspaces (
    org_id       text,
    workspace_id text,
    archived_at  timestamptz,
    created_at   timestamptz,
    doc          jsonb,
    actor        text,
    request_id   text,
    lsn          pg_lsn      not null,
    tx_id        bigint,
    op           text        not null,
    committed_at timestamptz not null,
    primary key (org_id, workspace_id, lsn)
);
create index workspaces_actor_idx on history.workspaces (actor, committed_at);
create index workspaces_request_idx on history.workspaces (request_id);

create table history.environment_keys (
    environment_key_id text,
    org_id             text,
    workspace_id       text,
    environment_id     text,
    name               text,
    key_digest         bytea,
    partial_key_hint   text,
    created_at         timestamptz,
    revoked_at         timestamptz,
    actor              text,
    request_id         text,
    lsn                pg_lsn      not null,
    tx_id              bigint,
    op                 text        not null,
    committed_at       timestamptz not null,
    primary key (environment_key_id, lsn)
);
create index environment_keys_actor_idx on history.environment_keys (actor, committed_at);
create index environment_keys_request_idx on history.environment_keys (request_id);

create table history.admin_keys (
    admin_key_id     text,
    org_id           text,
    name             text,
    key_digest       bytea,
    partial_key_hint text,
    created_at       timestamptz,
    revoked_at       timestamptz,
    actor            text,
    request_id       text,
    lsn              pg_lsn      not null,
    tx_id            bigint,
    op               text        not null,
    committed_at     timestamptz not null,
    primary key (admin_key_id, lsn)
);
create index admin_keys_actor_idx on history.admin_keys (actor, committed_at);
create index admin_keys_request_idx on history.admin_keys (request_id);

-- Every change across the tracked tables, for search by actor and time or by request id. Each branch
-- reads its table by the indexes above, and Postgres merges them in committed_at order.
create view history.changes as
    select 'agents'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'agent_id', agent_id, 'version', version) as row_key,
           lsn, op, actor, request_id, committed_at from history.agents
union all select 'deployments'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'deployment_id', deployment_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.deployments
union all select 'deployment_runs'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'deployment_id', deployment_id, 'run_id', run_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.deployment_runs
union all select 'files'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'file_id', file_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.files
union all select 'vaults'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'vault_id', vault_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.vaults
union all select 'credentials'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'vault_id', vault_id, 'credential_id', credential_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.credentials
union all select 'skills'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'skill_id', skill_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.skills
union all select 'skill_versions'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'skill_id', skill_id, 'version', version) as row_key,
           lsn, op, actor, request_id, committed_at from history.skill_versions
union all select 'memory_stores'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'store_id', store_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.memory_stores
union all select 'memories'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'store_id', store_id, 'memory_id', memory_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.memories
union all select 'memory_versions'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'store_id', store_id, 'version_id', version_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.memory_versions
union all select 'sessions'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'session_id', session_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.sessions
union all select 'dreams'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'dream_id', dream_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.dreams
union all select 'dream_runs'::text as table_name, jsonb_build_object('workspace_id', workspace_id, 'dream_id', dream_id, 'run_id', run_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.dream_runs
union all select 'api_keys'::text as table_name, jsonb_build_object('api_key_id', api_key_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.api_keys
union all select 'workspaces'::text as table_name, jsonb_build_object('org_id', org_id, 'workspace_id', workspace_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.workspaces
union all select 'environment_keys'::text as table_name, jsonb_build_object('environment_key_id', environment_key_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.environment_keys
union all select 'admin_keys'::text as table_name, jsonb_build_object('admin_key_id', admin_key_id) as row_key,
           lsn, op, actor, request_id, committed_at from history.admin_keys;
