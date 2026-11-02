use shared::{ApiError, AuthContext, Page, Pagination};

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

/// What a memories list asks for, besides the page.
#[derive(Debug, Clone, Copy)]
pub struct MemoryListQuery<'a> {
    /// Only memories below this prefix.
    pub path_prefix: Option<&'a str>,
    /// `created_at` (default) or `path`.
    pub order_by: Option<&'a str>,
    /// `0` lists every memory; `n` groups those deeper than `n` levels.
    pub depth: u32,
    /// Whether items include content.
    pub view: View,
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

    /// `GET /v1/memory_stores/{id}/memories`: one page of a store's memories
    /// below `path_prefix` (default `/`), ordered by `created_at` (newest
    /// first, default) or `path`. Depth `0` (the default) lists every memory;
    /// depth `n` lists memories up to `n` levels down and groups deeper ones
    /// into `memory_prefix` items (live: `1` lists direct children).
    /// `view=full` includes content. Live wire shape `{data, prefixes: []}`.
    pub async fn list_memories(
        &self,
        ctx: &AuthContext,
        raw_store_id: &str,
        query: &MemoryListQuery<'_>,
        page: &Pagination,
    ) -> Result<MemoryListPage, ApiError> {
        let MemoryListQuery { path_prefix, order_by, depth, view } = *query;
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

        let items = match depth {
            0 => memories.into_iter().map(|m| MemoryListItem::Memory(Box::new(m))).collect(),
            d => collapse_depth(memories, &prefix, d, order),
        };
        let mut page = Page::paginate(items, page);
        if view == View::Full {
            // The list read carries heads only; a full page (at most 20) reads
            // each head's content concurrently.
            let reads = page.data.iter_mut().filter_map(|item| match item {
                MemoryListItem::Memory(m) => Some(async {
                    let (_, v) = self.store.get_version(&ctx.workspace, &sid, &m.memory_version_id).await?;
                    m.content = v.blob;
                    Ok::<_, crate::store::StoreError>(())
                }),
                MemoryListItem::Prefix(_) => None,
            });
            futures::future::try_join_all(reads).await.map_err(map_store)?;
        }
        Ok(MemoryListPage { data: page.data, prefixes: Vec::new(), next_page: page.next_page })
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
