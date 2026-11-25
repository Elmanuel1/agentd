-- Environments are one plain table again, one row per environment, so the change feed carries
-- them and a delete removes the row. Each current revision becomes the environment's row; past
-- revisions are not kept.
set local lock_timeout = '5s';

drop view environments;
drop function environments_revise();
drop function environments_soft_delete();

create table environments (
    org_id         text        not null,
    environment_id text        not null,
    name           text        not null,
    state          text        not null,
    created_at     timestamptz not null,
    doc            jsonb       not null,
    primary key (org_id, environment_id)
);

insert into environments (org_id, environment_id, name, state, created_at, doc)
select org_id, environment_id, name, state, created_at, doc
from environment_revisions
where superseded_by is null and deleted_at is null;

drop index environments_org_created_idx;
drop table environment_revisions;
drop function revisions_append_only();

create index environments_org_created_idx on environments (org_id, created_at desc, environment_id desc);
