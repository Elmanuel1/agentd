-- One compaction checkpoint per thread and covered event, and the index a step reads
-- each thread's latest checkpoint from. The primary thread's events carry no
-- session_thread_id, so it is keyed as ''. Events written before compaction carry no
-- checkpoint, so their through_seq is null and they never collide.
-- CONCURRENTLY, as V10: after a failed build, drop the INVALID index, run
-- `flyway repair`, then migrate again.
create unique index concurrently session_events_checkpoint
    on session_events (workspace_id, session_id, (coalesce(doc->>'session_thread_id', '')),
                       ((doc->'checkpoint'->>'through_seq')::bigint))
    where doc->>'type' = 'agent.thread_context_compacted';
