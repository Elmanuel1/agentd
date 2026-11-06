-- Environments keep every revision. The live row of an environment is the one with
-- superseded_by is null and deleted_at is null; the view `environments` shows those
-- rows with the old columns, so every existing query reads and writes it unchanged.
alter table environments rename to environment_revisions;
alter table environment_revisions
    add column revision      bigserial,
    add column written_at    timestamptz not null default now(),
    add column superseded_by bigint,
    add column deleted_at    timestamptz,
    add column deleted_by    text,
    add column actor         text        not null
        default coalesce(nullif(current_setting('agentd.actor', true), ''), 'unattributed'),
    add column request_id    text
        default nullif(current_setting('agentd.request_id', true), ''),
    drop constraint environments_pkey,
    add primary key (org_id, environment_id, revision);

create unique index environment_revisions_live_idx
    on environment_revisions (org_id, environment_id) where superseded_by is null and deleted_at is null;

drop index environments_org_created_idx;
create index environments_org_created_idx
    on environment_revisions (org_id, created_at desc) where superseded_by is null and deleted_at is null;

create view environments as
    select org_id, environment_id, name, state, created_at, doc
    from environment_revisions where superseded_by is null and deleted_at is null;

-- UPDATE environments: mark the live row superseded, then insert the new value as the live row.
create function environments_revise() returns trigger language plpgsql as $$
declare
    next_rev bigint := nextval(pg_get_serial_sequence('environment_revisions', 'revision'));
begin
    update environment_revisions
       set superseded_by = next_rev
     where org_id = old.org_id and environment_id = old.environment_id
       and superseded_by is null and deleted_at is null;
    insert into environment_revisions (org_id, environment_id, name, state, created_at, doc, revision)
    values (new.org_id, new.environment_id, new.name, new.state, new.created_at, new.doc, next_rev);
    return new;
end $$;

-- DELETE FROM environments: stamp the live row deleted; nothing is removed.
create function environments_soft_delete() returns trigger language plpgsql as $$
begin
    update environment_revisions
       set deleted_at = now(),
           deleted_by = coalesce(nullif(current_setting('agentd.actor', true), ''), 'unattributed')
     where org_id = old.org_id and environment_id = old.environment_id
       and superseded_by is null and deleted_at is null;
    return old;
end $$;

create trigger environments_revise instead of update on environments
    for each row execute function environments_revise();
create trigger environments_soft_delete instead of delete on environments
    for each row execute function environments_soft_delete();
create trigger environment_revisions_append_only before update or delete on environment_revisions
    for each row execute function revisions_append_only();
