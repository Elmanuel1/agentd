-- Resource history: every tracked table records who wrote each row, and announces each delete, so the
-- change feed carries every change with its actor and request id (ADR resource-history 0001).

-- The writing connection's actor and request id, set by every write checkout; `unattributed` and null
-- when unset, as the column defaults of the coordination tables give.
create function history_stamp() returns trigger language plpgsql as $$
begin
    new.actor := coalesce(nullif(current_setting('agentd.actor', true), ''), 'unattributed');
    new.request_id := nullif(current_setting('agentd.request_id', true), '');
    return new;
end $$;

-- A delete's change event carries the row's last writer, not the deleter, so the delete is announced
-- with a transactional logical message that commits or rolls back with it.
create function history_deleted() returns trigger language plpgsql as $$
begin
    perform pg_logical_emit_message(true, 'agentd-history', jsonb_build_object(
        'table', tg_table_name,
        'actor', coalesce(nullif(current_setting('agentd.actor', true), ''), 'unattributed'),
        'request_id', nullif(current_setting('agentd.request_id', true), ''),
        'row', to_jsonb(old))::text);
    return old;
end $$;

-- Constant defaults and the replica identity change only the catalog. REPLICA IDENTITY FULL logs the
-- old row whole, so an update that leaves a TOASTed column (a doc over about 2 KB) unchanged still
-- reaches the feed with its value.
alter table agents
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table deployments
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table deployment_runs
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table files
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table vaults
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table credentials
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table skills
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table skill_versions
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table memory_stores
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table memories
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table memory_versions
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table sessions
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table dreams
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table dream_runs
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table api_keys
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table workspaces
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table environment_keys
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table admin_keys
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;

create trigger agents_history_stamp before insert or update on agents
    for each row execute function history_stamp();
create trigger agents_history_deleted before delete on agents
    for each row execute function history_deleted();
create trigger deployments_history_stamp before insert or update on deployments
    for each row execute function history_stamp();
create trigger deployments_history_deleted before delete on deployments
    for each row execute function history_deleted();
create trigger deployment_runs_history_stamp before insert or update on deployment_runs
    for each row execute function history_stamp();
create trigger deployment_runs_history_deleted before delete on deployment_runs
    for each row execute function history_deleted();
create trigger files_history_stamp before insert or update on files
    for each row execute function history_stamp();
create trigger files_history_deleted before delete on files
    for each row execute function history_deleted();
create trigger vaults_history_stamp before insert or update on vaults
    for each row execute function history_stamp();
create trigger vaults_history_deleted before delete on vaults
    for each row execute function history_deleted();
create trigger credentials_history_stamp before insert or update on credentials
    for each row execute function history_stamp();
create trigger credentials_history_deleted before delete on credentials
    for each row execute function history_deleted();
create trigger skills_history_stamp before insert or update on skills
    for each row execute function history_stamp();
create trigger skills_history_deleted before delete on skills
    for each row execute function history_deleted();
create trigger skill_versions_history_stamp before insert or update on skill_versions
    for each row execute function history_stamp();
create trigger skill_versions_history_deleted before delete on skill_versions
    for each row execute function history_deleted();
create trigger memory_stores_history_stamp before insert or update on memory_stores
    for each row execute function history_stamp();
create trigger memory_stores_history_deleted before delete on memory_stores
    for each row execute function history_deleted();
create trigger memories_history_stamp before insert or update on memories
    for each row execute function history_stamp();
create trigger memories_history_deleted before delete on memories
    for each row execute function history_deleted();
create trigger memory_versions_history_stamp before insert or update on memory_versions
    for each row execute function history_stamp();
create trigger memory_versions_history_deleted before delete on memory_versions
    for each row execute function history_deleted();
create trigger sessions_history_stamp before insert or update on sessions
    for each row execute function history_stamp();
create trigger sessions_history_deleted before delete on sessions
    for each row execute function history_deleted();
create trigger dreams_history_stamp before insert or update on dreams
    for each row execute function history_stamp();
create trigger dreams_history_deleted before delete on dreams
    for each row execute function history_deleted();
create trigger dream_runs_history_stamp before insert or update on dream_runs
    for each row execute function history_stamp();
create trigger dream_runs_history_deleted before delete on dream_runs
    for each row execute function history_deleted();
create trigger api_keys_history_stamp before insert or update on api_keys
    for each row execute function history_stamp();
create trigger api_keys_history_deleted before delete on api_keys
    for each row execute function history_deleted();
create trigger workspaces_history_stamp before insert or update on workspaces
    for each row execute function history_stamp();
create trigger workspaces_history_deleted before delete on workspaces
    for each row execute function history_deleted();
create trigger environment_keys_history_stamp before insert or update on environment_keys
    for each row execute function history_stamp();
create trigger environment_keys_history_deleted before delete on environment_keys
    for each row execute function history_deleted();
create trigger admin_keys_history_stamp before insert or update on admin_keys
    for each row execute function history_stamp();
create trigger admin_keys_history_deleted before delete on admin_keys
    for each row execute function history_deleted();
