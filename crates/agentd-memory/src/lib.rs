//! agentd-memory — Memory Stores, Memories, and Memory Versions.
//!
//! Wire-compatible with the live API (all shapes probed):
//! - memory head carries `memory_version_id` — a moving pointer to the
//!   current version,
//! - every mutation appends an immutable `memver_…` record
//!   (`operation: created|modified|deleted`, `created_by` actor),
//! - create at an occupied path → 409 `memory_path_conflict_error`,
//! - update is **POST** to the memory id (PATCH/PUT → 405 — enforced by the
//!   router's method table),
//! - content ≤ 102400 bytes; optimistic concurrency via
//!   `precondition.content_sha256`; versions support `redact`.
//!
//! **Deliberate extension** (the promote-don't-replace design): the head
//! pointer is a movable ref — [`service::MemoriesService::rollback`] repoints
//! `latest` to an earlier version in O(1) without rewriting content, and
//! [`store::VersionSelector`] lets a session resolve `latest` or pin a
//! specific version at session-create. The dream review gate *gates version
//! creation*; adoption of a new `latest` by the next session IS promotion.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod routes;
pub mod service;
pub mod store;
pub mod types;

pub use service::MemoriesService;
pub use store::{MemoryPersistence, VersionSelector};
pub use types::{Memory, MemoryId, MemoryStoreEntity, MemoryStoreId, MemoryVersion, MemoryVersionId};
