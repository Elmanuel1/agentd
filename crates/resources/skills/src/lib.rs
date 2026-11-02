//! skills — the standard Anthropic **Skills API** (`/v1/skills`).
//!
//! NOTE: this is NOT a managed-agents resource. It's the shared Skills API on
//! its own beta header **`skills-2025-10-02`** (probed live 2026-07-06). A
//! custom skill is a zip bundle with a single top-level folder containing
//! `SKILL.md`; the API extracts the folder name (`directory`) and the
//! `name`/`description` from the SKILL.md YAML frontmatter. Bundles are stored
//! in object storage; only metadata lives in the DB. Agents reference skills by
//! id via the `skills[]` field (see `agents::SkillRef`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod routes;

mod bundle;
mod service;
mod store;
mod types;

use std::sync::Arc;

use shared::module::BodyLimit;
use shared::Module;

pub use bundle::{parse_bundle, Bundle, UploadFile};
pub use service::SkillsService;
pub use store::{SkillVersionsStore, SkillsStore, StoreError};
pub use types::{
    beta_version_id, Skill, SkillId, SkillListPage, SkillResource, SkillSource, SkillTag, SkillVersion,
    SkillVersionId, SkillVersionResource, SkillVersionTag, VersionRef,
};

/// The skills module: its routes, with the Skills API's 30 MB body cap.
pub fn module(service: Arc<SkillsService>) -> Module {
    Module::new(routes::router(), service).with_body_limit(BodyLimit { api: "Skills", max_mb: 30 })
}
