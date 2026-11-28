-- Each row of these tables carries the time of its last write and a version raised by every
-- update, set by one trigger so every statement that writes them, raw SQL included, keeps both.
-- `updated_at` is the transaction's start time, for display; the controller's check reads
-- `version`.
set local lock_timeout = '5s';

create function stamp_change() returns trigger language plpgsql as $$
begin
    new.updated_at := now();
    if tg_op = 'UPDATE' then
        new.version := old.version + 1;
    end if;
    return new;
end $$;

alter table vaults
    add column updated_at timestamptz not null default now(),
    add column version bigint not null default 1;
alter table credentials
    add column updated_at timestamptz not null default now(),
    add column version bigint not null default 1;
alter table workspaces
    add column updated_at timestamptz not null default now(),
    add column version bigint not null default 1;
alter table environments
    add column updated_at timestamptz not null default now(),
    add column version bigint not null default 1;
alter table organizations
    add column updated_at timestamptz not null default now(),
    add column version bigint not null default 1;

alter table vaults alter column updated_at drop default;
alter table credentials alter column updated_at drop default;
alter table workspaces alter column updated_at drop default;
alter table environments alter column updated_at drop default;
alter table organizations alter column updated_at drop default;

create trigger vaults_stamp_change before insert or update on vaults
    for each row execute function stamp_change();
create trigger credentials_stamp_change before insert or update on credentials
    for each row execute function stamp_change();
create trigger workspaces_stamp_change before insert or update on workspaces
    for each row execute function stamp_change();
create trigger environments_stamp_change before insert or update on environments
    for each row execute function stamp_change();
create trigger organizations_stamp_change before insert or update on organizations
    for each row execute function stamp_change();
