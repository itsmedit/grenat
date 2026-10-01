//! Embeddings and vector search, checked: embedding models, `embed`,
//! `Vector(n)` fields and `nearest` (E0100, E0200, E0300, E0414, E0500).

mod common;

use common::*;

const MODELS: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
model :docs, provider: :voyage, name: \"voyage-3.5\", kind: :embedding, dimensions: 1024, price: {input: 0.06}
";

const DOC: &str = "\
struct Doc
  table :docs
  id: Int?
  text: String
  embedding: Vector(1024)
end
";

#[test]
fn embed_gives_vectors_typed_and_untainted() {
    clean(&format!(
        "{MODELS}{DOC}
def vectors(texts: Array(String)) -> Array(Array(Float)) uses llm = embed(:docs, texts)
def vector(text: String) -> Array(Float) uses llm = embed(text)
def index(texts: Array(String)) -> Int uses llm, db
  all = vectors(texts)
  texts.each_with_index {{ |t, i| Doc.create(text: t, embedding: all[i]) }}
  all.size
end
def search(q: String) -> Array(Doc) uses llm, db.read
  Doc.nearest(:embedding, vector(q), limit: 3, where: {{text: q}}, max_distance: 0.5)
end
prompt guess(q: String) -> ~String using :fast
  user q
end
## A vector carries no instructions: stored from an untrusted text.
def remember(q: String) -> Doc uses llm, db
  Doc.create(text: q, embedding: embed(:docs, guess(q).trust!))
end
test \"fake vectors\" do
  mock_embed :docs, vectors: {{\"a\" => [1.0]}}
  mock_embed()
end
"
    ));
}

#[test]
fn embed_is_an_llm_effect_and_no_secret_reaches_it() {
    single(
        &format!("{MODELS}def f(q: String) -> Array(Float) uses db\n  embed(:docs, q)\nend\n"),
        "E0300",
        "embed(:docs, q)",
    );
    single(
        &format!("{MODELS}def f uses llm, env\n  embed(:docs, Credentials.fetch(:x, :y))\nend\n"),
        "E0414",
        "Credentials.fetch(:x, :y)",
    );
}

#[test]
fn embed_names_an_embedding_model() {
    single(&format!("{MODELS}embed(:fast, \"x\")\n"), "E0200", ":fast");
    single(&format!("{MODELS}embed(:doc, \"x\")\n"), "E0100", ":doc");
    single(&format!("{MODELS}embed(:docs, 42)\n"), "E0200", "42");
    single(&format!("{MODELS}embed(:docs, \"a\", \"b\")\n"), "E0200", "embed(:docs, \"a\", \"b\")");
    let without = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\nembed(\"x\")\n";
    single(without, "E0500", "embed(\"x\")");
}

#[test]
fn models_are_declared_for_what_their_provider_does() {
    single("model :d, provider: :anthropic, name: \"x\", kind: :embedding\n", "E0500", ":anthropic");
    single("model :d, provider: :voyage, name: \"voyage-3.5\"\n", "E0500", ":voyage");
    single("model :d, provider: :groq, name: \"x\", kind: :embedding\n", "E0500", ":groq");
    single("model :d, provider: :openai, name: \"x\", kind: :vectors\n", "E0500", ":vectors");
    single("model :d, provider: :openai, name: \"gpt-5\", dimensions: 8\n", "E0500", "dimensions:");
    // a prompt answers with a chat model: an embedding model is not the default
    let only_docs = "model :docs, provider: :openai, name: \"x\", kind: :embedding\n";
    single(&format!("{only_docs}prompt p(q: String) -> ~String\n  user q\nend\n"), "E0500", "p");
    let d = single(&format!("{MODELS}prompt p(q: String) -> ~String using :docs\n  user q\nend\n"), "E0500", ":docs");
    assert!(d.message.contains("embedding model"), "{}", d.message);
    single(&format!("{MODELS}agent A\n  model :docs\nend\n"), "E0500", ":docs");
}

#[test]
fn vectors_and_nearest_are_checked() {
    single("struct V\n  v: Vector\nend\n", "E0200", "Vector");
    single("struct V\n  v: Vector(0)\nend\n", "E0200", "Vector(0)");
    single("struct V\n  v: Array(3)\nend\n", "E0200", "3");
    let search =
        |call: &str| format!("{MODELS}{DOC}def f(v: Array(Float)) -> Array(Doc) uses db.read\n  {call}\nend\n");
    single(&search("Doc.nearest(:text, v)"), "E0200", ":text");
    single(&search("Doc.nearest(:embedding, \"v\")"), "E0200", "\"v\"");
    single(&search("Doc.nearest(:embedding, v, top: 3)"), "E0200", "3");
    single(&search("Doc.nearest(:embedding)"), "E0200", "Doc.nearest(:embedding)");
    single(
        &format!(
            "{MODELS}{DOC}def f(v: Array(Float)) -> Array(Doc)\n  Doc.nearest(:embedding, v)\nend\ndef main uses db.write\n  f([1.0])\nend\n"
        ),
        "E0300",
        "f([1.0])",
    );
    clean("migration \"1\" do |db|\n  db.migrate(\"CREATE TABLE t (v #{db.vector(3)})\")\nend\n");
    single("def f(db: Database) -> Int uses db = db.vector(3)\n", "E0200", "db.vector(3)");
}
