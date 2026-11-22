-- One step row per (session, thread): each thread of a session claims and runs its own step, so a
-- session's threads call the model at the same time. `thread_id = ''` is the session's own step,
-- which takes the client's events, wakes the threads that have work and makes the decisions that
-- span the session (idle, the grading round, the budget). A wake without a thread lands on it.
alter table loop_steps add column thread_id text not null default '';
alter table loop_steps drop constraint loop_steps_pkey;
alter table loop_steps add primary key (workspace_id, session_id, thread_id);

-- The transaction that wrote each event. Writers no longer take a per-session lock, so an event can
-- commit after a later-numbered one; readers serve events only below the oldest running transaction
-- (`pg_snapshot_xmin`) and page by (xid, seq), so no reader skips an event that commits late.
alter table session_events add column xid xid8 not null default pg_current_xact_id();
create index session_events_xid_idx on session_events (workspace_id, session_id, xid, seq);
