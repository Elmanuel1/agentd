use serde::{Deserialize, Serialize};

/// An amount of money: a decimal string and its currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonetaryAmount {
    /// The amount, as a decimal string.
    pub amount: String,
    /// The currency code.
    pub currency: String,
}

/// A session's spending limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Budget {
    /// Stop at a list-price cost.
    Limit {
        /// The most the session may cost at list price.
        max_list_cost: MonetaryAmount,
    },
}
