//! shared — shared kernel for the agentd control plane.
//!
//! Everything in this crate is pure (no I/O): wire primitives, ID grammar,
//! the error envelope, validation limits with exact wire error strings,
//! the cron/timezone schedule engine, the swappable ports ([`ports`]), and
//! the [`module::Module`] each resource contributes to the app.
//!
//! Wire shapes in this crate are ground truth captured from the live
//! Anthropic Managed Agents API (`anthropic-beta: managed-agents-2026-04-01`),
//! not from documentation. Where the two disagree, the fixtures win.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod actor;
pub mod beta;
pub mod content;
pub mod deleted;
pub mod error;
pub mod extract;
pub mod hash;
pub mod id;
pub mod limits;
pub mod metadata;
pub mod module;
pub mod page;
pub mod patch;
pub mod ports;
pub mod schedule;
pub mod scope;
pub mod tenancy;
pub mod time;
pub mod usage;

pub use actor::Actor;
pub use content::ContentBlock;
pub use deleted::Deleted;
pub use error::{ApiError, BackendError, ErrorType};
pub use extract::{
    multipart_error, request_too_large_response, unknown_route_response, wire_response, with_request_id,
    ApiJson, ApiJsonError, NoRequestId, RouteError,
};
pub use id::{parse_id, Id, RequestId, Resource};
pub use metadata::Metadata;
pub use module::Module;
pub use page::{
    reject_unknown_params, IdCursor, IdPage, ListSpec, Messages, Page, Pagination, UnknownParams,
};
pub use patch::{MetadataPatch, Patch};
pub use scope::Scope;
pub use tenancy::{AuthContext, OrgId, WorkspaceId};
pub use time::Timestamp;
pub use usage::Usage;
