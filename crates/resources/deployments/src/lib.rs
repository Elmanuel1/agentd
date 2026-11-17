//! deployments — Deployments (cron → sessions) + Deployment Runs.
//!
//! Live-verified wire (deployment fixture): compact `agent` ref
//! `{id,type,version}`, `schedule.upcoming_runs_at` with exactly 5 entries,
//! tz-correct fire times (`0 0 1 1 *` NY → `2027-01-01T05:00:00Z`),
//! `paused_reason: {type:"manual"}` after pause.
//!
//! Fire contract (the fire inbox, or `/internal/fire`):
//! - a fire of one `(deployment, scheduled_at)` may arrive more than once;
//!   its run and session ids come from that pair, so a repeat finds what the
//!   first made instead of making another,
//! - business failures (rate-limited, agent/env archived, vault missing)
//!   are recorded as error runs and return success to the scheduler,
//! - a missed tick is dropped (no backfill).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod fire;
pub mod routes;
pub mod run_error;
pub mod service;
pub mod store;
pub mod types;

use std::sync::Arc;

use shared::Module;
use shared::beta::BetaGate;

pub use fire::DeploymentFire;
pub use run_error::RunError;
pub use service::{DeploymentDeps, DeploymentsService};
pub use store::DeploymentsStore;
pub use types::{Deployment, DeploymentId, DeploymentRun, DeploymentRunId};

/// The deployments module: its routes, behind the Managed Agents beta, and
/// its scheduled fires.
pub fn module(service: Arc<DeploymentsService>) -> Module {
    Module::new(routes::router(), service.clone())
        .with_beta(BetaGate::managed(&["/v1/deployments", "/v1/deployment_runs"]))
        .with_fire_handler(Arc::new(DeploymentFire::new(service)))
}
