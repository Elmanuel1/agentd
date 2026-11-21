//! Swappable external ports. Each backend implements one trait and is chosen
//! by config at the composition root; the in-memory fakes make every consumer
//! testable.

pub mod blobs;
pub mod broadcast;
pub mod changes;
pub mod fire;
pub mod history;
pub mod keys;
pub mod rate;
pub mod scheduler;
pub mod secrets;

/// The future a port's `dyn` trait returns: native `async fn` in a trait cannot be called through `dyn`.
pub use futures::future::BoxFuture;

pub use scheduler::{
    FakeScheduler, FireId, FireTarget, ScheduleChange, ScheduleSpec, ScheduleState, SchedulerError,
    SchedulerPort, TenantRef,
};
