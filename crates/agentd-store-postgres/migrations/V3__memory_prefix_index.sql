-- Index the memories `path` for anchored prefix listing (`LIKE 'prefix%'`).
--
-- A default-collation btree index is NOT used for `LIKE`; `text_pattern_ops`
-- gives character-pattern (prefix) ordering that the planner can range-scan.
-- This turns the directory-browse listing from a sequential scan into an index
-- range scan, without changing the query or its semantics.
create index memories_path_prefix_idx
    on memories (workspace_id, store_id, path text_pattern_ops)
    where deleted = false;
