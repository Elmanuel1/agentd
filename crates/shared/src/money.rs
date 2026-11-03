//! Money on the wire: `{"amount":"2500","currency":"USD"}`, an amount in the
//! currency's minor units as an integer string, so no float rounding applies.

use serde::{Deserialize, Serialize};

use crate::ApiError;

/// The currencies amounts are priced in.
pub const CURRENCIES: [&str; 1] = ["USD"];

/// An amount of money in minor units ("2500" is $25.00).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonetaryAmount {
    /// Minor units, an integer decimal string with no leading zeros.
    pub amount: String,
    /// Uppercase ISO-4217 code.
    pub currency: String,
}

impl MonetaryAmount {
    /// `minor` units of USD.
    #[must_use]
    pub fn usd(minor: u64) -> Self {
        Self { amount: minor.to_string(), currency: "USD".into() }
    }

    /// The amount in minor units, after checking it and its currency; `field`
    /// names the amount in the error.
    pub fn minor_units(&self, field: &str) -> Result<u64, ApiError> {
        if !CURRENCIES.contains(&self.currency.as_str()) {
            return Err(ApiError::invalid_request(format!(
                "{field}.currency: currency \"{}\" is not supported; supported: {}",
                self.currency,
                CURRENCIES.join(", ")
            )));
        }
        let canonical = !self.amount.starts_with('0') || self.amount == "0";
        match self.amount.parse::<u64>() {
            Ok(n) if canonical && self.amount.bytes().all(|b| b.is_ascii_digit()) => Ok(n),
            _ => Err(ApiError::invalid_request(format!(
                "{field}.amount: must be a whole number of minor units with no leading zeros, like \"2500\""
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_are_whole_minor_units_in_a_priced_currency() {
        assert_eq!(MonetaryAmount::usd(2500).minor_units("x").unwrap(), 2500);
        assert_eq!(MonetaryAmount::usd(0).minor_units("x").unwrap(), 0);
        for bad in ["5.00", "007", "", "-1", "+1", "1e3"] {
            let m = MonetaryAmount { amount: bad.into(), currency: "USD".into() };
            assert!(m
                .minor_units("budget.max_list_cost")
                .unwrap_err()
                .message
                .starts_with("budget.max_list_cost.amount"));
        }
        let eur = MonetaryAmount { amount: "1".into(), currency: "EUR".into() };
        assert_eq!(
            eur.minor_units("b").unwrap_err().message,
            "b.currency: currency \"EUR\" is not supported; supported: USD"
        );
    }
}
