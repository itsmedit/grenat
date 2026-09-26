//! Strings, arrays and structs in native code: results, reference counting
//! (every call must free all it allocates, errors included), deoptimization.

mod common;

use common::*;
use grenat_codegen::{Data, Failure, Trap};

fn record(ty: &str, fields: &[(&str, Data)]) -> Data {
    Data::Record { ty: ty.into(), fields: fields.iter().map(|(n, d)| (n.to_string(), d.clone())).collect() }
}

fn point(x: f64, y: f64) -> Data {
    record("Point", &[("x", float(x)), ("y", float(y))])
}

const STRINGS: &str = r#"
def greet(name: String, n: Int) -> String = "hello #{name} × #{n}, #{n * 0.5} #{n > 1}"
def repeat(s: String, n: Int) -> String
  out = ""
  i = 0
  while i < n
    out = out + s
    i += 1
  end
  out
end
def twice(s: String) -> String = s + s
def shout(s: String) -> String = s.strip.upcase + "!"
def length(s: String) -> Int = s.length
def compare(a: String, b: String) -> Int = a <=> b
def same(a: String, b: String) -> Bool = a == b && !(a != b)
def char(s: String, i: Int) -> String = s[i]
def stars(n: Int) -> String = "*" * n
def unused(s: String, t: String) -> Int = 1
def pick(c: Bool, a: String, b: String) -> String
  if c
    a
  else
    b
  end
end
def fail_after(s: String, n: Int) -> String
  t = s + "x"
  u = t + t
  n / 0
  u
end
"#;

#[test]
fn strings_are_built_and_compared_like_the_interpreter() {
    let (p, jit) = compile(STRINGS);
    assert_eq!(jit.report().interpreted, []);
    assert_eq!(call(p, &jit, "greet", &[string("Ada"), int(3)]), Ok(string("hello Ada × 3, 1.5 true")));
    assert_eq!(call(p, &jit, "repeat", &[string("ab"), int(3)]), Ok(string("ababab")));
    assert_eq!(call(p, &jit, "twice", &[string("é")]), Ok(string("éé")));
    assert_eq!(call(p, &jit, "shout", &[string("  hey ")]), Ok(string("HEY!")));
    assert_eq!(call(p, &jit, "length", &[string("héllo")]), Ok(int(5)));
    assert_eq!(call(p, &jit, "compare", &[string("a"), string("b")]), Ok(int(-1)));
    assert_eq!(call(p, &jit, "same", &[string("a"), string("a")]), Ok(boolean(true)));
    assert_eq!(call(p, &jit, "char", &[string("héllo"), int(-4)]), Ok(string("é")));
    assert_eq!(call(p, &jit, "stars", &[int(3)]), Ok(string("***")));
    assert_eq!(call(p, &jit, "unused", &[string("a"), string("b")]), Ok(int(1)));
    assert_eq!(call(p, &jit, "pick", &[boolean(false), string("a"), string("b")]), Ok(string("b")));
}

#[test]
fn what_native_code_cannot_represent_deoptimizes_without_leaking() {
    let (p, jit) = compile(STRINGS);
    // `nil` in the interpreter
    assert_eq!(call_full(p, &jit, "char", &[string("ab"), int(2)]), Err(Failure::Deopt));
    // a `TypeError` in the interpreter
    assert_eq!(call_full(p, &jit, "stars", &[int(-1)]), Err(Failure::Deopt));
    // an error with objects alive: all released
    assert_eq!(call(p, &jit, "fail_after", &[string("s"), int(1)]), Err(Trap::DivisionByZero));
}

const ARRAYS: &str = "
def sum(xs: Array(Int)) -> Int
  total = 0
  i = 0
  while i < xs.length
    total += xs[i]
    i += 1
  end
  total
end
def squares(n: Int) -> Array(Int)
  out = []
  n.times do |i|
    out << i * i
  end
  out
end
def sieve(n: Int) -> Array(Int)
  composite = []
  (n + 1).times do |i|
    composite << false
  end
  primes = []
  2.upto(n) do |i|
    unless_composite = !composite[i]
    if unless_composite
      primes.push(i)
      j = i * i
      while j <= n
        composite[j] = true
        j += i
      end
    end
  end
  primes
end
def words(xs: Array(String)) -> String
  out = \"\"
  xs.each_with_index do |w, i|
    out = out + \"#{i}:#{w} \"
  end
  out
end
def fill(xs: Array(Int)) -> Int
  xs << 4
  xs[0] = 10
  xs[-1] += 1
  xs.pop + xs.sum
end
def first(xs: Array(String)) -> String = xs.first
def same(xs: Array(Int)) -> Array(Int) = xs
def both(a: Array(Int), b: Array(Int)) -> Array(Int) = a + b
def gap(xs: Array(Int)) -> Int
  xs[5] = 1
  0
end
def overflow(xs: Array(Int)) -> Int = xs.sum
";

#[test]
fn arrays_are_read_and_written_in_place() {
    let (p, jit) = compile(ARRAYS);
    assert_eq!(jit.report().interpreted, []);
    assert_eq!(call(p, &jit, "sum", &[ints(&[1, 2, 3])]), Ok(int(6)));
    assert_eq!(call(p, &jit, "squares", &[int(4)]), Ok(ints(&[0, 1, 4, 9])));
    assert_eq!(call(p, &jit, "sieve", &[int(30)]), Ok(ints(&[2, 3, 5, 7, 11, 13, 17, 19, 23, 29])));
    assert_eq!(call(p, &jit, "words", &[strings(&["a", "b"])]), Ok(string("0:a 1:b ")));
    assert_eq!(call(p, &jit, "first", &[strings(&["x", "y"])]), Ok(string("x")));
    assert_eq!(call(p, &jit, "both", &[ints(&[1]), ints(&[2])]), Ok(ints(&[1, 2])));
}

#[test]
fn array_arguments_come_back_with_their_new_content() {
    let (p, jit) = compile(ARRAYS);
    let returned = call_full(p, &jit, "fill", &[ints(&[1, 2])]).expect("a result");
    // [1, 2] << 4 → [10, 2, 4] → [10, 2, 5]; pop 5 → [10, 2]: 5 + 12
    assert_eq!(returned.value, int(17));
    assert_eq!(returned.arrays, [Some(vec![int(10), int(2)])]);
    // the very array given: an alias, not a copy
    assert_eq!(call_full(p, &jit, "same", &[ints(&[1])]).expect("a result").value, Data::Alias(0));
    // the same array twice
    let returned = call_full(p, &jit, "both", &[ints(&[1]), Data::Alias(0)]).expect("a result");
    assert_eq!(returned.value, ints(&[1, 1]));
}

#[test]
fn array_edge_cases_deoptimize_or_trap_without_leaking() {
    let (p, jit) = compile(ARRAYS);
    assert_eq!(call_full(p, &jit, "first", &[strings(&[])]), Err(Failure::Deopt));
    assert_eq!(call_full(p, &jit, "gap", &[ints(&[1])]), Err(Failure::Deopt));
    assert_eq!(call(p, &jit, "overflow", &[ints(&[i64::MAX, 1])]), Err(Trap::Overflow));
    // elements of the wrong type: interpreted
    assert!(jit.call(function(p, "sum"), &[strings(&["a"])], 100, &|| false).is_none());
}

const STRUCTS: &str = "
struct Point
  x: Float
  y: Float
end
struct Named
  name: String
  at: Point
end
def step(p: Point, n: Int) -> Point
  i = 0
  while i < n
    p = Point(x: p.x + 1.0, y: p.y)
    i += 1
  end
  p
end
def label(n: Named) -> String = \"#{n.name}@#{n.at.x}\"
def make(name: String) -> Named = Named(name: name, at: Point.new(x: 1.0, y: 2.0))
def rename(n: Named, name: String) -> Named = Named(name: name + n.name, at: n.at)
def norm(p: Point) -> Float = Math.sqrt(p.x * p.x + p.y * p.y)
";

#[test]
fn structs_are_built_read_and_reused() {
    let (p, jit) = compile(STRUCTS);
    assert_eq!(jit.report().interpreted, []);
    assert_eq!(call(p, &jit, "step", &[point(0.0, 5.0), int(1000)]), Ok(point(1000.0, 5.0)));
    let named = record("Named", &[("name", string("a")), ("at", point(3.0, 4.0))]);
    assert_eq!(call(p, &jit, "label", std::slice::from_ref(&named)), Ok(string("a@3.0")));
    assert_eq!(
        call(p, &jit, "make", &[string("b")]),
        Ok(record("Named", &[("name", string("b")), ("at", point(1.0, 2.0))]))
    );
    assert_eq!(
        call(p, &jit, "rename", &[named, string("x")]),
        Ok(record("Named", &[("name", string("xa")), ("at", point(3.0, 4.0))]))
    );
    assert_eq!(call(p, &jit, "norm", &[point(3.0, 4.0)]), Ok(float(5.0)));
    // a record of another type, or with a field of another type: interpreted
    assert!(jit.call(function(p, "norm"), &[record("Other", &[])], 100, &|| false).is_none());
    let wrong = record("Point", &[("x", int(1)), ("y", float(2.0))]);
    assert!(jit.call(function(p, "norm"), &[wrong], 100, &|| false).is_none());
}

#[test]
fn unsupported_object_code_stays_interpreted_with_a_reason() {
    let (_, jit) = compile(
        "def f(xs: Array(Int)) -> Int\n  xs.each do |x|\n    return x\n  end\n  0\nend\n\
         def g(xs: Array(Int)) -> Int\n  ys = []\n  ys.length\nend\n\
         def h(s: String) -> Bool = s == 1\n",
    );
    let reasons: Vec<(String, String)> = jit.report().interpreted.clone();
    assert_eq!(
        reasons,
        [
            ("f".to_string(), "returns from inside a block".to_string()),
            ("g".to_string(), "uses an empty array `[]` whose element type is unknown".to_string()),
            ("h".to_string(), "applies an operator to `String` and `Int`".to_string()),
        ]
    );
}

#[test]
fn uniquely_owned_values_are_updated_in_place() {
    let allocated = |f: &dyn Fn()| {
        let before = grenat_runtime::allocations();
        f();
        grenat_runtime::allocations() - before
    };
    let (p, jit) = compile(STRUCTS);
    // 1000 new points, each built in the memory of the previous one
    let n = allocated(&|| {
        call(p, &jit, "step", &[point(0.0, 0.0), int(1000)]).unwrap();
    });
    assert!(n <= 2, "{n} allocations");
    let (p, jit) = compile(STRINGS);
    // `out = out + s` appends in place: `out` is unique
    let n = allocated(&|| {
        call(p, &jit, "repeat", &[string("ab"), int(1000)]).unwrap();
    });
    assert!(n <= 3, "{n} allocations");
}

const OWNERSHIP: &str = r##"
def id(s: String) -> String = s
def both(a: String, b: String) -> String = a + "|" + b
def moved_and_borrowed(s: String) -> String = both(s, id(s))
def find(xs: Array(String), wanted: String) -> Int
  i = 0
  while i < xs.length
    w = xs[i]
    return i if w == wanted
    i += 1
  end
  -1
end
def branches(c: Int, s: String, t: String) -> String
  u = s + t
  if c > 1
    return u
  elsif c > 0
    v = t
    u = v + v
  else
    u = "none"
  end
  u
end
def dies_in_one_branch(c: Bool, s: String) -> Int
  if c
    s.length
  else
    0
  end
end
def short(s: String, t: String) -> Bool = s.length > 0 && s == t || t.empty?
def rebind(n: Int) -> String
  s = "a"
  n.times do |i|
    s = s + i.to_s
    t = s
    s = t + "-"
  end
  s
end
def nested(words: Array(String)) -> Array(String)
  out = []
  words.each do |w|
    out << w + w
    out << "#{w}!"
  end
  out
end
"##;

#[test]
fn every_path_releases_exactly_what_it_owns() {
    let (p, jit) = compile(OWNERSHIP);
    assert_eq!(jit.report().interpreted, []);
    assert_eq!(call(p, &jit, "moved_and_borrowed", &[string("s")]), Ok(string("s|s")));
    let xs = strings(&["a", "b", "c"]);
    assert_eq!(call(p, &jit, "find", &[xs.clone(), string("b")]), Ok(int(1)));
    assert_eq!(call(p, &jit, "find", &[xs, string("z")]), Ok(int(-1)));
    for (c, expected) in [(2, "ab"), (1, "bb"), (0, "none")] {
        assert_eq!(call(p, &jit, "branches", &[int(c), string("a"), string("b")]), Ok(string(expected)));
    }
    assert_eq!(call(p, &jit, "dies_in_one_branch", &[boolean(false), string("abc")]), Ok(int(0)));
    assert_eq!(call(p, &jit, "dies_in_one_branch", &[boolean(true), string("abc")]), Ok(int(3)));
    assert_eq!(call(p, &jit, "short", &[string(""), string("")]), Ok(boolean(true)));
    assert_eq!(call(p, &jit, "short", &[string("a"), string("b")]), Ok(boolean(false)));
    assert_eq!(call(p, &jit, "rebind", &[int(3)]), Ok(string("a0-1-2-")));
    assert_eq!(call(p, &jit, "nested", &[strings(&["x", "y"])]), Ok(strings(&["xx", "x!", "yy", "y!"])));
}

#[test]
fn native_objects_stay_on_their_thread() {
    let (p, jit) = compile(OWNERSHIP);
    std::thread::scope(|scope| {
        for t in 0..8 {
            let jit = &jit;
            scope.spawn(move || {
                for i in 0..200 {
                    let expected = format!("{t}-{i}|{t}-{i}");
                    let arg = string(&format!("{t}-{i}"));
                    assert_eq!(call(p, jit, "moved_and_borrowed", &[arg]), Ok(string(&expected)));
                }
            });
        }
    });
}
