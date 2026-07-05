//! agentd-core — shared kernel for the agentd control plane.
//!
//! Everything in this crate is pure (no I/O): wire primitives, ID grammar,
//! the error envelope, validation limits with exact wire error strings,
//! the cron/timezone schedule engine, and the swappable ports
//! ([`ports::Runtime`], [`ports::SchedulerPort`]).
//!
//! Wire shapes in this crate are ground truth captured from the live
//! Anthropic Managed Agents API (`anthropic-beta: managed-agents-2026-04-01`),
//! not from documentation. Where the two disagree, the fixtures win.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod actor;
pub mod content;
pub mod error;
pub mod hash;
pub mod id;
pub mod limits;
pub mod metadata;
pub mod page;
pub mod ports;
pub mod schedule;
pub mod scope;
pub mod tenancy;
pub mod time;
pub mod usage;

pub use actor::Actor;
pub use content::ContentBlock;
pub use error::{ApiError, ErrorType};
pub use id::{Id, RequestId, Resource};
pub use metadata::Metadata;
pub use page::{Page, Pagination};
pub use scope::Scope;
pub use tenancy::{AuthContext, OrgId, WorkspaceId};
pub use time::Timestamp;
pub use usage::Usage;
