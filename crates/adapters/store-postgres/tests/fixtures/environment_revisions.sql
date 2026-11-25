-- Run by CI between V27 and V28: one environment with a superseded and a live revision, and
-- one deleted environment, for V28 to copy (pg_environments_table.rs).
insert into environments (org_id, environment_id, name, state, created_at, doc)
values ('org_seed', 'env_seedlive', 'first', 'active', '2026-01-01T00:00:00Z', '{"name":"first"}'),
       ('org_seed', 'env_seedgone', 'gone', 'active', '2026-01-02T00:00:00Z', '{"name":"gone"}');
update environments set name = 'second', doc = '{"name":"second"}'
where org_id = 'org_seed' and environment_id = 'env_seedlive';
delete from environments where org_id = 'org_seed' and environment_id = 'env_seedgone';
