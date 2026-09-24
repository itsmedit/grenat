//! Per-model prices and the cost of a call.

use crate::*;

/// Price in dollars per million tokens (input, output).
pub(crate) fn price_per_mtok(model: &str) -> Option<(f64, f64)> {
    const PRICES: &[(&str, f64, f64)] = &[
        ("claude-fable-5", 10.0, 50.0),
        ("claude-mythos-5", 10.0, 50.0),
        ("claude-opus-5-5", 4.0, 20.0),
        ("claude-opus-5", 5.0, 25.0),
        ("claude-opus-4", 5.0, 25.0),
        ("claude-sonnet-5", 2.0, 10.0),
        ("claude-sonnet-4", 3.0, 15.0),
        ("claude-haiku-4-5", 1.0, 5.0),
    ];
    PRICES.iter().find(|(prefix, ..)| model.starts_with(prefix)).map(|&(_, input, output)| (input, output))
}

/// Cost of a call in dollars; `None` for a model with an unknown price.
pub fn cost_usd(model: &str, usage: &Usage) -> Option<f64> {
    let (input, output) = price_per_mtok(model)?;
    let input_equiv = usage.input_tokens as f64
        + usage.cache_creation_input_tokens as f64 * 1.25
        + usage.cache_read_input_tokens as f64 * 0.1;
    Some((input_equiv * input + usage.output_tokens as f64 * output) / 1_000_000.0)
}
