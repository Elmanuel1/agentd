//! shared — shared kernel for the agentd control plane.
//!
//! Everything in this crate but [`net`] is pure (no I/O): wire primitives, ID grammar,
//! the error envelope, validation limits with exact wire error strings,
//! the cron/timezone schedule engine, the swappable ports ([`ports`]), and
//! the [`module::Module`] each resource contributes to the app.
//!
//! Wire shapes in this crate are ground truth captured from the live
//! Anthropic Managed Agents API (`anthropic-beta: managed-agents-2026-04-01`),
//! not from documentation. Where the two disagree, the fixtures win.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod actor;
pub mod audit;
pub mod beta;
pub mod charset;
pub mod content;
pub mod deleted;
pub mod error;
pub mod extract;
pub mod hash;
pub mod id;
pub mod limits;
pub mod metadata;
pub mod module;
pub mod money;
pub mod net;
pub mod page;
pub mod patch;
pub mod ports;
pub mod prices;
pub mod schedule;
pub mod scope;
pub mod tenancy;
pub mod time;
pub mod usage;

pub use actor::Actor;
pub use audit::{Writer, with_writer};
pub use content::ContentBlock;
pub use deleted::Deleted;
pub use error::{ApiError, BackendError, ErrorType};
pub use extract::{
    ApiJson, ApiJsonError, ErrorCause, NoRequestId, RouteError, current_request_id, multipart_error,
    request_too_large_response, unknown_route_response, wire_response, with_request_id,
};
pub use id::{Id, RequestId, Resource, parse_id};
pub use metadata::Metadata;
pub use module::Module;
pub use money::MonetaryAmount;
pub use page::{
    IdCursor, IdPage, ListQuery, ListSpec, Messages, Page, Pagination, UnknownParams, reject_unknown_params,
};
pub use patch::{MetadataPatch, Patch};
pub use scope::Scope;
pub use tenancy::{AdminContext, AuthContext, EnvironmentCaller, OrgId, SessionCaller, WorkspaceId};
pub use time::Timestamp;
pub use usage::Usage;
