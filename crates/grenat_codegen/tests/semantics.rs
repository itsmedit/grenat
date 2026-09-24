//! Native results, edge case by edge case: they must be the interpreter's.

mod common;

use common::*;
use grenat_codegen::{Failure, Returned, Trap};

const ARITH: &str = "\
def div(a: Int, b: Int) -> Int = a / b
def rem(a: Int, b: Int) -> Int = a % b
def add(a: Int, b: Int) -> Int = a + b
def mul(a: Int, b: Int) -> Int = a * b
def neg(a: Int) -> Int = -a
def abs(a: Int) -> Int = a.abs
def cmp(a: Float, b: Float) -> Int = a <=> b
def mix(a: Int, b: Float) -> Float = a / b + a * 2.0
def to_i(x: Float) -> Int = x.to_i
";

#[test]
fn integer_division_rounds_toward_negative_infinity() {
    let (p, jit) = compile(ARITH);
    for (a, b, q, r) in [(7, 2, 3, 1), (-7, 2, -4, 1), (7, -2, -4, -1), (-7, -2, 3, -1), (6, 3, 2, 0), (-6, 3, -2, 0)] {
        assert_eq!(call(p, &jit, "div", &[int(a), int(b)]), Ok(int(q)), "{a} / {b}");
        assert_eq!(call(p, &jit, "rem", &[int(a), int(b)]), Ok(int(r)), "{a} % {b}");
    }
}

#[test]
fn integer_errors_are_trapped_not_wrapped() {
    let (p, jit) = compile(ARITH);
    assert_eq!(call(p, &jit, "div", &[int(1), int(0)]), Err(Trap::DivisionByZero));
    assert_eq!(call(p, &jit, "rem", &[int(1), int(0)]), Err(Trap::DivisionByZero));
    assert_eq!(call(p, &jit, "div", &[int(i64::MIN), int(-1)]), Err(Trap::Overflow));
    assert_eq!(call(p, &jit, "rem", &[int(i64::MIN), int(-1)]), Ok(int(0)));
    assert_eq!(call(p, &jit, "add", &[int(i64::MAX), int(1)]), Err(Trap::Overflow));
    assert_eq!(call(p, &jit, "mul", &[int(i64::MAX), int(2)]), Err(Trap::Overflow));
    assert_eq!(call(p, &jit, "neg", &[int(i64::MIN)]), Err(Trap::Overflow));
    assert_eq!(call(p, &jit, "abs", &[int(i64::MIN)]), Err(Trap::Overflow));
    assert_eq!(call(p, &jit, "abs", &[int(-5)]), Ok(int(5)));
}

#[test]
fn floats_mix_with_ints_like_the_interpreter() {
    let (p, jit) = compile(ARITH);
    assert_eq!(call(p, &jit, "mix", &[int(3), float(2.0)]), Ok(float(7.5)));
    assert_eq!(call(p, &jit, "cmp", &[float(1.0), float(2.0)]), Ok(int(-1)));
    assert_eq!(call(p, &jit, "cmp", &[float(f64::NAN), float(2.0)]), Ok(int(0)));
    // saturating conversion, NaN to 0 (Rust's `as`)
    assert_eq!(call(p, &jit, "to_i", &[float(-2.9)]), Ok(int(-2)));
    assert_eq!(call(p, &jit, "to_i", &[float(1e30)]), Ok(int(i64::MAX)));
    assert_eq!(call(p, &jit, "to_i", &[float(f64::NAN)]), Ok(int(0)));
}

#[test]
fn control_flow_locals_and_loops() {
    let src = "\
def sum_to(n: Int) -> Int
  total = 0
  i = 1
  while i <= n
    total += i
    i += 1
  end
  total
end
def gcd(a: Int, b: Int) -> Int
  while b != 0
    t = b
    b = a % b
    a = t
  end
  a
end
def sign(x: Float) -> Int
  if x > 0.0
    1
  elsif x < 0.0
    -1
  else
    0
  end
end
def first_even(a: Int, b: Int) -> Int
  return a if a.even?
  return b if b.even?
  -1
end
";
    let (p, jit) = compile(src);
    assert_eq!(jit.report().compiled.len(), 4, "{:?}", jit.report());
    assert_eq!(call(p, &jit, "sum_to", &[int(100)]), Ok(int(5050)));
    assert_eq!(call(p, &jit, "gcd", &[int(84), int(36)]), Ok(int(12)));
    assert_eq!(call(p, &jit, "sign", &[float(-3.5)]), Ok(int(-1)));
    assert_eq!(call(p, &jit, "sign", &[float(0.0)]), Ok(int(0)));
    assert_eq!(call(p, &jit, "first_even", &[int(3), int(4)]), Ok(int(4)));
    assert_eq!(call(p, &jit, "first_even", &[int(3), int(5)]), Ok(int(-1)));
}

#[test]
fn boolean_operators_short_circuit() {
    // `1 / 0` must never run
    let src = "\
def safe_and(n: Int) -> Bool = n != 0 && 10 / n > 1
def safe_or(n: Int) -> Bool = n == 0 || 10 / n > 1
def not_zero(n: Int) -> Bool = !n.zero?
";
    let (p, jit) = compile(src);
    assert_eq!(call(p, &jit, "safe_and", &[int(0)]), Ok(boolean(false)));
    assert_eq!(call(p, &jit, "safe_and", &[int(2)]), Ok(boolean(true)));
    assert_eq!(call(p, &jit, "safe_or", &[int(0)]), Ok(boolean(true)));
    assert_eq!(call(p, &jit, "not_zero", &[int(3)]), Ok(boolean(true)));
}

#[test]
fn recursion_mutual_recursion_and_its_limit() {
    let src = "\
def fib(n: Int) -> Int
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end
def is_even(n: Int) -> Bool = if n == 0 then true else is_odd(n - 1) end
def is_odd(n: Int) -> Bool = if n == 0 then false else is_even(n - 1) end
def forever(n: Int) -> Int = forever(n + 1)
";
    let (p, jit) = compile(src);
    assert_eq!(call(p, &jit, "fib", &[int(25)]), Ok(int(75025)));
    assert_eq!(call(p, &jit, "is_even", &[int(10)]), Ok(boolean(true)));
    assert_eq!(call(p, &jit, "is_odd", &[int(7)]), Ok(boolean(true)));
    assert_eq!(call(p, &jit, "forever", &[int(0)]), Err(Trap::StackOverflow));
    // the limit is the caller's remaining depth
    let fib = function(p, "fib");
    let value = |r: Option<Result<Returned, Failure>>| r.map(|r| r.map(|r| r.value));
    assert_eq!(value(jit.call(fib, &[int(20)], 5, &|| false)), Some(Err(Failure::Trap(Trap::StackOverflow))));
    assert_eq!(value(jit.call(fib, &[int(20)], 20, &|| false)), Some(Ok(int(6765))));
}

#[test]
fn native_code_is_safe_to_call_from_many_threads() {
    let (p, jit) = compile("def fib(n: Int) -> Int\n  return n if n < 2\n  fib(n - 1) + fib(n - 2)\nend\n");
    std::thread::scope(|s| {
        for n in 15..23 {
            let jit = &jit;
            s.spawn(move || {
                let expected = [610, 987, 1597, 2584, 4181, 6765, 10946, 17711][n as usize - 15];
                assert_eq!(call(p, jit, "fib", &[int(n)]), Ok(int(expected)));
            });
        }
    });
}

#[test]
fn native_loops_stop_when_their_task_is_cancelled() {
    let src = "\
def spin(n: Int) -> Int
  words = [\"a\", \"b\"]
  total = 0
  i = 0
  while i < n
    total += words[i % 2].length
    i += 1
  end
  total
end
def recurse(n: Int) -> Int = if n == 0 then 0 else 1 + recurse(n - 1) end
";
    let (p, jit) = compile(src);
    let asked = std::cell::Cell::new(0);
    let before = grenat_runtime::live_objects();
    // cancelled at the third checkpoint (the ticker raises one every 10 ms):
    // the loop stops, its array is released
    let cancel_third = || {
        asked.set(asked.get() + 1);
        asked.get() >= 3
    };
    let r = jit.call(function(p, "spin"), &[int(1_000_000_000_000)], 100, &cancel_third).unwrap();
    assert_eq!(r.map(|r| r.value), Err(Failure::Trap(Trap::Cancelled)));
    assert_eq!(asked.get(), 3);
    assert_eq!(grenat_runtime::live_objects(), before);
    // function entries are checkpoints too; an already cancelled task stops at once
    let r = jit.call(function(p, "recurse"), &[int(1_000)], 10_000, &|| true).unwrap();
    assert_eq!(r.map(|r| r.value), Err(Failure::Trap(Trap::Cancelled)));
    // never cancelled: runs to the end
    assert_eq!(call(p, &jit, "spin", &[int(100_000)]), Ok(int(100_000)));
}
