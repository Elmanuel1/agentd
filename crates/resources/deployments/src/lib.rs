//! deployments — Deployments (cron → sessions) + Deployment Runs.
//!
//! Live-verified wire (deployment fixture): compact `agent` ref
//! `{id,type,version}`, `schedule.upcoming_runs_at` with exactly 5 entries,
//! tz-correct fire times (`0 0 1 1 *` NY → `2027-01-01T05:00:00Z`),
//! `paused_reason: {type:"manual"}` after pause.
//!
//! Fire contract (`/internal/fire`, called by the scheduler backend):
//! - **no retry** — the scheduler fires once per `(target, scheduled_at)`,
//! - business failures (rate-limited, agent/env archived, vault missing)
//!   are recorded as error runs and return success to the scheduler,
//! - a missed tick is dropped (no backfill).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod routes;
pub mod service;
pub mod store;
pub mod types;

pub use service::{DeploymentDeps, DeploymentsService};
pub use store::DeploymentsStore;
pub use types::{Deployment, DeploymentId, DeploymentRun, DeploymentRunId, RunError};
