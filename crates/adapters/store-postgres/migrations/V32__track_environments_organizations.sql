-- Environments and organizations are tracked tables: each write records its writer and each delete
-- is announced, so the change feed carries every one with its actor and the whole row.
set local lock_timeout = '5s';

alter table environments
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;
alter table organizations
    add column actor text not null default 'unattributed',
    add column request_id text,
    replica identity full;

create trigger environments_history_stamp before insert or update on environments
    for each row execute function history_stamp();
create trigger environments_history_deleted before delete on environments
    for each row execute function history_deleted();
create trigger organizations_history_stamp before insert or update on organizations
    for each row execute function history_stamp();
create trigger organizations_history_deleted before delete on organizations
    for each row execute function history_deleted();
