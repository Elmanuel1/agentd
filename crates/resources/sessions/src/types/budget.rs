use serde::{Deserialize, Serialize};
pub use shared::MonetaryAmount;
use shared::{ApiError, Patch};

use super::{Session, SessionStatus};

/// A session's spending limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Budget {
    /// Stop issuing model requests once the list cost reaches `max_list_cost`.
    Limit {
        /// The most the session may cost at list price.
        max_list_cost: MonetaryAmount,
    },
}

impl Budget {
    /// The limit in minor units, after checking its amount and currency.
    pub fn max_minor_units(&self) -> Result<u64, ApiError> {
        let Self::Limit { max_list_cost } = self;
        max_list_cost.minor_units("budget.max_list_cost")
    }
}

/// A budget change the API refuses, with the reason Anthropic names.
fn rejected(reason: &str, message: &str) -> ApiError {
    let mut e = ApiError::invalid_request(message);
    e.extra.insert("reason".into(), reason.into());
    e
}

impl Session {
    /// Replace or remove the budget: a session created without one never gets
    /// one, and a replacement must stay above the list cost so far.
    pub fn change_budget(&mut self, patch: Patch<Budget>) -> Result<(), ApiError> {
        if matches!(patch, Patch::Keep) {
            return Ok(());
        }
        if self.status == SessionStatus::Terminated {
            return Err(ApiError::conflict_invalid_request(
                "cannot change the budget of a terminated session",
            ));
        }
        let Patch::Set(budget) = patch else {
            self.budget = None;
            return Ok(());
        };
        let max = budget.max_minor_units()?;
        if self.budget.is_none() {
            return Err(rejected(
                "budget_create_only",
                "budget: a budget can only be set when the session is created, and a removed budget cannot be re-added",
            ));
        }
        if self.usage.cost_unmeasured() {
            return Err(rejected(
                "model_not_budgetable",
                "budget: the session ran a model with no list price, so a budget cannot measure it; remove the budget instead",
            ));
        }
        let spent = self.usage.list_cost.as_ref().map_or(Ok(0), |c| c.minor_units("usage.list_cost"))?;
        if max <= spent {
            return Err(rejected(
                "budget_not_raised",
                &format!("budget: max_list_cost must be above the session's list cost so far ({spent} minor units)"),
            ));
        }
        self.budget = Some(budget);
        Ok(())
    }
}
