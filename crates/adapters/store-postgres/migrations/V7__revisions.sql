-- Shared by every revisions table: no row is deleted, and a row changes only to set
-- superseded_by, or deleted_at and deleted_by, once.
create function revisions_append_only() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception '% rows are never deleted', tg_table_name;
    end if;
    if to_jsonb(new) - 'superseded_by' - 'deleted_at' - 'deleted_by'
           <> to_jsonb(old) - 'superseded_by' - 'deleted_at' - 'deleted_by'
       or (old.superseded_by is not null and new.superseded_by is distinct from old.superseded_by)
       or (old.deleted_at is not null and new.deleted_at is distinct from old.deleted_at)
       or (old.deleted_at is not null and new.deleted_by is distinct from old.deleted_by) then
        raise exception '% rows are append-only', tg_table_name;
    end if;
    return new;
end $$;
