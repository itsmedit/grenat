//! `native def`: Rust functions of a native facet, checked from their
//! declarations — types, effects on the callers, taint of the results,
//! secrets refused (E0200, E0300, E0412, E0413, E0414, E0500).

mod common;

use common::*;

const NATIVES: &str = "\
tool send(to: String, body: String) -> Unit uses net
  puts body
end
struct Cell
  row: Int
  text: String
end
## Adds two integers.
native def add(a: Int, b: Int) -> Int pure
native def shout(text: String) -> ~String
native def read_sheet(path: String) -> ~Array(Array(String)) uses fs.read
native def cells(rows: Array(Array(String))) -> Array(Cell) pure
native def count_words(text: String) -> Hash(String, Int) pure
native def find(words: Array(String), word: String) -> Int? pure
native def tick
";

fn with(code: &str) -> String {
    format!("{NATIVES}{code}")
}

#[test]
fn native_functions_are_called_like_any_function() {
    clean(&with(
        "def main uses fs.read
  n = add(1, 2) + find([\"a\"], \"a\").to_i
  rows = read_sheet(\"sheet.csv\").trust!
  puts cells(rows).map { |c| c.text }.join(\", \"), count_words(\"a b\").size, n
  tick
end
",
    ));
}

#[test]
fn a_wrong_argument_is_a_type_error() {
    let d = single(&with("add(1, \"two\")\n"), "E0200", "\"two\"");
    assert!(d.message.contains("`add`"), "{}", d.message);
    single(&with("cells([1, 2])\n"), "E0200", "[1, 2]");
    single(&with("add(1)\n"), "E0200", "add(1)");
    let d = single(&with("x = add(1, 2)\nx.upcase\n"), "E0200", "upcase");
    assert!(d.message.contains("Int"), "{}", d.message);
}

#[test]
fn the_effects_of_a_native_function_are_the_callers() {
    let d = single(&with("def main uses net\n  read_sheet(\"x\")\nend\n"), "E0300", "read_sheet(\"x\")");
    assert!(d.message.contains("`fs.read`"), "{}", d.message);
    single(&with("def main\n  read_sheet(\"x\")\nend\n"), "E0300", "read_sheet(\"x\")");
    // an intermediate function passes them on
    single(&with("def load = read_sheet(\"x\")\ndef main uses net\n  load\nend\n"), "E0300", "load");
    clean(&with("def main\n  puts add(1, 2)\nend\n"));
}

#[test]
fn a_native_result_is_untrusted_unless_pure() {
    let d = single(&with("send(\"a\", shout(\"hi\"))\n"), "E0412", "shout(\"hi\")");
    assert!(d.message.contains("`send` (effect `net`)"), "{}", d.message);
    single(&with("rows = read_sheet(\"x\")\nsend(\"a\", rows.first.first)\n"), "E0412", "rows.first.first");
    clean(&with("send(\"a\", shout(\"hi\").trust!)\n"));
    // pure: trusted, as long as its arguments are
    clean(&with("send(\"a\", add(1, 2).to_s)\nsend(\"a\", count_words(\"a\").keys.first)\n"));
    single(&with("send(\"a\", find([shout(\"x\")], \"X\").to_s)\n"), "E0412", "find([shout(\"x\")], \"X\").to_s");
}

#[test]
fn declarations_say_what_crosses_and_how_far_it_is_trusted() {
    let d = single("native def f(x: Int) -> Int\n", "E0413", "Int");
    assert_eq!(d.help.as_deref(), Some("write `-> ~Int`, or declare the function `pure`"));
    single("native def f(x: Int) -> ~Int pure\n", "E0413", "~Int");
    single("native def f(x: Int) -> Int uses fs.read pure\n", "E0500", "fs.read");
    single("native def f(x) -> Int pure\n", "E0500", "x");
    single("native def f(x: Money) -> Int pure\n", "E0500", "Money");
    single("native def f(x: Hash(Int, String)) -> Int pure\n", "E0500", "Hash(Int, String)");
    single("class Box\n  @x: Int = 0\nend\nnative def f(b: Box) -> Int pure\n", "E0500", "Box");
    single("native def f(k: Secret) -> Int pure\n", "E0500", "Secret");
    single("native def f(x: Int) -> ~Money\n", "E0500", "~Money");
    single("native def f uses teleport\n", "E0500", "teleport");
    clean("native def f(x: Int?, h: Hash(String, Array(Float))) -> ~Bool\nnative def g(x: Int)\n");
}

#[test]
fn no_secret_reaches_native_code() {
    let src = with("shout(Credentials.fetch(:github, :token))\n");
    let d = diags(&src);
    assert!(
        d.iter().any(|d| d.code == Some("E0414") && d.message == "a secret reaches native code through `shout`"),
        "{}",
        render(&src, &d)
    );
}
