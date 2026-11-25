-- One row per organization, made by `agentd organization create`. Every org-scoped row names an
-- existing organization and goes with it on delete. The keys are added `not valid`: rows written
-- before this migration are not checked, every write after it is.
set local lock_timeout = '5s';

create table organizations (
    org_id     text        primary key,
    created_at timestamptz not null default now()
);

alter table workspaces add constraint workspaces_org_fk
    foreign key (org_id) references organizations on delete cascade not valid;
alter table environments add constraint environments_org_fk
    foreign key (org_id) references organizations on delete cascade not valid;
alter table admin_keys add constraint admin_keys_org_fk
    foreign key (org_id) references organizations on delete cascade not valid;
alter table api_keys add constraint api_keys_org_fk
    foreign key (org_id) references organizations on delete cascade not valid;
alter table environment_keys add constraint environment_keys_org_fk
    foreign key (org_id) references organizations on delete cascade not valid;
alter table work_items add constraint work_items_org_fk
    foreign key (org_id) references organizations on delete cascade not valid;
