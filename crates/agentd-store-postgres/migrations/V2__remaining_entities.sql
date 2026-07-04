-- agentd control-plane schema V2 — the remaining entities.
--
-- Same hybrid layout as V1: full wire object in `doc` (jsonb), dashboard
-- filter/sort columns lifted out with indexes. Large content blobs (memory
-- version content, skill SKILL.md, credential secrets) NEVER live here — they
-- go to object storage via the agentd-blobs BlobStore, keyed by entity id.

-- ── environments (ORG-scoped, shared across workspaces) ─────────────────────
create table environments (
    org_id         text        not null,
    environment_id text        not null,
    name           text        not null,
    state          text        not null,
    created_at     timestamptz not null,
    doc            jsonb       not null,
    primary key (org_id, environment_id)
);
create index environments_org_created_idx on environments (org_id, created_at desc);

-- ── vaults (+ credentials) ──────────────────────────────────────────────────
-- Vault + the SECRET-FREE credential projection live here. The secret material
-- (SecretString) is written to object storage as an opaque blob keyed by
-- credential id — a stand-in for a real secret manager, never in the DB.
create table vaults (
    workspace_id text        not null,
    vault_id     text        not null,
    display_name text        not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (workspace_id, vault_id)
);
create index vaults_ws_created_idx on vaults (workspace_id, created_at desc);

create table credentials (
    workspace_id  text        not null,
    vault_id      text        not null,
    credential_id text        not null,
    display_name  text        not null,
    has_secret    boolean     not null,
    created_at    timestamptz not null,
    doc           jsonb       not null,   -- secret-free wire Credential
    primary key (workspace_id, vault_id, credential_id)
);
create index credentials_vault_idx on credentials (workspace_id, vault_id);

-- ── skills (+ versions) ─────────────────────────────────────────────────────
-- SKILL.md content goes to object storage keyed by (skill_id, version); only
-- metadata is kept here.
create table skills (
    workspace_id text        not null,
    skill_id     text        not null,
    display_name text        not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (workspace_id, skill_id)
);
create index skills_ws_created_idx on skills (workspace_id, created_at desc);

create table skill_versions (
    workspace_id text        not null,
    skill_id     text        not null,
    version      bigint      not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,   -- SkillVersion with content unset
    primary key (workspace_id, skill_id, version)
);

-- ── memory (stores → memories → versions), agent-style versioning ───────────
create table memory_stores (
    workspace_id text        not null,
    store_id     text        not null,
    name         text        not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (workspace_id, store_id)
);
create index memory_stores_ws_created_idx on memory_stores (workspace_id, created_at desc);

-- head pointer: `head_version_id` IS the `latest` ref. `deleted` retains the
-- chain for audit. `path` is unique among LIVE memories in a store.
create table memories (
    workspace_id    text        not null,
    store_id        text        not null,
    memory_id       text        not null,
    path            text        not null,
    head_version_id text        not null,
    deleted         boolean     not null,
    created_at      timestamptz not null,
    doc             jsonb       not null,   -- head Memory (content unset)
    primary key (workspace_id, store_id, memory_id)
);
create unique index memories_live_path_idx
    on memories (workspace_id, store_id, path) where deleted = false;

-- append-only, immutable once written (redact scrubs the S3 blob + flips
-- has_blob, never deletes the row). `seq` gives global newest-first ordering.
create table memory_versions (
    workspace_id  text        not null,
    store_id      text        not null,
    memory_id     text        not null,
    version_id    text        not null,
    seq           bigserial   not null,
    has_blob      boolean     not null,   -- content in S3 (false after redact)
    created_at    timestamptz not null,
    doc           jsonb       not null,   -- MemoryVersion metadata (content unset)
    primary key (workspace_id, store_id, version_id)
);
create index memory_versions_chain_idx
    on memory_versions (workspace_id, store_id, memory_id, seq);
create index memory_versions_store_seq_idx
    on memory_versions (workspace_id, store_id, seq desc);

-- ── sessions (+ append-only event log) ──────────────────────────────────────
create table sessions (
    workspace_id text        not null,
    session_id   text        not null,
    status       text        not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (workspace_id, session_id)
);
create index sessions_ws_created_idx on sessions (workspace_id, created_at desc);
create index sessions_ws_status_idx  on sessions (workspace_id, status);

-- The shared event log: SSE tails this table (order by seq), NOT an in-process
-- broadcast — so any replica can serve a session's stream.
create table session_events (
    workspace_id text        not null,
    session_id   text        not null,
    event_id     text        not null,
    seq          bigserial   not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (workspace_id, session_id, event_id)
);
create index session_events_log_idx
    on session_events (workspace_id, session_id, seq);

-- ── dreams (+ runs) ─────────────────────────────────────────────────────────
create table dreams (
    workspace_id text        not null,
    dream_id     text        not null,
    name         text        not null,
    status       text        not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (workspace_id, dream_id)
);
create index dreams_ws_created_idx on dreams (workspace_id, created_at desc);

create table dream_runs (
    workspace_id text        not null,
    dream_id     text        not null,
    run_id       text        not null,
    seq          bigserial   not null,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (workspace_id, dream_id, run_id)
);
create index dream_runs_lookup_idx
    on dream_runs (workspace_id, dream_id, seq desc);
