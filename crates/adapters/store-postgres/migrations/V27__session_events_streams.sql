-- Each event's thread and type as columns, as an event store keys its streams: a thread's step reads
-- its own events from its latest checkpoint, the session's events and the outcomes' spans through
-- these indexes instead of every event's document. The primary thread's events carry no
-- session_thread_id, so it is ''. `text_pattern_ops` serves the prefix matches (`session.%`).
alter table session_events
    add column thread_id text generated always as (coalesce(doc->>'session_thread_id', '')) stored,
    add column event_type text generated always as (doc->>'type') stored;
create index session_events_thread_idx on session_events (workspace_id, session_id, thread_id, seq);
create index session_events_type_idx
    on session_events (workspace_id, session_id, event_type text_pattern_ops, seq);
