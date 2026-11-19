-- fire_claims is the fire queue: each occurrence waits here, with its schedule's spec, until a
-- worker runs it at or after due_at. outcome says why a done fire finished: it ran (with a session
-- or a recorded error), it was skipped once its following occurrence came due, or it was dropped
-- because no handler owns its kind or its spec cannot be read. outcome is null until done.
alter table fire_claims
    add column spec    jsonb,
    add column due_at  timestamptz,
    add column outcome text check (outcome in ('ran', 'skipped', 'dropped'));
