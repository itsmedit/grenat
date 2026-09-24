//! Vérification statique : ce qui doit être signalé, et surtout ce qui ne doit pas l'être.

use grenat_types::{Diagnostic, check};

fn diags(src: &str) -> Vec<Diagnostic> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    check(&parsed.program)
}

fn clean(src: &str) {
    let d = diags(src);
    assert!(d.is_empty(), "diagnostics inattendus :\n{}", render(src, &d));
}

/// Un seul diagnostic, avec ce code, sur ce texte du source.
fn single(src: &str, code: &str, at: &str) -> Diagnostic {
    let d = diags(src);
    assert_eq!(d.len(), 1, "un seul diagnostic attendu :\n{}", render(src, &d));
    let diag = d.into_iter().next().unwrap();
    assert_eq!(diag.code, Some(code), "{}", render(src, std::slice::from_ref(&diag)));
    assert_eq!(&src[diag.span.range()], at, "{}", diag.message);
    diag
}

fn render(src: &str, diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| format!("  [{}] {} « {} »", d.code.unwrap_or("-"), d.message, &src[d.span.range()]))
        .collect::<Vec<_>>()
        .join("\n")
}

fn example(name: &str) -> String {
    std::fs::read_to_string(format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn examples_are_clean() {
    for name in ["bases.grn", "explorateur.grn", "support_desk.grn"] {
        let src = example(name);
        let d = diags(&src);
        assert!(d.is_empty(), "{name} :\n{}", render(&src, &d));
    }
}

// ── Teinte ───────────────────────────────────────────────────

const PRELUDE: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
struct Summary
  title: String
  bullets: Array(String)
  def headline = title.upcase
end
prompt summarize(text: String) -> ~Summary using :fast
  user text
end
tool send(to: String, body: String) -> Unit uses net
  puts body
end
";

#[test]
fn llm_output_cannot_reach_the_network() {
    let src = format!("{PRELUDE}s = summarize(\"x\")\nsend(\"a@b.c\", s.title)\n");
    let d = single(&src, "E0412", "s.title");
    assert!(d.message.contains("`send` (effet `net`)"), "{}", d.message);
    assert_eq!(&src[d.notes[0].0.range()], "summarize(\"x\")");
}

#[test]
fn validation_clears_the_taint() {
    for fix in [
        "s = summarize(\"x\").check { |x| x.bullets.size > 0 }?",
        "s = summarize(\"x\").trust!",
        "s = summarize(\"x\").approve(by: :human)",
    ] {
        clean(&format!("{PRELUDE}{fix}\nsend(\"a@b.c\", s.title)\n"));
    }
}

#[test]
fn taint_flows_through_interpolation_blocks_and_collections() {
    for leak in [
        "send(\"a\", \"Résumé : #{s.title}\")",
        "s.bullets.each { |b| send(\"a\", b) }",
        "xs = []\nxs << s.title\nsend(\"a\", xs.join(\", \"))",
        "send(\"a\", s.headline)",
        "t = s.title\nu = t.upcase\nsend(\"a\", u)",
    ] {
        let src = format!("{PRELUDE}s = summarize(\"x\")\n{leak}\n");
        let d = diags(&src);
        assert!(d.iter().any(|d| d.code == Some("E0412")), "fuite non détectée :\n{leak}");
    }
}

#[test]
fn taint_flows_through_helper_functions() {
    // la fonction intermédiaire est vérifiée pour la teinte réelle de son argument
    let src = format!(
        "{PRELUDE}def format(s: Summary) -> String = \"#{{s.title}} !\"\nsend(\"a\", format(summarize(\"x\")))\n"
    );
    single(&src, "E0412", "format(summarize(\"x\"))");
    // … et la même fonction reste utilisable avec une valeur propre
    clean(&format!(
        "{PRELUDE}def format(s: Summary) -> String = \"#{{s.title}} !\"\nsend(\"a\", format(Summary(title: \"t\", bullets: [])))\n"
    ));
}

#[test]
fn taint_is_tracked_in_agent_state() {
    let src = format!(
        "{PRELUDE}agent Notes
  @last: String = \"\"
  on Remember(text: String)
    @last = summarize(text).title
  end
  on Publish
    send(\"a\", @last)
  end
end
"
    );
    single(&src, "E0412", "@last");
}

#[test]
fn file_write_is_a_dangerous_sink() {
    let src = format!("{PRELUDE}def main uses llm, fs.write\n  File.write(\"out.txt\", summarize(\"x\").title)\nend\n");
    single(&src, "E0412", "summarize(\"x\").title");
}

#[test]
fn prompts_and_run_must_declare_taint() {
    let src = "model :m, name: \"claude-haiku-4-5\"\nprompt p(x: String) -> String\n  user x\nend\n";
    let d = single(src, "E0413", "String");
    assert_eq!(d.help.as_deref(), Some("écrivez `-> ~String`"));

    let src = "model :m, name: \"claude-haiku-4-5\"\nagent A\n  on Go -> String\n    run \"fais\"\n  end\nend\n";
    single(src, "E0413", "String");
}

// ── Effets ───────────────────────────────────────────────────

#[test]
fn main_must_declare_what_it_does() {
    let src = format!("{PRELUDE}def main\n  summarize(\"x\")\nend\n");
    let d = single(&src, "E0300", "summarize(\"x\")");
    assert_eq!(d.help.as_deref(), Some("ajoutez `uses llm`"));
    clean(&format!("{PRELUDE}def main uses llm\n  summarize(\"x\")\nend\n"));
}

#[test]
fn declared_effects_must_cover_the_body() {
    let src = "def load uses fs.read(\"./docs\")\n  File.read(\"./secrets/key\")\nend\n";
    let d = single(src, "E0300", "File.read(\"./secrets/key\")");
    assert!(d.message.contains("fs.read(\"./secrets/key\")"), "{}", d.message);
    clean("def load uses fs.read(\"./docs\")\n  File.read(\"./docs/a.md\")\nend\n");
    clean("def load uses fs\n  File.write(\"a\", \"b\")\nend\n");
}

#[test]
fn effects_propagate_through_undeclared_helpers() {
    let src = "def helper = File.read(\"a\")\ndef main uses llm\n  helper\nend\n";
    let d = single(src, "E0300", "helper");
    assert!(d.message.contains("fs.read(\"a\")"), "{}", d.message);
}

#[test]
fn tools_must_declare_effects() {
    let src = "tool read(path: String) -> String\n  File.read(path)\nend\n";
    single(src, "E0300", "File.read(path)");
}

#[test]
fn unknown_effect_names_are_reported() {
    let d = single("def f uses fs.raed\n  1\nend\n", "E0500", "fs.raed");
    assert_eq!(d.help.as_deref(), Some("vouliez-vous `fs.read` ?"));
}

// ── Noms et types ────────────────────────────────────────────

#[test]
fn unknown_names_get_suggestions() {
    let d = single("total = 1\nputs totla\n", "E0100", "totla");
    assert_eq!(d.help.as_deref(), Some("vouliez-vous `total` ?"));

    let src = format!("{PRELUDE}s = Summary(title: \"a\", bullets: [])\nputs s.titel\n");
    let d = single(&src, "E0200", "titel");
    assert_eq!(d.help.as_deref(), Some("vouliez-vous `title` ?"));
}

#[test]
fn calls_are_checked() {
    let fns = "def greet(name: String, times: Int = 1) -> String = name * times\n";
    single(&format!("{fns}greet()\n"), "E0200", "greet()");
    single(&format!("{fns}greet(\"a\", 2, 3)\n"), "E0200", "3");
    single(&format!("{fns}greet(42)\n"), "E0200", "42");
    let d = single(&format!("{fns}greet(\"a\", time: 2)\n"), "E0200", "2");
    assert_eq!(d.help.as_deref(), Some("vouliez-vous `times` ?"));
    clean(&format!("{fns}greet(\"a\")\ngreet(name: \"a\", times: 3)\n"));
}

#[test]
fn builtin_methods_are_checked() {
    single("puts \"abc\".upcaes\n", "E0200", "upcaes");
    single("x = 1 + \"a\"\n", "E0200", "1 + \"a\"");
    single("x = \"a\" + 1\n", "E0200", "\"a\" + 1");
    clean("xs = [1, 2, 3]\nys = xs.map { |x| x * 2 }.select(&.even?)\nputs ys.sum + ys.first\n");
}

#[test]
fn return_types_are_checked() {
    single("def f -> Int = \"a\"\n", "E0200", "\"a\"");
    clean("def f -> Float = 1\ndef g -> String? = nil\ndef h -> Unit = puts 1\n");
}

#[test]
fn patterns_are_checked() {
    let shapes = "enum Shape\n  Circle(radius: Float)\n  Square(side: Float)\nend\nenum Color\n  Red\nend\n";
    single(&format!("{shapes}def f(s: Shape) = case s\n  in Red then 1\n  else 2\n  end\n"), "E0200", "Red");
    single(
        &format!("{shapes}def f(s: Shape) = case s\n  in Circle(radus: r) then r\n  else 2\n  end\n"),
        "E0100",
        "radus:",
    );
    clean(&format!(
        "{shapes}def f(s: Shape) -> Float = case s\n  in Circle(r) then r * 2.0\n  in Square(side:) then side\n  end\n"
    ));
}

#[test]
fn agents_are_checked() {
    let agent = "model :m, name: \"claude-haiku-4-5\"\ntool look(q: String) -> String uses net\n  q\nend\nagent A\n  model :m\n  tools look\n  on Go(topic: String) -> ~String\n    run \"cherche #{topic}\"\n  end\nend\n";
    clean(&format!("{agent}def main uses llm, net\n  spawn(A).ask(Go(topic: \"x\"))\nend\n"));
    single(&format!("{agent}def main uses llm, net\n  spawn(A).ask(Stop())\nend\n"), "E0100", "Stop()");
    single(&agent.replace("tools look", "tools lok"), "E0100", "lok");
    single(&agent.replace("model :m\n  tools", "model :n\n  tools"), "E0100", ":n");
    single(
        &format!("{agent}def main uses llm\n  spawn(A).ask(Go(topic: \"x\"))\nend\n"),
        "E0300",
        "spawn(A).ask(Go(topic: \"x\"))",
    );
}

#[test]
fn prompt_output_must_be_serializable() {
    let src = "model :m, name: \"claude-haiku-4-5\"\nprompt p(x: String) -> ~Hash(String, Int)\n  user x\nend\n";
    single(src, "E0500", "~Hash(String, Int)");
}

#[test]
fn unknown_values_are_never_reported() {
    // typage graduel : ce qui vient de JSON est inconnu, donc accepté
    clean("data = Json.parse(\"{}\")\nputs data[\"a\"].whatever.chain(1, 2)\n");
}
