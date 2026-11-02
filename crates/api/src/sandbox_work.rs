//! Each session's sandbox runs a worker on a work item: the work it is
//! issued at launch, and where the worker reaches agentd.

use std::collections::BTreeMap;
use std::sync::Arc;

use sessions::types::SessionId;
use sessions::SandboxWork;
use shared::AuthContext;
use work::WorkService;

/// [`SandboxWork`] over the work service.
#[derive(Debug, Clone)]
pub struct WorkIssuer {
    /// The work items.
    pub work: Arc<WorkService>,
    /// agentd's API as the sandbox reaches it.
    pub api_url: String,
}

impl SandboxWork for WorkIssuer {
    fn issue(
        &self,
        ctx: &AuthContext,
        environment_id: &str,
        session: &SessionId,
    ) -> BTreeMap<String, String> {
        let (work, secret) = self.work.issue(ctx, environment_id, session.as_str(), true);
        BTreeMap::from([
            ("AGENTD_API_URL".to_owned(), self.api_url.clone()),
            ("AGENTD_ENVIRONMENT_ID".to_owned(), environment_id.to_owned()),
            ("AGENTD_WORK_ID".to_owned(), work.id.as_str().to_owned()),
            ("AGENTD_WORK_SECRET".to_owned(), secret),
        ])
    }

    fn end(&self, session: &SessionId) {
        self.work.end(session.as_str());
    }
}
