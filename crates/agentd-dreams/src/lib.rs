//! agentd-dreams — the offline reflection layer. **No Anthropic equivalent;
//! this is the original contribution.**
//!
//! Contract (promote-don't-replace, as designed):
//! 1. A dream fires (cron via the Scheduler port, or manually).
//! 2. A [`Proposer`] reads recent outcomes/transcripts/memory and emits
//!    candidate memories with provenance + confidence.
//! 3. The [`ReviewGate`] judges each candidate against the **live** memory:
//!    evidence · conflict · novelty.
//! 4. **Pass → a new memory version is written** (advancing `latest`);
//!    the next session that resolves `latest` adopts it — that adoption IS
//!    promotion. **Fail → nothing is written**; `latest` is untouched.
//!
//! Every decision is journaled on the [`types::DreamRun`] — the audit trail
//! of what was proposed, the verdict, and which memory version resulted.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod service;
pub mod store;
pub mod types;

pub use service::{DreamsService, Proposer, ReviewGate};
pub use types::{Candidate, Dream, DreamId, DreamRun, Verdict};
