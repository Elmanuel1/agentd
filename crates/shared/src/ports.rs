//! Swappable external ports. Each backend implements one trait and is chosen
//! by config at the composition root; the in-memory fakes make every consumer
//! testable.

pub mod blobs;
pub mod fire;
pub mod keys;
pub mod scheduler;
pub mod secrets;

pub use scheduler::{
    FakeScheduler, FireId, FireTarget, ScheduleChange, ScheduleSpec, ScheduleState, SchedulerError,
    SchedulerPort, TenantRef,
};
