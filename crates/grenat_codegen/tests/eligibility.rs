//! Which functions are compiled, and why the others stay interpreted.

mod common;

use common::*;
use grenat_codegen::Scalar;

fn reason(src: &str, name: &str) -> String {
    let (_, jit) = compile(src);
    let report = jit.report();
    assert!(!report.compiled.contains(&name.to_string()), "`{name}` should not be compiled");
    report
        .interpreted
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, r)| r.clone())
        .unwrap_or_else(|| panic!("`{name}` is not a candidate: {report:?}"))
}

#[test]
fn numeric_functions_are_compiled() {
    let (program, jit) = compile("def fib(n: Int) -> Int\n  return n if n < 2\n  fib(n - 1) + fib(n - 2)\nend\n");
    assert_eq!(jit.report().compiled, ["fib"]);
    assert!(jit.is_compiled(function(program, "fib")));
}

#[test]
fn ordinary_functions_are_not_candidates() {
    let src = "\
def untyped(n) = n
def no_return(n: Int) = n
def default(n: Int = 1) -> Int = n
def text(s: String) -> String = s
def effect(n: Int) -> Int uses fs.read
  n
end
";
    let (_, jit) = compile(src);
    assert!(jit.report().compiled.is_empty());
    assert!(jit.report().interpreted.is_empty(), "{:?}", jit.report());
}

#[test]
fn unsupported_bodies_are_reported() {
    assert_eq!(reason("def f(n: Int) -> Int\n  puts n\n  n\nend\n", "f"), "calls `puts`, which is not compiled");
    assert_eq!(reason("def f(n: Int) -> Int\n  s = \"a\"\n  n\nend\n", "f"), "uses strings");
    assert_eq!(reason("def f(x: Float) -> Float = x % 2.0\n", "f"), "uses `%` on a `Float`");
    assert_eq!(reason("def f(n: Int) -> Int = n ** 2\n", "f"), "uses `**`");
    assert_eq!(
        reason("def f(n: Int) -> Int\n  x = 1\n  x = 1.5\n  n\nend\n", "f"),
        "changes the type of `x` from `Int` to `Float`"
    );
    assert_eq!(reason("def f(n: Int) -> Float = n\n", "f"), "returns `Int` instead of `Float`");
    assert_eq!(
        reason("def f(n: Int) -> Int\n  if n\n    1\n  else\n    2\n  end\nend\n", "f"),
        "uses a `Int` as a condition"
    );
}

#[test]
fn reading_a_local_before_it_is_certainly_assigned_is_rejected() {
    // the interpreter raises NameError here; native code must not return zero instead
    let src = "def f(n: Int) -> Int\n  if n > 0\n    x = 1\n  end\n  x\nend\n";
    assert_eq!(reason(src, "f"), "may read `x` before assigning it");
    let src = "def f(n: Int) -> Int\n  while n > 0\n    x = n\n    n -= 1\n  end\n  x\nend\n";
    assert_eq!(reason(src, "f"), "may read `x` before assigning it");
    // both branches assign: certain
    compile_ok("def f(n: Int) -> Int\n  if n > 0\n    x = 1\n  else\n    x = 2\n  end\n  x\nend\n", "f");
    // one branch returns: certain on the other path
    compile_ok("def f(n: Int) -> Int\n  if n > 0\n    return 0\n  else\n    x = 2\n  end\n  x\nend\n", "f");
}

#[test]
fn a_function_calling_an_interpreted_one_is_interpreted_too() {
    let src = "\
def leaf(n: Int) -> Int
  puts n
  n
end
def caller(n: Int) -> Int = leaf(n) + 1
def pure(n: Int) -> Int = n + 1
";
    let (_, jit) = compile(src);
    assert_eq!(jit.report().compiled, ["pure"]);
    let reasons: Vec<&str> = jit.report().interpreted.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(reasons, ["leaf", "caller"]);
}

#[test]
fn arguments_must_match_the_signature_exactly() {
    let (program, jit) = compile("def half(x: Float) -> Float = x / 2.0\n");
    let half = function(program, "half");
    // an Int would be divided as an Int by the interpreter: the caller must interpret this call
    assert!(jit.call(half, &[Scalar::Int(3)], 100).is_none());
    assert!(jit.call(half, &[], 100).is_none());
    assert_eq!(jit.call(half, &[Scalar::Float(3.0)], 100), Some(Ok(Scalar::Float(1.5))));
}

fn compile_ok(src: &str, name: &str) {
    let (_, jit) = compile(src);
    assert!(jit.report().compiled.contains(&name.to_string()), "{:?}", jit.report());
}
