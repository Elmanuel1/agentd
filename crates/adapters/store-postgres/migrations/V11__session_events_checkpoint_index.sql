-- One compaction checkpoint per thread and covered event, and the index a step reads
-- each thread's latest checkpoint from. The loop keeps a checkpoint beside its
-- agent.thread_context_compacted event, under the doc's engine object, naming the
-- last event it covers. The primary thread's events carry no session_thread_id, so it
-- is keyed as ''.
-- CONCURRENTLY, as V10: after a failed build, drop the INVALID index, run
-- `flyway repair`, then migrate again.
create unique index concurrently session_events_checkpoint
    on session_events (workspace_id, session_id, (coalesce(doc->>'session_thread_id', '')),
                       (doc->'engine'->'checkpoint'->>'through'))
    where doc->>'type' = 'agent.thread_context_compacted';
