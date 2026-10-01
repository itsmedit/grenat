//! Embeddings (`embed`, `mock_embed`) and vector search on records
//! (`Vector(n)` fields, `nearest`), on SQLite and on PostgreSQL.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, migrate, run_main, run_tests};
use serde_json::{Value as Json, json};

const DOCS: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
model :docs, provider: :openai, name: \"text-embedding-3-small\", kind: :embedding, dimensions: 64

struct Doc
  table :docs
  id: Int?
  title: String
  lang: String = \"en\"
  embedding: Vector(64)
end

migration \"001_docs\" do |db|
  db.migrate(\"CREATE TABLE docs (id #{db.primary_key}, title TEXT NOT NULL, lang TEXT NOT NULL, embedding #{db.vector(64)})\")
end

def index(titles: Array(String), lang: String) uses llm, db
  vectors = embed(:docs, titles)
  titles.each_with_index { |t, i| Doc.create(title: t, lang:, embedding: vectors[i]) }
end

def search(q: String, limit: Int) -> Array(String) uses llm, db.read
  Doc.nearest(:embedding, embed(:docs, q), limit:).map { |d| d.title }
end

def titles(docs: Array(Doc)) -> Array(String) = docs.map { |d| d.title }

TITLES = [\"Refund policy: how to get a refund\", \"Exporting invoices to CSV\", \"Changing your password\"]
";

/// Each test's error, or `None`.
fn results(src: &str) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

fn all_pass(src: &str) {
    for (name, error) in results(src) {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}

#[test]
fn the_nearest_records_are_found_by_meaning() {
    all_pass(&format!(
        "{DOCS}
test \"nearest first\" do
  mock_embed :docs
  index(TITLES, \"en\")
  assert_equal \"Refund policy: how to get a refund\", search(\"How do I get a refund?\", 2).first
  assert_equal 2, search(\"refund\", 2).size
  assert_equal 3, search(\"refund\", 10).size
  assert_equal 64, Doc.find(1)&.embedding&.size
end
test \"filters, a distance at most, a default limit\" do
  mock_embed :docs
  index(TITLES, \"en\")
  index([\"Rembourser une facture\"], \"fr\")
  query = embed(:docs, \"refund\")
  assert_equal [\"Rembourser une facture\"], titles(Doc.nearest(:embedding, query, where: {{lang: \"fr\"}}))
  assert_equal [\"Refund policy: how to get a refund\"], titles(Doc.nearest(:embedding, query, max_distance: 0.4))
  assert_equal 4, Doc.nearest(:embedding, query).size
end
test \"given vectors, and records without one\" do
  mock_embed :docs, vectors: {{\"north\" => [0.0, 1.0] + (3..64).map {{ |i| 0.0 }}, \"east\" => [1.0] + (2..64).map {{ |i| 0.0 }}}}
  Doc.create(title: \"up\", embedding: embed(:docs, \"north\"))
  Doc.create(title: \"right\", embedding: embed(:docs, \"east\"))
  Doc.create(title: \"nowhere\", embedding: nil)
  assert_equal [\"right\", \"up\"], titles(Doc.nearest(:embedding, embed(:docs, \"east\")))
  assert_equal 3, Doc.count
end
test \"no text, no call\" do
  assert_equal [], embed(:docs, [])
end
"
    ));
}

#[test]
fn only_a_direction_counts_and_floats_stay_in_range() {
    let zeros = "(4..64).map { |i| 0.0 }";
    let results = results(&format!(
        "{DOCS}
def vector(x: Float, y: Float) -> Array(Float) = [x, y, 0.0] + {zeros}
def compass uses db
  Doc.create(title: \"north\", embedding: vector(0.0, 1.0))
  Doc.create(title: \"east\", embedding: vector(1.0, 0.0))
end
test \"large and tiny queries\" do
  compass
  assert_equal [\"east\"], titles(Doc.nearest(:embedding, vector(1.0e38, 0.0), limit: 1))
  assert_equal 1, Doc.nearest(:embedding, vector(1.0e38, 0.0), max_distance: 0.1).size
  assert_equal [\"east\"], titles(Doc.nearest(:embedding, vector(1.0e-200, 0.0), limit: 1))
  assert_equal [\"north\"], titles(Doc.nearest(:embedding, vector(1.0e-200, 1.0e-150), limit: 1))
end
test \"a query out of range\" do
  Doc.nearest(:embedding, vector(1.0e200, 0.0))
end
test \"a float out of range\" do
  Doc.create(title: \"far\", embedding: vector(1.0e39, 0.0))
end
test \"not a number\" do
  Doc.nearest(:embedding, vector(0.0 / 0.0, 0.0))
end
"
    ));
    let errors: Vec<&str> = results.iter().map(|(_, e)| e.as_deref().unwrap_or("passed")).collect();
    assert_eq!(
        errors,
        [
            "passed",
            "TypeError: a vector holds 32-bit floats: 1e200 is out of their range",
            "TypeError: a vector holds 32-bit floats: 1e39 is out of their range",
            "TypeError: a vector holds 32-bit floats: NaN is out of their range",
        ]
    );
}

#[test]
fn mistakes_are_said() {
    let results = results(&format!(
        "{DOCS}
test \"a vector of another size\" do
  mock_embed :docs
  Doc.create(title: \"x\", embedding: [1.0, 2.0])
end
test \"a fake of another size\" do
  mock_embed :docs, dimensions: 8
  embed(:docs, \"x\")
end
test \"an empty text\" do
  mock_embed :docs
  embed(:docs, \"  \")
end
test \"no fake in tests\" do
  embed(:docs, \"refund\")
end
test \"a vector is not compared for equality\" do
  mock_embed :docs
  Doc.where(embedding: embed(:docs, \"x\"))
end
test \"given vectors of two sizes\" do
  mock_embed :docs, vectors: {{\"a\" => [1.0], \"b\" => [1.0, 2.0]}}
end
test \"given vectors of another size\" do
  mock_embed :docs, vectors: {{\"a\" => [1.0]}}, dimensions: 4
end
test \"an embedding model answers no prompt\" do
  mock_embed :docs
  Conversation.new(model: :docs).say(\"hi\")
end
"
    ));
    let errors: Vec<&str> = results.iter().map(|(_, e)| e.as_deref().unwrap_or("passed")).collect();
    assert_eq!(
        errors,
        [
            "TypeError: `Doc.embedding` is a `Vector(64)`: a vector of 2 floats cannot be stored",
            "LlmError: `text-embedding-3-small` gave a vector of 8 dimensions, not the 64 declared",
            "ArgumentError: `embed`: an empty text has no meaning to embed",
            "LlmError: no real model in tests: `text-embedding-3-small` embeds outside `mock_embed`",
            "ArgumentError: `Doc.embedding` is a vector: search it with `nearest`",
            "ArgumentError: `mock_embed`: the vectors given have different sizes (1, 2)",
            "ArgumentError: `mock_embed`: vectors of 1 floats are given, not of the 4 of `dimensions:`",
            "LlmError: `text-embedding-3-small` is an embedding model: it answers `embed`, not prompts",
        ]
    );
}

#[test]
fn declarations_say_what_a_model_is_for() {
    let declare = |line: &str| {
        let parsed = grenat_parser::parse(line);
        run_main(&parsed.program, Vec::new(), Options { output: Output::Capture(Arc::default()), ..Options::default() })
            .unwrap_err()
            .message
    };
    assert!(
        declare("model :d, provider: :anthropic, name: \"x\", kind: :embedding\n").contains("declare a `voyage` model")
    );
    assert!(declare("model :d, provider: :voyage, name: \"voyage-3.5\"\n").contains("add `kind: :embedding`"));
    assert!(declare("model :d, provider: :groq, name: \"x\", kind: :embedding\n").contains("makes no embeddings"));
    assert!(declare("model :d, provider: :openai, name: \"gpt-5\", dimensions: 8\n").contains("for embedding models"));
    assert!(
        declare("model :d, provider: :openai, name: \"x\", kind: :vector\n").contains("invalid option `kind: :vector`")
    );
    // an embedding model's price is for its input only
    let parsed = grenat_parser::parse(
        "model :d, provider: :openai, name: \"x\", kind: :embedding, price: {input: 0.02}\nmodel :f, provider: :openai, name: \"y\", price: {input: 1}\n",
    );
    let e = run_main(&parsed.program, Vec::new(), Options::default()).unwrap_err();
    assert!(e.message.contains("model `:f`: `price:` is `{input: …, output: …}`"), "{}", e.message);
}

/// A server playing `POST /v1/embeddings`; the requests it received.
fn embeddings_server(requests: usize) -> (String, Arc<Mutex<Vec<Json>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    std::thread::spawn(move || {
        for _ in 0..requests {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim_end().is_empty() {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut raw = vec![0; length];
            reader.read_exact(&mut raw).unwrap();
            let body: Json = serde_json::from_slice(&raw).unwrap();
            let data: Vec<Json> = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, text)| json!({"index": i, "embedding": [text.as_str().unwrap().len() as f64, 1.0, 0.0]}))
                .collect();
            log.lock().unwrap().push(body);
            let payload = json!({"data": data, "model": "nomic-embed-text", "usage": {"prompt_tokens": 4}}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let mut stream = stream;
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (url, received)
}

#[test]
fn a_real_provider_is_called_counted_and_recorded() {
    let (url, received) = embeddings_server(2);
    let db = temp_dir("embeddings-ledger").join("app.db");
    let src = format!(
        "database \"sqlite://{}\"
model :local, provider: :ollama, name: \"nomic-embed-text\", kind: :embedding, dimensions: 3, base_url: \"{url}\", price: {{input: 50000}}
def main uses llm
  p embed(:local, \"abc\")
  p embed([\"a\", \"hello\"])
  puts budget.spent
end
",
        db.display()
    );
    let parsed = grenat_parser::parse(&src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let out = Arc::new(Mutex::new(String::new()));
    let options =
        Options { output: Output::Capture(out.clone()), journal: Some(temp_dir("journal")), ..Options::default() };
    let summary = run_main(&parsed.program, Vec::new(), options).unwrap();
    // 4 tokens a request, at $50,000 per million
    assert_eq!(*out.lock().unwrap(), "[3.0, 1.0, 0.0]\n[[1.0, 1.0, 0.0], [5.0, 1.0, 0.0]]\n$0.40\n");
    assert_eq!((summary.llm_calls, summary.tokens), (2, 8));
    let received = received.lock().unwrap();
    assert_eq!(received[0], json!({"model": "nomic-embed-text", "input": ["abc"], "dimensions": 3}));
    assert_eq!(received[1]["input"], json!(["a", "hello"]));
    let mut connection = grenat_db::connect(&format!("sqlite://{}", db.display())).unwrap();
    let calls = grenat_ops::calls::since(connection.as_mut(), 0.0).unwrap();
    let recorded: Vec<_> = calls.iter().map(|c| (c.model.as_str(), c.input_tokens, c.output_tokens)).collect();
    assert_eq!(recorded, [("nomic-embed-text", 4, 0), ("nomic-embed-text", 4, 0)]);
    assert!((calls[0].cost_usd - 0.2).abs() < 1e-9);
}

#[test]
fn a_budget_stops_embeddings() {
    let (url, _) = embeddings_server(1);
    let src = format!(
        "model :local, provider: :ollama, name: \"m\", kind: :embedding, base_url: \"{url}\", price: {{input: 500000}}
def main uses llm
  within budget(usd: 1.00) do
    embed(:local, \"first\")
    embed(:local, \"second\")
  end
end
"
    );
    let parsed = grenat_parser::parse(&src);
    let options = Options { output: Output::Capture(Arc::default()), ..Options::default() };
    let e = run_main(&parsed.program, Vec::new(), options).unwrap_err();
    assert_eq!(e.ty, "BudgetExceeded", "{}", e.message);
}

#[test]
fn postgres_records_hold_and_search_vectors() {
    let Ok(url) = std::env::var("GRENAT_TEST_POSTGRES") else {
        eprintln!("GRENAT_TEST_POSTGRES not set: vectors on PostgreSQL not tested");
        return;
    };
    let table = format!("grenat_docs_{}", std::process::id());
    let src = format!(
        "database \"{url}\"
model :docs, provider: :openai, name: \"text-embedding-3-small\", kind: :embedding
struct Doc
  table :{table}
  id: Int?
  title: String
  embedding: Vector(3)
end
migration \"{table}\" do |db|
  db.migrate(\"CREATE TABLE {table} (id #{{db.primary_key}}, title TEXT NOT NULL, embedding #{{db.vector(3)}} NOT NULL)\")
end
def main uses db
  Doc.create(title: \"east\", embedding: [1.0, 0.0, 0.0])
  Doc.create(title: \"north\", embedding: [0.0, 1.0, 0.0])
  Doc.create(title: \"up\", embedding: [0.0, 0.0, 1.0])
  p Doc.nearest(:embedding, [0.1, 1.0, 0.2], limit: 2).map {{ |d| d.title }}
  p Doc.nearest(:embedding, [1.0, 0.0, 0.0], max_distance: 0.5).map {{ |d| d.title }}
  p Doc.find(2)&.embedding
end
"
    );
    let parsed = grenat_parser::parse(&src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = || Options { journal: Some(temp_dir("journal")), ..Options::default() };
    let out = Arc::new(Mutex::new(String::new()));
    let result = migrate(&parsed.program, options()).and_then(|_| {
        run_main(&parsed.program, Vec::new(), Options { output: Output::Capture(out.clone()), ..options() })
    });
    let mut db = grenat_db::connect(&url).unwrap();
    let storage = grenat_db::vectors::storage(db.as_mut(), &table, "embedding");
    db.batch(&format!("DROP TABLE IF EXISTS {table}; DELETE FROM grenat_migrations WHERE name = '{table}'")).unwrap();
    result.unwrap();
    assert_eq!(*out.lock().unwrap(), "[\"north\", \"up\"]\n[\"east\"]\n[0.0, 1.0, 0.0]\n");
    match storage.unwrap() {
        grenat_db::vectors::Storage::PgVector => eprintln!("searched by pgvector"),
        grenat_db::vectors::Storage::Bytes => {
            eprintln!("the PostgreSQL server has no pgvector: searched by brute force (pgvector's path not tested)")
        }
    }
}
