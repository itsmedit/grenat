//! Strings and arrays, checked: slices are `String?` / `Array(T)?`, the
//! Ruby methods added to strings, arrays and hashes are typed, `format` and
//! `%` are strings (secrets when a secret is written in them), and taint
//! goes through all of them.

mod common;

use common::*;

#[test]
fn the_new_methods_are_typed() {
    clean(
        "\
def head(s: String) -> String? = s[0, 4]
def tail(s: String) -> String? = s[1..]
def middle(s: String) -> String? = s[1...-1]
def first(s: String) -> String = s[0]
def part(s: String) -> String? = s.slice(0, 2)
def inner(xs: Array(Int)) -> Array(Int)? = xs[1..-2]
def some(xs: Array(Int)) -> Array(Int)? = xs[0, 2]
def item(xs: Array(Int)) -> Int = xs.slice(0)
def flat(xs: Array(Array(Int))) -> Array(Int) = xs.flatten
def deep(xs: Array(Array(Array(Int)))) -> Array(Int) = xs.flatten
def level(xs: Array(Array(Array(Int)))) -> Array(Array(Int)) = xs.flatten(1)
def total(xs: Array(Int)) -> Int = xs.reduce(:+)
def product(xs: Array(Float)) -> Float = xs.inject(1.0, :*)
def summed(xs: Array(Int)) -> Int = xs.reduce(&:+)
def bare(s: String) -> String = s.delete_prefix(\"v\").delete_suffix(\".rb\").center(10, \"*\").tr(\"a-z\", \"A-Z\")
def tidy(s: String) -> String = s.squeeze.swapcase.delete(\"-\").ljust(4, \".\").rjust(6)
def vowels(s: String) -> Int = s.count(\"aeiou\")
def either(s: String) -> Bool = s.start_with?(\"a\", \"b\") || s.end_with?(\"x\", \"y\")
def pairs(xs: Array(Int)) -> Array(Array(Int)) = xs.each_slice(2)
def windows(xs: Array(Int)) -> Array(Array(Int)) = xs.each_cons(2)
def ranged -> Array(Array(Int)) = (1..9).each_slice(3)
def bounds(xs: Array(Int)) -> Array(Int) = xs.minmax
def top(xs: Array(Int)) -> Array(Int) = xs.max(2) + xs.min(2)
def doubled(xs: Array(Int)) -> Array(Int) = xs.each_with_object([]) { |x, acc| acc << x * 2 }
def kept(xs: Array(Int?)) -> Array(Int) = xs.filter_map { |x| x }
def leading(xs: Array(Int)) -> Array(Int) = xs.take_while { |x| x > 0 } + xs.drop_while { |x| x > 0 }
def single(xs: Array(Int)) -> Bool = xs.one? { |x| x > 1 }
def spin(xs: Array(Int)) -> Array(Int) = xs.rotate(2)
def grid(xs: Array(Int)) -> Array(Array(Int)) = xs.product(xs)
def desc(xs: Array(Int)) -> Array(Int) = xs.sort { |a, b| b <=> a }
def at(xs: Array(Int)) -> Int? = xs.find_index { |x| x > 1 }
def ones(xs: Array(Int)) -> Int = xs.count(1)
def by_parity(xs: Array(Int)) -> Array(Int) = xs.uniq { |x| x % 2 }
def table(xs: Array(Int)) -> Hash(Int, Int) = xs.to_h { |x| [x, x * x] }
def tens(h: Hash(String, Int)) -> Hash(String, Int) = h.transform_values { |v| v * 10 }
def symbols(h: Hash(String, Int)) -> Hash(Symbol, Int) = h.transform_keys { |k| k.to_sym }
def names(h: Hash(String, Int)) -> Array(String) = h.filter_map { |k, v| k if v > 1 }
def merged(a: Hash(String, Int), b: Hash(String, Int)) -> Hash(String, Int) = a.merge(b) { |k, x, y| x + y }
def found(h: Hash(String, Int)) -> Int = h.fetch(\"a\") { |k| k.size }
def counted(h: Hash(String, Int)) -> Int = h.count { |k, v| v > 1 }
def nothing(h: Hash(String, Int)) -> Bool = h.none? { |k, v| v > 1 }
def flipped(h: Hash(String, Int)) -> Hash(Int, String) = h.each_with_object({}) { |pair, acc| acc[pair[1]] = pair[0] }
def nested(h: Hash(String, Int)) = h.dig(\"a\")
def money(x: Float) -> String = format(\"%.2f\", x)
def percent(x: Float) -> String = \"%.1f%%\" % x
def both(a: String, b: Int) -> String = \"%s-%05d\" % [a, b]
def size(s: String) -> Int = s.then { |v| v.size }
",
    );
}

#[test]
fn their_results_have_their_types() {
    for (src, at, says) in [
        ("def f(s: String) -> Int = s[0, 2]\n", "s[0, 2]", "`String?`"),
        ("def f(xs: Array(Int)) -> Int = xs[0..1]\n", "xs[0..1]", "`Array(Int)?`"),
        ("def f(s: String) -> Int = s.delete_suffix(\"x\")\n", "s.delete_suffix(\"x\")", "`String`"),
        ("def f(xs: Array(String)) -> Int = xs.reduce(:+)\n", "xs.reduce(:+)", "`String`"),
        ("def f(xs: Array(Array(String))) -> Array(Int) = xs.flatten\n", "xs.flatten", "`Array(String)`"),
        ("def f(s: String) -> String = s.then { |v| v.size }\n", "s.then { |v| v.size }", "`Int`"),
        ("def f(x: Float) -> Int = format(\"%.2f\", x)\n", "format(\"%.2f\", x)", "`String`"),
        (
            "def f(h: Hash(String, Int)) -> Hash(String, String) = h.transform_values { |v| v * 2 }\n",
            "h.transform_values { |v| v * 2 }",
            "`Hash(String, Int)`",
        ),
    ] {
        let d = single(src, "E0200", at);
        assert!(d.message.contains(says), "{src}: {}", d.message);
    }
}

#[test]
fn indexes_are_checked() {
    for (src, at, says) in [
        ("def f(s: String) = s[0, \"1\"]\n", "s[0, \"1\"]", "takes two `Int`s, got `String`"),
        ("def f(s: String) = s[\"a\"]\n", "s[\"a\"]", "`String` is indexed by an `Int` or a `Range`, got `String`"),
        ("def f(xs: Array(Int)) = xs[0, 1, 2]\n", "xs[0, 1, 2]", "one or two values, got 3"),
        ("def f(h: Hash(String, Int)) = h[\"a\", \"b\"]\n", "h[\"a\", \"b\"]", "one key, got 2"),
        ("def f(s: String) = s.then\n", "then", "`then` expects a block"),
    ] {
        let d = single(src, "E0200", at);
        assert!(d.message.contains(says), "{src}: {}", d.message);
    }
    // what Grenat still has not: regular expressions, `scan`
    let d = single("def f(s: String) = s.scan(\"a\")\n", "E0200", "scan");
    assert!(d.message.contains("unknown method `scan` for `String`"), "{}", d.message);
}

#[test]
fn a_secret_is_never_sliced_and_formats_into_a_secret() {
    for (body, at, says) in [
        ("t[0, 4]", "t[0, 4]", "a secret cannot be indexed nor sliced"),
        ("t[1..]", "t[1..]", "a secret cannot be indexed nor sliced"),
        ("t.delete_prefix(\"sk-\")", "delete_prefix", "a secret has no method `delete_prefix`"),
        ("t.then { |x| x }", "then", "a secret has no method `then`"),
    ] {
        let src = format!("def f uses env\n  t = Credentials.fetch(:api, :token)\n  {body}\nend\n");
        let d = single(&src, "E0414", at);
        assert!(d.message.contains(says), "{body}: {}", d.message);
    }
    // a secret written in a format makes a secret: fine in a header, not where text goes
    clean(
        "\
def call uses env, net(\"api.x.com\")
  token = Credentials.fetch(:api, :token)
  Http.get(\"https://api.x.com\", headers: {\"Authorization\" => format(\"Bearer %s\", token)})
  Http.get(\"https://api.x.com\", headers: {\"Authorization\" => \"Bearer %s\" % token})
end
",
    );
    let src =
        "def show(text: String) = text\ndef f uses env\n  show(format(\"%s\", Credentials.fetch(:api, :token)))\nend\n";
    let d = single(src, "E0414", "format(\"%s\", Credentials.fetch(:api, :token))");
    assert!(d.message.contains("a secret is not one"), "{}", d.message);
}

#[test]
fn what_comes_from_untrusted_text_is_untrusted() {
    for derived in [
        "s.title[0, 4].to_s",
        "s.title[1..].to_s",
        "s.title.delete_prefix(\"x\")",
        "s.title.tr(\"a\", \"b\")",
        "\"ab\".ljust(9, s.title)",
        "\"a\".sub(\"a\", s.title)",
        "format(\"%s\", s.title)",
        "\"%s!\" % s.title",
        "s.title.then { |t| t }",
        "s.bullets.each_slice(2).flatten.join",
        "[1].zip([s.title]).flatten.join",
        "[1].each_with_object([]) { |x, acc| acc << s.title }.join",
        "{a: 1}.transform_values { |v| s.title }.values.join",
        "{a: \"1\"}.merge({b: s.title}).values.join",
        "s.bullets.reduce(:+).to_s",
        "s.bullets.filter_map { |b| b }.join",
    ] {
        let src = format!("{PRELUDE}s = summarize(\"x\")\nsend(\"a@b.c\", {derived})\n");
        let d = diags(&src);
        assert!(
            d.iter().any(|d| d.code == Some("E0412") && &src[d.span.range()] == derived),
            "{derived}:\n{}",
            render(&src, &d)
        );
    }
    let src = format!("{PRELUDE}s = summarize(\"x\")\nsend(\"a@b.c\", s.title.trust![0, 4].to_s)\n");
    clean(&src);
}
