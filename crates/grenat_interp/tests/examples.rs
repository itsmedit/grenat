//! Exemples du dépôt exécutés de bout en bout.

mod common;

use common::*;
use grenat_interp::Response;
use grenat_interp::Scripted;
use serde_json::json;

#[test]
fn explorateur_example_runs_end_to_end() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/explorateur.grn")).unwrap();
    let replies = vec![
        Response::tool_call("t1", "list_dir", json!({"path": "src"})),
        Response::tool_call("t2", "read_file", json!({"path": "src/lib.rs", "max_lines": 1})),
        Response::tool_call(
            "t3",
            "final_answer",
            json!({"summary": "Un interpréteur.", "files_read": ["src/lib.rs"], "maturity": "Solide", "next_steps": ["Typer"]}),
        ),
        Response::text_reply("Grenat, enfin des agents typés."),
    ];
    let r = run_with(&src, replies, &[]);
    r.result.unwrap();
    assert_eq!(
        r.output,
        "## arg1 — Solide\nUn interpréteur.\n\nFichiers lus : src/lib.rs\n- Typer\n\n> Grenat, enfin des agents typés.\n"
    );
    let listing = &r.requests[1]["messages"][2]["content"][0]["content"];
    assert!(listing.as_str().unwrap().contains("lib.rs"), "{listing}");
    let first_line = r.requests[2]["messages"][4]["content"][0]["content"].as_str().unwrap().to_string();
    let expected = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    assert_eq!(first_line, expected.lines().next().unwrap());
    assert_eq!(r.requests[0]["fallbacks"], "default");
    assert_eq!(r.requests[3]["model"], "claude-haiku-4-5");
}

#[test]
fn bases_example_runs() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/bases.grn")).unwrap();
    assert!(run(&src).starts_with("fib(25) = 75025\n"));
}

#[test]
fn support_desk_example_runs_end_to_end() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/support_desk.grn")).unwrap();
    // les deux tickets sont traités en parallèle : réponses calculées depuis la requête
    let provider = Scripted::responder(|body| {
        if body.get("tools").is_some() {
            return Response::tool_call(
                "t1",
                "final_answer",
                json!({"body": "Menu Factures > Exporter.", "sources": ["docs/export.md"], "confidence": 0.9}),
            );
        }
        let spam = body["messages"][0]["content"].as_str().unwrap().contains("GAGNEZ");
        let category = if spam { "Spam" } else { "Question" };
        Response::json_reply(json!({"category": category, "priority": "Normal", "language": "fr", "reason": "motif"}))
    });
    let r = run_provider(&src, provider, &["o"], &["../../examples/tickets.jsonl"]);
    if let Err(e) = &r.result {
        panic!("{e:?}\n{}", r.output);
    }
    assert!(r.output.contains("Envoyer à ana@example.com ?"), "{}", r.output);
    assert!(
        r.output.contains("[smtp] à ana@example.com — Re: votre demande\nMenu Factures > Exporter.\n"),
        "{}",
        r.output
    );
    assert!(r.output.ends_with("✓ 1 envoyés, 1 ignorés — coût $0.0014\n"), "{}", r.output);
    assert_eq!(r.requests.len(), 3);
    assert_eq!(r.requests.iter().filter(|q| q["model"] == "claude-opus-5").count(), 1);
}
