//! Native code must be invisible: every program behaves exactly the same with
//! and without the JIT (differential tests), only faster.

mod common;

use common::*;
use grenat_interp::Scripted;

fn with_jit(src: &str, jit: bool) -> Run {
    run_mode(src, Scripted::new([]), &[], &[], Mode { jit, log: false, ..Mode::default() })
}

/// Runs `src` interpreted and native; asserts identical output and outcome.
fn same_both_ways(src: &str) -> String {
    let (native, interpreted) = (with_jit(src, true), with_jit(src, false));
    assert_eq!(native.output, interpreted.output, "output differs with the JIT");
    assert_eq!(
        native.result.as_ref().map(|_| ()).map_err(|e| (&e.ty, &e.message)),
        interpreted.result.as_ref().map(|_| ()).map_err(|e| (&e.ty, &e.message)),
        "outcome differs with the JIT"
    );
    native.output
}

/// Functions used by the differential tests; every one of them is compiled.
const NUMERIC: &str = "\
def div(a: Int, b: Int) -> Int = a / b
def rem(a: Int, b: Int) -> Int = a % b
def poly(x: Int) -> Int = 3 * x * x - 7 * x + 11
def mean(a: Int, b: Float) -> Float = (a + b) / 2.0
def clamp(x: Float, lo: Float, hi: Float) -> Float
  return lo if x < lo
  return hi if x > hi
  x
end
def collatz(n: Int) -> Int
  steps = 0
  while n != 1
    if n.even?
      n = n / 2
    else
      n = 3 * n + 1
    end
    steps += 1
  end
  steps
end
def fib(n: Int) -> Int
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end
def cmp(a: Float, b: Float) -> Int = a <=> b
def both(a: Bool, b: Bool) -> Bool = a && !b || b && !a
";

#[test]
fn the_numeric_functions_are_really_compiled() {
    let src = format!("{NUMERIC}puts 1\n");
    let r = run_mode(&src, Scripted::new([]), &[], &[], Mode { jit: true, log: true, ..Mode::default() });
    assert!(
        r.output.starts_with("[jit] native: div, rem, poly, mean, clamp, collatz, fib, cmp, both\n"),
        "{}",
        r.output
    );
}

#[test]
fn arithmetic_is_identical_on_many_inputs() {
    let driver = "\
values = [-7, -3, -2, -1, 0, 1, 2, 3, 7, 9223372036854775807, -9223372036854775807 - 1]
values.each do |a|
  values.each do |b|
    [:div, :rem].each do |op|
      begin
        r = if op == :div then div(a, b) else rem(a, b) end
        puts \"#{a} #{op} #{b} = #{r}\"
      rescue ZeroDivisionError, OverflowError => e
        puts \"#{a} #{op} #{b} raises #{e.type}\"
      end
    end
  end
  begin
    puts \"poly(#{a}) = #{poly(a)}\"
  rescue OverflowError
    puts \"poly(#{a}) overflows\"
  end
end
";
    let out = same_both_ways(&format!("{NUMERIC}{driver}"));
    assert!(out.contains("-7 div 2 = -4\n"), "{out}");
    assert!(out.contains("-9223372036854775808 div -1 raises OverflowError\n"), "{out}");
    assert!(out.contains("-9223372036854775808 rem -1 = 0\n"), "{out}");
    assert!(out.contains("1 div 0 raises ZeroDivisionError\n"), "{out}");
    assert!(out.contains("poly(9223372036854775807) overflows\n"), "{out}");
}

#[test]
fn floats_loops_and_recursion_are_identical() {
    let driver = "\
p [mean(3, 4.5), mean(-1, 0.5)]
p [clamp(-2.0, 0.0, 1.0), clamp(0.25, 0.0, 1.0), clamp(9.0, 0.0, 1.0)]
p (1..30).to_a.map { |n| collatz(n) }
p (0..20).to_a.map { |n| fib(n) }
p [cmp(1.0, 2.0), cmp(2.0, 2.0), cmp(0.0 / 0.0, 1.0)]
p [both(true, false), both(true, true), both(false, false)]
";
    let out = same_both_ways(&format!("{NUMERIC}{driver}"));
    assert!(out.starts_with("[3.75, -0.25]\n[0.0, 0.25, 1.0]\n"), "{out}");
}

#[test]
fn arguments_of_another_type_are_interpreted() {
    // `mean` expects (Int, Float): with a Float first, the interpreter runs it — same result either way
    same_both_ways(&format!("{NUMERIC}p mean(2.5, 1.5)\np div(7, 2)\n"));
}

#[test]
fn deep_recursion_raises_the_same_error() {
    let src = "def down(n: Int) -> Int = if n == 0 then 0 else 1 + down(n - 1) end\np down(100)\np down(1_000_000)\n";
    let native = with_jit(src, true);
    assert_eq!(native.output, "100\n");
    assert_eq!(native.err().ty, "StackOverflow");
    assert_eq!(with_jit(src, false).err().ty, "StackOverflow");
}

#[test]
fn native_errors_carry_the_function_in_their_trace() {
    let e = with_jit("def div(a: Int, b: Int) -> Int = a / b\ndiv(1, 0)\n", true).err();
    assert_eq!((e.ty.as_str(), e.message.as_str()), ("ZeroDivisionError", "division by zero"));
    assert_eq!(e.trace[0].0, "div");
}

#[test]
fn taint_flows_through_native_code() {
    let src = "\
model :m, name: \"claude-haiku-4-5\"
prompt count(text: String) -> ~Int using :m
  user text
end
def double(n: Int) -> Int = n * 2
p double(count(\"x\"))
p double(21)
";
    let provider = Scripted::new([grenat_interp::Response::json_reply(serde_json::json!({"value": 5}))]);
    let r = run_mode(src, provider, &[], &[], Mode::default());
    assert_eq!(r.ok(), "~10\n42\n");
}

#[test]
fn native_code_is_much_faster() {
    let src = "def fib(n: Int) -> Int\n  return n if n < 2\n  fib(n - 1) + fib(n - 2)\nend\np fib(24)\n";
    let (native, interpreted) = (with_jit(src, true), with_jit(src, false));
    assert_eq!(native.output, "46368\n");
    assert_eq!(interpreted.output, "46368\n");
    assert!(
        native.elapsed * 5 < interpreted.elapsed,
        "native {:?} vs interpreted {:?}",
        native.elapsed,
        interpreted.elapsed
    );
}

/// Functions over strings, arrays and structs; every one of them is compiled.
const OBJECTS: &str = "\
struct Point
  x: Float
  y: Float
end
def label(p: Point) -> String = \"(#{p.x}, #{p.y})\"
def shift(p: Point, n: Int) -> Point
  n.times do |i|
    p = Point(x: p.x + 1.0, y: p.y - 0.5)
  end
  p
end
def join(words: Array(String)) -> String
  out = \"\"
  words.each_with_index do |w, i|
    out = out + \"#{i}=#{w.upcase};\"
  end
  out
end
def grow(xs: Array(Int)) -> Array(Int)
  xs << xs.length
  xs[0] += 100
  xs
end
def fresh(n: Int) -> Array(Int)
  out = []
  1.upto(n) do |i|
    out << i * i
  end
  out
end
def at(xs: Array(Int), i: Int) -> Int = xs[i]
def mutate_then_fail(xs: Array(Int)) -> Int
  xs << 7
  xs[0] / 0
end
def pair(a: Array(Int), b: Array(Int)) -> Int
  a << 1
  b.length
end
";

#[test]
fn the_object_functions_are_really_compiled() {
    let src = format!("{OBJECTS}puts 1\n");
    let r = run_mode(&src, Scripted::new([]), &[], &[], Mode { jit: true, log: true, ..Mode::default() });
    assert!(
        r.output.starts_with("[jit] native: label, shift, join, grow, fresh, at, mutate_then_fail, pair\n"),
        "{}",
        r.output
    );
}

#[test]
fn objects_behave_identically_natively() {
    let driver = "\
p label(shift(Point(x: 0.0, y: 1.0), 3))
p join([\"a\", \"é\"])
xs = [1, 2]
ys = grow(xs)
ys << 9
p [xs, ys]
p fresh(5)
p [at([1, 2, 3], -1), at([1, 2, 3], 7)]
zs = [5]
begin
  mutate_then_fail(zs)
rescue ZeroDivisionError => e
  p [e.type, zs]
end
same = [1]
p [pair(same, same), same]
p label(Point(x: 1, y: 2.0))
";
    let out = same_both_ways(&format!("{OBJECTS}{driver}"));
    let expected = "\
\"(3.0, -0.5)\"
\"0=A;1=É;\"
[[101, 2, 2, 9], [101, 2, 2, 9]]
[1, 4, 9, 16, 25]
[3, nil]
[\"ZeroDivisionError\", [5, 7]]
[2, [1, 1]]
\"(1, 2.0)\"
";
    assert_eq!(out, expected);
}

#[test]
fn deoptimized_calls_are_interpreted() {
    // `xs[7]` is nil: native code gives up, the interpreter computes `nil`
    let r = with_jit(&format!("{OBJECTS}p at([1], 7)\n"), true);
    assert_eq!(r.ok(), "nil\n");
}

#[test]
fn a_cancelled_task_stops_inside_native_code() {
    let src = "\
def spin(n: Int) -> Int
  i = 0
  while i < n
    i += 1
  end
  i
end
p(race do
  spin(1_000_000_000_000)
  spin(10)
end)
";
    let started = std::time::Instant::now();
    let r = with_jit(src, true);
    assert_eq!(r.ok(), "10\n");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}
