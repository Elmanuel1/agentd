//! DynamoDB/S3 implementations of the entity store traits.
//!
//! Each entity maps its `(partition, id[, version])` onto DynamoDB
//! `(pk, sk)` and stores the wire object as JSON. The three implemented here
//! cover every pattern the remaining six reuse verbatim:
//! - [`agents`] — a **versioned** entity (SK carries the version),
//! - [`deployments`] — the **cross-replica exactly-once claim** (conditional
//!   write) + an append-only child log (runs),
//! - [`files`] — **DynamoDB metadata + S3 blob**.

pub mod agents;
pub mod deployments;
pub mod files;

/// Zero-padded version segment so DynamoDB's lexical `sk` order == numeric
/// version order.
pub(crate) fn vseg(version: u64) -> String {
    format!("{version:020}")
}
