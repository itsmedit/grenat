//! Names, arity, fields, methods, types, patterns (E0100, E0200).

mod common;

use common::*;

#[test]
fn unknown_names_get_suggestions() {
    let d = single("total = 1\nputs totla\n", "E0100", "totla");
    assert_eq!(d.help.as_deref(), Some("did you mean `total`?"));

    let src = format!("{PRELUDE}s = Summary(title: \"a\", bullets: [])\nputs s.titel\n");
    let d = single(&src, "E0200", "titel");
    assert_eq!(d.help.as_deref(), Some("did you mean `title`?"));
}

#[test]
fn calls_are_checked() {
    let fns = "def greet(name: String, times: Int = 1) -> String = name * times\n";
    single(&format!("{fns}greet()\n"), "E0200", "greet()");
    single(&format!("{fns}greet(\"a\", 2, 3)\n"), "E0200", "3");
    single(&format!("{fns}greet(42)\n"), "E0200", "42");
    let d = single(&format!("{fns}greet(\"a\", time: 2)\n"), "E0200", "2");
    assert_eq!(d.help.as_deref(), Some("did you mean `times`?"));
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
fn unknown_values_are_never_reported() {
    // gradual typing: whatever comes from JSON is unknown, hence accepted
    clean("data = Json.parse(\"{}\")\nputs data[\"a\"].whatever.chain(1, 2)\n");
}

#[test]
fn indexing_errors_do_not_cascade() {
    single("x = Nope[1]\n", "E0100", "Nope");
    single("x = 1\ny = x[0]\n", "E0200", "x[0]");
}

#[test]
fn batch_map_is_typed_like_map() {
    single("x = [1, 2].batch_map { |n| n * 2 }\nx.first.upcase\n", "E0200", "upcase");
}

#[test]
fn constants_are_typed_by_their_value() {
    clean("module M\n  LIMIT = 5\n  def self.twice -> Int = LIMIT * 2\nend\np M::LIMIT + 1\n");
    single("module M\n  LIMIT = 5\nend\np M::LIMIT.upcase\n", "E0200", "upcase");
    single("module M\n  LIMIT = 5\nend\np M::LIMT\n", "E0100", "M::LIMT");
}

#[test]
fn a_record_s_class_methods_know_where_and_create() {
    clean(
        "struct Note
  table :notes
  id: Int?
  text: String

  def self.latest -> Array(Note) uses db
    where(text: \"x\")
  end
end
",
    );
}
