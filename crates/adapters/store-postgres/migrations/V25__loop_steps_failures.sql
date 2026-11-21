-- A step's failures in a row: each failure waits up to 5 s doubled per earlier one, at most 5
-- minutes, and a commit resets the count. bigint, as attempts.
set local lock_timeout = '5s';
alter table loop_steps add column failures bigint not null default 0;
