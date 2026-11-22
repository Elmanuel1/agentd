//! Keeps this replica's list prices current from a model list
//! (`AGENTD_MODEL_PRICES_URL`), read on the [`super::daily`] schedule. Until
//! a read succeeds, the built-in table prices each model.

use shared::prices::{Feed, install};

use super::daily;

impl daily::Listed for Feed {
    fn is_empty(&self) -> bool {
        Feed::is_empty(self)
    }
}

/// Read the list at `url` now and on the daily schedule, for as long as the
/// process runs.
pub fn spawn(url: String) -> tokio::task::JoinHandle<()> {
    let read = move |http: reqwest::Client| {
        let req = http.get(&url);
        async move { daily::json(req).await.map(|list| Feed::parse(&list)) }
    };
    daily::spawn("prices", read, install)
}
