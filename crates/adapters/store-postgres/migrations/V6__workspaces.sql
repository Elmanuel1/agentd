-- ── workspaces (ORG-scoped; the Admin API's /v1/organizations/workspaces) ──
-- The wire object lives in doc; the columns are what a list filters and
-- sorts by.
create table workspaces (
    org_id       text        not null,
    workspace_id text        not null,
    archived_at  timestamptz,
    created_at   timestamptz not null,
    doc          jsonb       not null,
    primary key (org_id, workspace_id)
);
create index workspaces_org_created_idx on workspaces (org_id, created_at desc, workspace_id desc);
