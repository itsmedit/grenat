//! Deterministic embeddings for tests: given vectors for given texts, and
//! for any other text a vector hashed from its words — texts sharing words
//! point the same way, so that a nearest search finds what a test expects
//! without a model.

use std::collections::HashMap;

use crate::*;

pub struct FakeEmbeddings {
    label: String,
    dimensions: usize,
    known: HashMap<String, Vec<f64>>,
}

impl FakeEmbeddings {
    /// `label` names the fake in errors (the model it stands for).
    pub fn new(label: impl Into<String>, dimensions: usize, known: HashMap<String, Vec<f64>>) -> FakeEmbeddings {
        FakeEmbeddings { label: label.into(), dimensions: dimensions.max(1), known }
    }

    /// The vector of `text`: its own if given, else hashed from its words.
    pub fn vector(&self, text: &str) -> Vec<f64> {
        if let Some(vector) = self.known.get(text) {
            return vector.clone();
        }
        hashed(text, self.dimensions)
    }
}

/// A bag of words: each lowercase word adds ±1 to a dimension chosen by its
/// hash (FNV-1a), then the vector is scaled to length 1.
fn hashed(text: &str, dimensions: usize) -> Vec<f64> {
    let mut vector = vec![0.0; dimensions];
    let lower = text.to_lowercase();
    for word in lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let hash =
            word.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
        let sign = if hash >> 63 == 0 { 1.0 } else { -1.0 };
        vector[(hash % dimensions as u64) as usize] += sign;
    }
    let norm = vector.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > 0.0 {
        vector.iter_mut().for_each(|x| *x /= norm);
    }
    vector
}

impl Provider for FakeEmbeddings {
    fn complete(&self, _request: &Request) -> Result<Response, LlmError> {
        Err(LlmError::new(format!("the fake of {} only makes embeddings", self.label)))
    }

    fn embed(&self, request: &EmbeddingRequest) -> Result<Embeddings, LlmError> {
        Ok(Embeddings {
            vectors: request.inputs.iter().map(|text| self.vector(text)).collect(),
            usage: Usage::default(),
            model: "fake".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f64], b: &[f64]) -> f64 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn texts_sharing_words_are_close_and_given_vectors_win() {
        let known = HashMap::from([("hello".to_string(), vec![1.0, 0.0])]);
        let fake = FakeEmbeddings::new("`:docs`", 64, known);
        let refund = fake.vector("How do I get a refund?");
        assert_eq!(refund.len(), 64);
        assert!((cosine(&refund, &refund) - 1.0).abs() < 1e-9);
        assert_eq!(refund, fake.vector("how do i get a REFUND"));
        let close = cosine(&refund, &fake.vector("Refund policy: how to get a refund"));
        let far = cosine(&refund, &fake.vector("Exporting invoices to CSV"));
        assert!(close > far, "{close} <= {far}");
        assert_eq!(fake.vector("hello"), [1.0, 0.0]);
        assert_eq!(fake.vector("!!!"), vec![0.0; 64]);
        let model = ModelConfig::new("openai", "text-embedding-3-small");
        let request = EmbeddingRequest { model: &model, inputs: vec!["a".into(), "hello".into()] };
        let answer = fake.embed(&request).unwrap();
        assert_eq!(answer.vectors.len(), 2);
        assert_eq!(answer.vectors[1], [1.0, 0.0]);
        let chat =
            Request { model: &model, system: None, messages: Vec::new(), tools: Vec::new(), output_schema: None };
        assert!(fake.complete(&chat).unwrap_err().message.contains("only makes embeddings"));
    }
}
