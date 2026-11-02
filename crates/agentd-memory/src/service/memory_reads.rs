use agentd_core::{ApiError, AuthContext};

use super::{map_store, parse_mem_pair, parse_store_id, MemoriesService, View};
use crate::store::{MemoryRecord, VersionSelector};
use crate::types::{Memory, MemoryListItem, MemoryListPage, MemoryPrefix, MemoryPrefixTag};

/// List ordering — matches the live `order_by` values (default `created_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryOrder {
    /// Newest first.
    CreatedAt,
    /// Path ascending.
    Path,
}

impl MemoryOrder {
    fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw {
            None | Some("created_at") => Ok(Self::CreatedAt),
            Some("path") => Ok(Self::Path),
            Some(_) => Err(ApiError::invalid_request("order_by must be one of: created_at, path")),
        }
    }
}

/// Normalize a `path_prefix` to a directory boundary: default `/`, always
/// trailing-slash terminated (live treats `/notes` == `/notes/`).
fn normalize_prefix(raw: Option<&str>) -> String {
    let p = raw.filter(|s| !s.is_empty()).unwrap_or("/");
    if p.ends_with('/') {
        p.to_owned()
    } else {
        format!("{p}/")
    }
}

fn item_path(item: &MemoryListItem) -> &str {
    match item {
        MemoryListItem::Memory(m) => &m.path,
        MemoryListItem::Prefix(p) => &p.path,
    }
}

/// Collapse memories whose path is deeper than `depth` (relative to `prefix`)
/// into `memory_prefix` directory entries. `depth` counts path segments below
/// the prefix — a memory with ≤ `depth` segments is a leaf, deeper ones fold
/// into the first-`depth`-segment directory.
fn collapse_depth(
    memories: Vec<Memory>,
    prefix: &str,
    depth: u32,
    order: MemoryOrder,
) -> Vec<MemoryListItem> {
    use std::collections::BTreeSet;
    let mut items: Vec<MemoryListItem> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for m in memories {
        let rel = m.path.strip_prefix(prefix).unwrap_or(m.path.as_str());
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if (segs.len() as u32) <= depth {
            items.push(MemoryListItem::Memory(Box::new(m)));
        } else {
            let taken: Vec<&str> = segs.into_iter().take(depth as usize).collect();
            let dir = format!("{prefix}{}/", taken.join("/"));
            if seen.insert(dir.clone()) {
                items.push(MemoryListItem::Prefix(MemoryPrefix {
                    path: dir,
                    tag: MemoryPrefixTag::MemoryPrefix,
                }));
            }
        }
    }
    if matches!(order, MemoryOrder::Path) {
        items.sort_by(|a, b| item_path(a).cmp(item_path(b)));
    }
    items
}

impl MemoriesService {
    /// `GET /v1/memory_stores/{sid}/memories/{mid}` — retrieve defaults to
    /// full view (content included).
    pub async fn get_memory(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        raw_mem_id: &str,
        view: View,
    ) -> Result<Memory, ApiError> {
        let (sid, mid) = parse_mem_pair(raw_store_id, raw_mem_id)?;
        let record = self.store.get_memory(&ctx.workspace, &sid, &mid).await.map_err(map_store)?;
        Ok(project_head(&record, view))
    }

    /// `GET /v1/memory_stores/{id}/memories` — directory-style browse.
    ///
    /// `path_prefix` scopes the subtree (default `/`); `order_by` is
    /// `created_at` (newest first, default) or `path` (ascending); `depth`
    /// (relative to the prefix) collapses deeper paths into `memory_prefix`
    /// entries. Content is always omitted (basic view). Matches the live wire
    /// shape `{data, prefixes: []}`.
    pub async fn list_memories(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        path_prefix: Option<&str>,
        order_by: Option<&str>,
        depth: Option<u32>,
    ) -> Result<MemoryListPage, ApiError> {
        let sid = parse_store_id(raw_store_id)?;
        let order = MemoryOrder::parse(order_by)?;
        let prefix = normalize_prefix(path_prefix);
        let records =
            self.store.list_memories(&ctx.workspace, &sid, Some(&prefix)).await.map_err(map_store)?;

        let mut memories: Vec<Memory> = records.iter().map(|r| project_head(r, View::Basic)).collect();
        match order {
            MemoryOrder::Path => memories.sort_by(|a, b| a.path.cmp(&b.path)),
            MemoryOrder::CreatedAt => memories.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        }

        let data = match depth {
            None => memories.into_iter().map(|m| MemoryListItem::Memory(Box::new(m))).collect(),
            Some(d) => collapse_depth(memories, &prefix, d, order),
        };
        Ok(MemoryListPage { data, prefixes: Vec::new() })
    }

    /// Resolve a live memory by its path, with full content — used by the
    /// dream review gate to compare a candidate against current memory.
    pub async fn content_at_path(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        path: &str,
    ) -> Result<Option<Memory>, ApiError> {
        let sid = parse_store_id(raw_store_id)?;
        let Some(mid) = self.store.find_by_path(&ctx.workspace, &sid, path).await.map_err(map_store)? else {
            return Ok(None);
        };
        let record = self.store.get_memory(&ctx.workspace, &sid, &mid).await.map_err(map_store)?;
        Ok(Some(project_head(&record, View::Full)))
    }
}

fn project_head(record: &MemoryRecord, view: View) -> Memory {
    let mut head = record.head.clone();
    head.content = match view {
        View::Full => record.resolve(&VersionSelector::Latest).and_then(|v| v.blob.clone()),
        View::Basic => None,
    };
    head
}
