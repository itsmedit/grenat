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
/// `max_chars` characters each (a longer text goes alone).
pub(crate) fn chunks(inputs: &[String], api: &EmbeddingApi) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let (mut start, mut chars) = (0, 0);
    for (i, input) in inputs.iter().enumerate() {
        let size = input.chars().count();
        if i > start && (i - start == api.max_inputs || chars + size > api.max_chars) {
            out.push(start..i);
            (start, chars) = (i, 0);
        }
        chars += size;
    }
    if start < inputs.len() {
        out.push(start..inputs.len());
    }
    out
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
        let api = EmbeddingApi { dimensions_field: "dimensions", max_inputs: 2, max_chars: 10 };
        let texts = |sizes: &[usize]| sizes.iter().map(|n| "x".repeat(*n)).collect::<Vec<_>>();
        assert_eq!(chunks(&texts(&[1, 1, 1, 1, 1]), &api), [0..2, 2..4, 4..5]);
        assert_eq!(chunks(&texts(&[6, 6, 20, 1]), &api), [0..1, 1..2, 2..3, 3..4]);
        assert_eq!(chunks(&texts(&[]), &api), Vec::<Range<usize>>::new());
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
