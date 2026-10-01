//! Per-model prices and the cost of a call, prompt caching included: a token
//! read from the cache costs a tenth of the input price (less on some
//! models), a token written to it a quarter more — twice the input price
//! when it is kept for an hour.

use crate::*;

/// Dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// A token read from the prompt cache (a hit).
    pub cache_read: f64,
    /// A token written to the prompt cache: for five minutes (Anthropic),
    /// or as long as the provider keeps it (OpenAI).
    pub cache_write: f64,
    /// A token written to the prompt cache for an hour (Anthropic).
    pub cache_write_1h: f64,
}

impl Price {
    /// The usual cache prices: reads at 0.1 times the input price, writes
    /// at 1.25 times (2 times for an hour).
    pub fn new(input: f64, output: f64) -> Price {
        Price::with_reads(input, output, 0.1)
    }

    /// Cache reads at `read` times the input price.
    fn with_reads(input: f64, output: f64, read: f64) -> Price {
        Price { input, output, cache_read: input * read, cache_write: input * 1.25, cache_write_1h: input * 2.0 }
    }
}

/// The price of an Anthropic model, by the longest prefix of its name
/// listed first (Anthropic's pricing page, October 2026).
pub(crate) fn price_of(model: &str) -> Option<Price> {
    // (prefix, input, output, cache reads as a share of the input price)
    const PRICES: &[(&str, f64, f64, f64)] = &[
        ("claude-fable-5-1", 10.0, 50.0, 0.025),
        ("claude-mythos-5-1", 10.0, 50.0, 0.025),
        ("claude-fable-5", 10.0, 50.0, 0.1),
        ("claude-mythos-5", 10.0, 50.0, 0.1),
        ("claude-opus-5-5", 4.0, 20.0, 0.05),
        ("claude-opus-5", 5.0, 25.0, 0.1),
        // Opus 4 and 4.1 (retired from the API) cost three times what 4.5 and later do
        ("claude-opus-4-1", 15.0, 75.0, 0.1),
        ("claude-opus-4-0", 15.0, 75.0, 0.1),
        ("claude-opus-4-2025", 15.0, 75.0, 0.1),
        ("claude-opus-4", 5.0, 25.0, 0.1),
        ("claude-sonnet-5", 2.0, 10.0, 0.1),
        ("claude-sonnet-4", 3.0, 15.0, 0.1),
        ("claude-haiku-4-5", 1.0, 5.0, 0.1),
        ("claude-3-5-haiku", 0.8, 4.0, 0.1),
    ];
    PRICES
        .iter()
        .find(|(prefix, ..)| model.starts_with(prefix))
        .map(|&(_, input, output, read)| Price::with_reads(input, output, read))
}

/// Dollars per million input and output tokens, for a model so billed.
type TokenPrices = Option<(f64, f64)>;

/// The price of OpenAI's transcription models (its pricing page, October
/// 2026): (prefix, dollars per minute — an estimate for those billed by
/// tokens —, dollars per million input and output tokens, if so billed).
const TRANSCRIPTION: &[(&str, f64, TokenPrices)] = &[
    ("gpt-transcribe", 0.0045, None),
    ("gpt-4o-mini-transcribe", 0.003, Some((1.25, 5.0))),
    ("gpt-4o-transcribe", 0.006, Some((2.5, 10.0))),
    ("whisper", 0.006, None),
];

/// The cost of a transcription: by its tokens where the model is billed by
/// tokens and the answer counts them, else by its minutes; `None` when
/// the model's price or the audio's duration is unknown.
pub fn transcription_cost(model: &str, transcript: &Transcript) -> Option<f64> {
    let &(_, per_minute, tokens) = TRANSCRIPTION.iter().find(|(prefix, ..)| model.starts_with(prefix))?;
    match tokens {
        Some((input, output)) if transcript.usage.total_tokens() > 0 => {
            Some(cost_at(Price::new(input, output), &transcript.usage))
        }
        _ => transcript.seconds.map(|seconds| seconds / 60.0 * per_minute),
    }
}

/// Cost of a call in dollars; `None` for a model with an unknown price.
pub fn cost_usd(model: &str, usage: &Usage) -> Option<f64> {
    Some(cost_at(price_of(model)?, usage))
}

/// Cost of a call at `price`.
pub fn cost_at(price: Price, usage: &Usage) -> f64 {
    let hour = usage.cache_creation_1h_input_tokens.min(usage.cache_creation_input_tokens);
    let minutes = usage.cache_creation_input_tokens - hour;
    let dollars = usage.input_tokens as f64 * price.input
        + usage.output_tokens as f64 * price.output
        + usage.cache_read_input_tokens as f64 * price.cache_read
        + minutes as f64 * price.cache_write
        + hour as f64 * price.cache_write_1h;
    dollars / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, written: u64, written_1h: u64, read: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: written,
            cache_read_input_tokens: read,
            cache_creation_1h_input_tokens: written_1h,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn cache_reads_and_writes_follow_anthropic_s_table() {
        const M: u64 = 1_000_000;
        // (model, 5m write, 1h write, read) in dollars per million tokens
        let table = [
            ("claude-fable-5-1", 12.5, 20.0, 0.25),
            ("claude-mythos-5-1", 12.5, 20.0, 0.25),
            ("claude-fable-5", 12.5, 20.0, 1.0),
            ("claude-opus-5-5", 5.0, 8.0, 0.20),
            ("claude-opus-5", 6.25, 10.0, 0.50),
            ("claude-opus-4-8", 6.25, 10.0, 0.50),
            ("claude-opus-4-1-20250805", 18.75, 30.0, 1.50),
            ("claude-opus-4-20250514", 18.75, 30.0, 1.50),
            ("claude-sonnet-5-5", 2.5, 4.0, 0.20),
            ("claude-sonnet-4-6", 3.75, 6.0, 0.30),
            ("claude-haiku-4-5-20251001", 1.25, 2.0, 0.10),
            ("claude-3-5-haiku-20241022", 1.0, 1.6, 0.08),
        ];
        for (model, write, write_1h, read) in table {
            let at = |u: Usage| cost_usd(model, &u).unwrap();
            assert!(close(at(usage(0, M, 0, 0, 0)), write), "{model} 5m write: {}", at(usage(0, M, 0, 0, 0)));
            assert!(close(at(usage(0, M, M, 0, 0)), write_1h), "{model} 1h write");
            assert!(close(at(usage(0, 0, 0, M, 0)), read), "{model} read: {}", at(usage(0, 0, 0, M, 0)));
        }
    }

    #[test]
    fn transcriptions_are_priced_by_tokens_or_by_minutes() {
        let transcript = |input: u64, output: u64, seconds: Option<f64>| Transcript {
            text: String::new(),
            segments: Vec::new(),
            language: None,
            usage: usage(input, 0, 0, 0, output),
            seconds,
            model: String::new(),
        };
        let ten_minutes = transcript(0, 0, Some(600.0));
        assert!(close(transcription_cost("whisper-1", &ten_minutes).unwrap(), 0.06));
        assert!(close(transcription_cost("gpt-transcribe", &ten_minutes).unwrap(), 0.045));
        // tokens when counted: 1M in at $2.50, 100k out at $10
        let counted = transcript(1_000_000, 100_000, Some(600.0));
        assert!(close(transcription_cost("gpt-4o-transcribe-diarize", &counted).unwrap(), 3.5));
        assert!(close(transcription_cost("gpt-4o-mini-transcribe", &counted).unwrap(), 1.75));
        assert!(close(transcription_cost("gpt-4o-mini-transcribe", &ten_minutes).unwrap(), 0.03));
        assert_eq!(transcription_cost("whisper-1", &transcript(0, 0, None)), None);
        assert_eq!(transcription_cost("voxtral", &ten_minutes), None);
    }

    #[test]
    fn a_call_adds_up_every_kind_of_token() {
        // Opus 5: 1,000 uncached at $5, 2,000 written for 5 min at $6.25, 1,000
        // for an hour at $10, 10,000 read at $0.50, 500 out at $25
        let cost = cost_usd("claude-opus-5", &usage(1_000, 3_000, 1_000, 10_000, 500)).unwrap();
        assert!(close(cost, (5_000.0 + 12_500.0 + 10_000.0 + 5_000.0 + 12_500.0) / 1_000_000.0), "{cost}");
        // a price given for a model: reads at a tenth, writes a quarter more, unless said
        let given = Price::new(2.0, 8.0);
        assert_eq!((given.cache_read, given.cache_write, given.cache_write_1h), (0.2, 2.5, 4.0));
        let cheaper_reads = Price { cache_read: 0.5, ..given };
        assert!(close(cost_at(cheaper_reads, &usage(0, 0, 0, 1_000_000, 0)), 0.5));
    }
}
