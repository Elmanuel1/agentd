//! agentd-skills — the standard Anthropic **Skills API** (`/v1/skills`).
//!
//! NOTE: this is NOT a managed-agents resource. It's the shared Skills API on
//! its own beta header **`skills-2025-10-02`** (probed live 2026-07-06). A
//! custom skill is a zip bundle with a single top-level folder containing
//! `SKILL.md`; the API extracts the folder name (`directory`) and the
//! `name`/`description` from the SKILL.md YAML frontmatter. Bundles are stored
//! in object storage; only metadata lives in the DB. Agents reference skills by
//! id via the `skills[]` field (see `agentd-agents::SkillRef`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod routes;

mod bundle;
mod service;
mod store;
mod types;

pub use bundle::{parse_bundle, Bundle};
pub use service::SkillsService;
pub use store::{SkillsStore, StoreError};
pub use types::{
    Skill, SkillId, SkillListPage, SkillResource, SkillSource, SkillTag, SkillVersion, SkillVersionId,
    SkillVersionResource, SkillVersionTag,
};
