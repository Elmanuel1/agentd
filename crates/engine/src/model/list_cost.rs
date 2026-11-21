//! What a Messages API call costs at Anthropic's list prices.

use sessions::events::ModelUsage;
use shared::prices::list_price;

/// The call's list cost in US dollars, priced as the model that served it;
/// `None` when Anthropic lists no price for that model.
pub(super) fn of(model: &str, u: &ModelUsage) -> Option<f64> {
    let prompt = u.input_tokens + u.cache_creation_input_tokens + u.cache_read_input_tokens;
    let price = list_price(model, prompt)?;
    let nanos = price.nanos(
        u.input_tokens,
        u.cache_creation_input_tokens,
        u.cache_read_input_tokens,
        u.output_tokens,
    );
    Some(nanos as f64 / 1e9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_token_is_priced_at_its_rate() {
        // Haiku 4.5: $1 input, $1.25 cache write, $0.10 cache read, $5 output per MTok.
        let u = ModelUsage {
            input_tokens: 2_000,
            cache_creation_input_tokens: 4_000,
            cache_read_input_tokens: 10_000,
            output_tokens: 1_000,
        };
        let cost = of("claude-haiku-4-5-20251001", &u).unwrap();
        assert!((cost - (0.002 + 0.005 + 0.001 + 0.005)).abs() < 1e-12, "{cost}");
    }

    #[test]
    fn a_model_with_no_list_price_has_no_cost() {
        assert_eq!(of("claude-mythos-preview", &ModelUsage::default()), None);
    }
}
