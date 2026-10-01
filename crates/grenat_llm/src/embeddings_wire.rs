//! Embeddings wire format — OpenAI's `POST /embeddings`, which Gemini (its
//! compatible endpoint), Mistral, Ollama and Voyage also speak — and the
//! split of many texts into requests a provider accepts.

use std::ops::Range;

use serde_json::{Value as Json, json};

use crate::catalog::EmbeddingApi;
use crate::*;

/// JSON body of a `POST /embeddings` request.
pub fn embeddings_body(request: &EmbeddingRequest, api: &EmbeddingApi) -> Json {
    let mut body = json!({"model": request.model.name, "input": request.inputs});
    if let Some(dimensions) = request.model.dimensions {
        body[api.dimensions_field] = json!(dimensions);
    }
    body
}

/// The requests `inputs` are sent in: at most `max_inputs` texts and
/// `max_tokens` tokens each, by [`estimated_tokens`], with a tenth kept as
/// a margin (a longer text goes alone).
pub(crate) fn chunks(inputs: &[String], api: &EmbeddingApi) -> Vec<Range<usize>> {
    let budget = api.max_tokens - api.max_tokens / 10;
    let mut out = Vec::new();
    let (mut start, mut tokens) = (0, 0);
    for (i, input) in inputs.iter().enumerate() {
        let size = estimated_tokens(input);
        if i > start && (i - start == api.max_inputs || tokens + size > budget) {
            out.push(start..i);
            (start, tokens) = (i, 0);
        }
        tokens += size;
    }
    if start < inputs.len() {
        out.push(start..inputs.len());
    }
    out
}

/// At least the tokens of `text`, whatever its script: two ASCII
/// characters a token (English is about four, code two), and a token for
/// each byte of other characters — a tokenizer working on bytes never
/// makes more (Chinese is one or two a character, three bytes).
pub(crate) fn estimated_tokens(text: &str) -> usize {
    let ascii = text.bytes().filter(u8::is_ascii).count();
    ascii.div_ceil(2) + (text.len() - ascii)
}

/// The vectors of an answer, in the order of the `expected` inputs.
pub(crate) fn parse_embeddings(body: &Json, expected: usize) -> Result<Embeddings, LlmError> {
    let data = body["data"].as_array().ok_or_else(|| LlmError::new("embeddings answer without `data`"))?;
    let mut items: Vec<(u64, &Json)> =
        data.iter().enumerate().map(|(i, d)| (d["index"].as_u64().unwrap_or(i as u64), &d["embedding"])).collect();
    items.sort_by_key(|(index, _)| *index);
    if items.len() != expected {
        return Err(LlmError::new(format!("{} texts sent, {} vectors received", expected, items.len())));
    }
    let mut vectors = Vec::with_capacity(items.len());
    for (index, embedding) in items {
        let vector: Option<Vec<f64>> = embedding.as_array().and_then(|xs| xs.iter().map(Json::as_f64).collect());
        match vector {
            Some(v) if !v.is_empty() => vectors.push(v),
            _ => return Err(LlmError::new(format!("the vector of text {index} is not a list of numbers"))),
        }
    }
    let usage = &body["usage"];
    let tokens = usage["prompt_tokens"].as_u64().or_else(|| usage["total_tokens"].as_u64()).unwrap_or(0);
    Ok(Embeddings {
        vectors,
        usage: Usage { input_tokens: tokens, ..Usage::default() },
        model: body["model"].as_str().unwrap_or_default().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_are_split_by_count_and_by_size() {
        // 10 tokens, 9 with the margin
        let api = EmbeddingApi { dimensions_field: "dimensions", max_inputs: 2, max_tokens: 10 };
        let texts = |sizes: &[usize]| sizes.iter().map(|n| "x".repeat(*n)).collect::<Vec<_>>();
        assert_eq!(chunks(&texts(&[1, 1, 1, 1, 1]), &api), [0..2, 2..4, 4..5]);
        assert_eq!(chunks(&texts(&[10, 10, 40, 1]), &api), [0..1, 1..2, 2..3, 3..4]);
        assert_eq!(chunks(&texts(&[8, 10]), &api).len(), 1);
        assert_eq!(chunks(&texts(&[]), &api), Vec::<Range<usize>>::new());
    }

    #[test]
    fn tokens_are_estimated_for_any_script() {
        assert_eq!(estimated_tokens("refund"), 3);
        assert_eq!(estimated_tokens("退款"), 6);
        assert_eq!(estimated_tokens("café"), 4);
        assert_eq!(estimated_tokens(""), 0);
    }

    #[test]
    fn chinese_texts_stay_under_the_providers_token_limits() {
        // 2,048 passages of 390 characters: as many tokens at least, at
        // one a character — 800,000 for OpenAI, which takes 300,000
        let passage = "退".repeat(390);
        let inputs = vec![passage; 2048];
        for (provider, limit) in [("openai", 300_000), ("voyage", 120_000)] {
            let api = crate::catalog::provider(provider).unwrap().embeddings.unwrap();
            let requests = chunks(&inputs, &api);
            assert!(requests.len() >= 3, "{provider}: {requests:?}");
            assert_eq!(requests.iter().map(ExactSizeIterator::len).sum::<usize>(), 2048);
            for range in requests {
                let characters: usize = inputs[range].iter().map(|t| t.chars().count()).sum();
                assert!(characters <= limit, "{provider}: {characters} tokens at least in a request");
            }
        }
    }

    #[test]
    fn answers_are_read_in_the_inputs_order() {
        let body = json!({
            "model": "m",
            "data": [{"index": 1, "embedding": [0.0, 1.0]}, {"index": 0, "embedding": [1.0, 0.0]}],
            "usage": {"total_tokens": 7}
        });
        let parsed = parse_embeddings(&body, 2).unwrap();
        assert_eq!(parsed.vectors, [[1.0, 0.0], [0.0, 1.0]]);
        assert_eq!(parsed.usage.input_tokens, 7);
        assert!(parse_embeddings(&body, 3).unwrap_err().message.contains("3 texts sent, 2 vectors"));
        let base64 = json!({"data": [{"index": 0, "embedding": "AAAA"}]});
        assert!(parse_embeddings(&base64, 1).unwrap_err().message.contains("not a list of numbers"));
        assert!(parse_embeddings(&json!({}), 1).is_err());
    }
}
