-- A runtime key reads its queue's work items by session, and lists them newest first.
-- The primary key leads with workspace_id and work_items_queue_idx holds only queued
-- rows, so neither serves these reads.
-- CONCURRENTLY, as V10: after a failed build, drop the INVALID index, run
-- `flyway repair`, then migrate again.
create index concurrently work_items_queue_session_idx
    on work_items (queue, session_id);
create index concurrently work_items_queue_created_idx
    on work_items (queue, created_at desc);
