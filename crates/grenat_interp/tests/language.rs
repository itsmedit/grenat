//! Core language: closures, modules, structs, Result, errors, Grenat tests.

mod common;

use common::*;
use grenat_interp::{Options, run_tests};

#[test]
fn closures_capture_and_mutate_outer_variables() {
    assert_eq!(run("total = 0\n[1, 2, 3].each { |x| total += x }\nputs total\n"), "6\n");
}

#[test]
fn return_inside_a_block_returns_from_the_method() {
    let src = "def first_even(xs: Array(Int)) -> Int?\n  xs.each { |x| return x if x.even? }\n  nil\nend\np first_even([1, 3, 4, 5])\n";
    assert_eq!(run(src), "4\n");
}

#[test]
fn main_receives_arguments() {
    assert_eq!(run("def main(args: Array(String))\n  puts args.first\nend\n"), "arg1\n");
}

#[test]
fn modules_are_included() {
    let src = "\
module Describable
  abstract def describe -> String
  def shout = describe.upcase
end
struct Invoice
  include Describable
  amount: Int
  def describe = \"invoice for #{amount}\"
end
puts Invoice(amount: 3).shout
";
    assert_eq!(run(src), "INVOICE FOR 3\n");
}

#[test]
fn structs_are_immutable_but_copyable() {
    let src = "struct P\n  x: Int\n  y: Int = 0\nend\na = P(x: 1)\nb = a.with(y: 2)\np a, b\n";
    assert_eq!(run(src), "P(x: 1, y: 0)\nP(x: 1, y: 2)\n");
    let e = run_err("struct P\n  x: Int\nend\na = P(x: 1)\na.x = 2\n", vec![]);
    assert_eq!(e.ty, "TypeError");
}

#[test]
fn case_without_match_raises() {
    let e = run_err("enum E\n  A\n  B\nend\ncase B\nin A then 1\nend\n", vec![]);
    assert_eq!(e.ty, "NoMatchingPattern");
}

#[test]
fn errors_carry_location_and_trace() {
    let src = "def inner\n  unknown_name\nend\ndef outer = inner\nouter\n";
    let e = run_err(src, vec![]);
    assert_eq!(e.ty, "NameError");
    assert_eq!(&src[e.span.unwrap().range()], "unknown_name");
    let names: Vec<_> = e.trace.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["inner", "outer"]);
}

#[test]
fn result_and_try_operator() {
    let src = "\
def parse(s: String) -> Result(Int, ParseError)
  return Err(ParseError(\"empty\")) if s.empty?
  Ok(s.to_i)
end
def double(s: String) = parse(s)? * 2
p double(\"21\")
begin
  double(\"\")
rescue ParseError => e
  puts e.message
end
p parse(\"\").or_else { |e| -1 }
";
    assert_eq!(run(src), "42\nempty\n-1\n");
}

#[test]
fn deep_recursion_is_reported_not_crashed() {
    let e = run_err("def f(n: Int) = f(n + 1)\nf(0)\n", vec![]);
    assert_eq!(e.ty, "StackOverflow");
}

#[test]
fn grenat_test_blocks() {
    let src = "\
test \"addition\" do
  assert_equal 4, 2 + 2
end
test \"failure\" do
  assert 1 > 2, \"one is not greater than two\"
end
test \"errors\" do
  assert_raises ZeroDivisionError { 1 / 0 }
end
";
    let parsed = grenat_parser::parse(src);
    let outcomes = run_tests(&parsed.program, Options::default()).unwrap();
    let summary: Vec<_> =
        outcomes.iter().map(|o| (o.name.as_str(), o.error.as_ref().map(|e| e.message.as_str()))).collect();
    assert_eq!(summary, [("addition", None), ("failure", Some("one is not greater than two")), ("errors", None)]);
}

#[test]
fn integer_arithmetic_follows_ruby_and_never_panics() {
    // floor division and modulo take the sign of the divisor
    assert_eq!(run("p [7 / 2, -7 / 2, 7 / -2, -7 / -2]\n"), "[3, -4, -4, 3]\n");
    assert_eq!(run("p [7 % 3, -7 % 3, 7 % -3, -7 % -3]\n"), "[1, 2, -2, -1]\n");
    let min = "-9223372036854775807 - 1";
    assert_eq!(run(&format!("p(({min}) % -1)\n")), "0\n");
    for overflow in [
        format!("({min}) / -1"),
        format!("({min}).abs"),
        format!("-({min})"),
        "9223372036854775807 + 1".to_string(),
        "9223372036854775807 * 2".to_string(),
    ] {
        assert_eq!(run_err(&format!("p({overflow})\n"), vec![]).ty, "OverflowError", "{overflow}");
    }
    assert_eq!(run_err("p(1 % 0)\n", vec![]).ty, "ZeroDivisionError");
}

#[test]
fn a_class_method_calls_its_siblings_without_a_receiver() {
    let src = "module Api\n  def self.base = \"https://x.io\"\n  def self.url(path: String) -> String = \"#{base}/#{path}\"\nend\nputs Api.url(\"a\")\n";
    assert_eq!(run(src), "https://x.io/a\n");
}

#[test]
fn hash_shorthand_takes_the_variable_of_the_same_name() {
    assert_eq!(run("query = \"rust\"\nn = 2\np({query:, n:, other: 3})\n"), "{query: \"rust\", n: 2, other: 3}\n");
}

#[test]
fn a_negation_is_a_command_argument() {
    assert_eq!(run("def yes?(x) = x\np yes? !false\nx = 3\np x != 2\n"), "true\ntrue\n");
}

#[test]
fn the_ternary() {
    let src = "\
x = 3
p x > 2 ? \"big\" : \"small\"
p x.even? ? \"even\" : x > 1 ? \"odd, above 1\" : \"odd\"
p (false || x == 3) ? 1 : 2
p \"#{x == 3 ? \"three\" : \"other\"}\"
y = x < 0 ?
  -1 :
  1
p y
def sign(n: Int) -> Int = n < 0 ? -1 : n == 0 ? 0 : 1
p [sign(-5), sign(0), sign(9)]
r = Ok(4)
def twice(r) = r? * 2
p twice(r)
";
    assert_eq!(run(src), "\"big\"\n\"odd, above 1\"\n1\n\"three\"\n1\n[-1, 0, 1]\n8\n");
}

#[test]
fn constants_of_a_type() {
    let src = "\
module GitHub
  API = \"https://api.github.com\"
  PER_PAGE = 50
  def self.url(path: String) -> String = \"#{API}/#{path}?per_page=#{PER_PAGE}\"
end
class Client
  RETRIES = 3
  def retries -> Int = RETRIES * 2
end
puts GitHub.url(\"repos\")
p GitHub::PER_PAGE + 1, Client.new.retries, GitHub.API
";
    assert_eq!(run(src), "https://api.github.com/repos?per_page=50\n51\n6\n\"https://api.github.com\"\n");
}

#[test]
fn top_level_constants() {
    assert_eq!(run("LIMIT = 3\ndef twice = LIMIT * 2\np LIMIT, twice\n"), "3\n6\n");
}
