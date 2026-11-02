//! Files, skills and outputs in memory.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use shared::{ApiError, AuthContext};

use crate::worker::{SessionAssets, SkillFiles};

/// Assets in memory: files and skills by id, and the outputs saved.
#[derive(Debug, Default, Clone)]
pub struct FakeAssets {
    /// File bytes by id.
    pub files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    /// Skills by id.
    pub skills: Arc<Mutex<BTreeMap<String, SkillFiles>>>,
    /// The environment config every lookup returns.
    pub environment: Arc<Mutex<serde_json::Value>>,
    /// The egress setup every session gets.
    pub egress: Arc<Mutex<crate::worker::SandboxEgress>>,
    /// Each output saved.
    pub outputs: Arc<Mutex<Vec<SavedOutput>>>,
}

/// An output the loop saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedOutput {
    /// The file name.
    pub filename: String,
    /// The bytes.
    pub bytes: Vec<u8>,
    /// The earlier file it replaced.
    pub replaced: Option<String>,
}

#[async_trait]
impl SessionAssets for FakeAssets {
    async fn file(&self, _ctx: &AuthContext, file_id: &str) -> Result<Vec<u8>, ApiError> {
        self.files.lock().get(file_id).cloned().ok_or_else(|| ApiError::not_found("file not found"))
    }

    async fn skill(
        &self,
        _ctx: &AuthContext,
        skill_id: &str,
        _version: &str,
    ) -> Result<SkillFiles, ApiError> {
        self.skills.lock().get(skill_id).cloned().ok_or_else(|| ApiError::not_found("skill not found"))
    }

    async fn egress(
        &self,
        _ctx: &AuthContext,
        _session: &str,
    ) -> Result<crate::worker::SandboxEgress, ApiError> {
        Ok(self.egress.lock().clone())
    }

    async fn environment(&self, _ctx: &AuthContext, _id: &str) -> Result<serde_json::Value, ApiError> {
        Ok(self.environment.lock().clone())
    }

    async fn save_output(
        &self,
        _ctx: &AuthContext,
        _session: &str,
        filename: &str,
        bytes: Vec<u8>,
        previous: Option<&str>,
    ) -> Result<String, ApiError> {
        let mut out = self.outputs.lock();
        out.push(SavedOutput { filename: filename.to_owned(), bytes, replaced: previous.map(str::to_owned) });
        Ok(format!("file_{:024}", out.len()))
    }
}
