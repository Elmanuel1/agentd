-- One compaction checkpoint per thread and covered event, and the index a step reads
-- each thread's latest checkpoint from. Events written before compaction carry no
-- checkpoint, so their key is null and they never collide.
-- CONCURRENTLY, as V10: after a failed build, drop the INVALID index, run
-- `flyway repair`, then migrate again.
create unique index concurrently session_events_checkpoint
    on session_events (workspace_id, session_id, (doc->>'session_thread_id'),
                       ((doc->'checkpoint'->>'through_seq')::bigint))
    where doc->>'type' = 'agent.thread_context_compacted';
