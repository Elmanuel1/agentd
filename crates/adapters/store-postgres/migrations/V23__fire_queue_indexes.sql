-- The workers claim due fires that are not done, oldest due first; the hourly sweep deletes done
-- fires by their occurrence.
-- CONCURRENTLY, as V10: after a failed build, drop the INVALID index, run `flyway repair`, then
-- migrate again.
create index concurrently fire_claims_due_idx on fire_claims (due_at) where not done;
create index concurrently fire_claims_done_idx on fire_claims (scheduled_at) where done;
