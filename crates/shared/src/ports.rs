//! Swappable external ports. Each backend implements one trait and is chosen
//! by config at the composition root; the in-memory fakes make every consumer
//! testable.

pub mod blobs;
pub mod fire;
pub mod keys;
pub mod lease;
pub mod runtime;
pub mod sandbox;
pub mod scheduler;
pub mod secrets;

pub use runtime::{FakeRuntime, JobKind, JobSpec, Runtime, RuntimeError, RuntimeHandle, TaskState};
pub use scheduler::{
    FakeScheduler, FireId, FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort, TenantRef,
};
