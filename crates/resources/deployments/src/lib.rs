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

pub mod fire;
pub mod routes;
pub mod service;
pub mod store;
pub mod types;

use std::sync::Arc;

use shared::beta::BetaGate;
use shared::Module;

pub use fire::DeploymentFire;
pub use service::{DeploymentDeps, DeploymentsService};
pub use store::DeploymentsStore;
pub use types::{Deployment, DeploymentId, DeploymentRun, DeploymentRunId, RunError};

/// The deployments module: its routes, behind the Managed Agents beta, and
/// its scheduled fires.
pub fn module(service: Arc<DeploymentsService>) -> Module {
    Module::new(routes::router(), service.clone())
        .with_beta(BetaGate::managed(&["/v1/deployments", "/v1/deployment_runs"]))
        .with_fire_handler(Arc::new(DeploymentFire::new(service)))
}
