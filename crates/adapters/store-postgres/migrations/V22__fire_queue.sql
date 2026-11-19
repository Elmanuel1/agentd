-- fire_claims is the fire queue: each occurrence waits here, with its schedule's spec, until a
-- worker runs it at or after due_at. outcome says why a done fire finished: `ran` (with a session or
-- a recorded error), `skipped` (its following occurrence came due first) or `dropped` (no handler owns
-- its kind, or its spec cannot be read); it is null until done.
set local lock_timeout = '5s';
alter table fire_claims
    add column spec    jsonb,
    add column due_at  timestamptz,
    add column outcome text;
