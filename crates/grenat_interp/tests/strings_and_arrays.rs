//! Strings and arrays as Ruby has them: slices (`s[0, 4]`, `s[1..]`,
//! `xs[1..-2]`), `flatten`, the usual string, array and hash methods,
//! `reduce(:+)`, `then`, `format` and `%` — with taint kept and secrets
//! never revealed.

mod common;

use common::*;
use grenat_interp::Response;

fn lines(src: &str) -> Vec<String> {
    run(src).lines().map(str::to_string).collect()
}

fn error_of(src: &str) -> String {
    let e = run_err(src, Vec::new());
    format!("{}: {}", e.ty, e.message)
}

const UNTRUSTED: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt say(x: String) -> ~String using :fast
  user x
end
";

const CREDENTIALS: &str = "mock_credentials({\"api\" => {\"token\" => \"sk-12345678\"}})\n";

#[test]
fn strings_are_sliced_by_characters() {
    let out = lines(
        "\
s = \"héllo wörld\"
p [s[0, 5], s[6, 100], s[-5, 3], s[11, 2], s[12, 1], s[0, -1]]
p [s[0..4], s[0...5], s[6..-1], s[-5..], s[6...], s[4..1], s[11..], s[12..]]
p [s[1], s[-1], s[20], s.slice(0, 5), s.slice(6..-1), s.slice(0), s.slice(11)]
",
    );
    assert_eq!(
        out,
        [
            "[\"héllo\", \"wörld\", \"wör\", \"\", nil, nil]",
            "[\"héllo\", \"héllo\", \"wörld\", \"wörld\", \"wörld\", \"\", \"\", nil]",
            "[\"é\", \"d\", nil, \"héllo\", \"wörld\", \"h\", nil]",
        ]
    );
}

#[test]
fn arrays_are_sliced_and_flattened() {
    let out = lines(
        "\
xs = [1, 2, 3, 4, 5]
p [xs[1, 2], xs[1..-2], xs[1...-1], xs[-2..], xs[5, 1], xs[5..], xs.slice(1, 2), xs.slice(-1)]
p [xs[6, 1], xs[-6..], xs[0, -1]]
p [[1, [2, [3, [4]]]].flatten, [1, [2, [3, [4]]]].flatten(1), [[1], [2]].flatten(0), [].flatten]
p [[[1, [2, [3]]]].flatten(-1), [[1, [2]]].flatten(-5), [[1, [2, [3]]]].flatten(2)]
",
    );
    assert_eq!(
        out,
        [
            "[[2, 3], [2, 3, 4], [2, 3, 4], [4, 5], [], [], [2, 3], 5]",
            "[nil, nil, nil]",
            "[[1, 2, 3, 4], [1, 2, [3, [4]]], [[1], [2]], []]",
            // a negative depth flattens every level, as in Ruby
            "[[1, 2, 3], [1, 2], [1, 2, [3]]]",
        ]
    );
}

#[test]
fn slicing_errors_say_why() {
    assert_eq!(
        error_of("p \"abc\"[0, \"1\"]"),
        "TypeError: a slice `[start, length]` takes two integers, got Int and String"
    );
    assert_eq!(error_of("p [1][0, 1, 2]"), "ArgumentError: an index takes one or two values, got 3");
    assert_eq!(error_of("p [[1]].flatten(1, 2)"), "ArgumentError: `flatten` takes one depth at most, got 2");
    assert_eq!(error_of("p \"abc\"[\"a\"]"), "TypeError: String cannot be indexed by String");
}

#[test]
fn slices_are_read_only() {
    for (src, says) in [
        ("xs = [1, 2, 3]\nxs[0, 2] = 9", "TypeError: a slice cannot be assigned"),
        ("xs = [1, 2, 3]\nxs[1..] = [5]", "TypeError: a slice cannot be assigned"),
        ("xs = [1, 2, 3]\nxs[0, 2] += [9]", "TypeError: a slice cannot be assigned"),
        ("s = \"abc\"\ns[1..] = \"z\"", "TypeError: a string cannot be changed in place"),
        ("s = \"abc\"\ns[0] = \"z\"", "TypeError: a string cannot be changed in place"),
    ] {
        assert!(error_of(src).starts_with(says), "{src}: {}", error_of(src));
    }
    // an item still can
    assert_eq!(
        run("xs = [1, 2, 3]\nxs[0] = 9\nxs[-1] = 7\nh = {}\nh[1..2] = 3\np xs, h\n"),
        "[9, 2, 7]\n{1..2 => 3}\n"
    );
}

#[test]
fn character_sets_are_intersected() {
    // as Ruby: a character counts when every set holds it
    let out = lines(
        "\
p [\"hello world\".count(\"lo\", \"o\"), \"hello\".count(\"a-y\", \"^l\"), \"hello\".count(\"lo\", \"\")]
p [\"hello world\".delete(\"lo\", \"o\"), \"aaabbboo\".squeeze(\"ab\", \"b\")]
",
    );
    assert_eq!(out, ["[2, 3, 0]", "[\"hell wrld\", \"aaaboo\"]"]);
    assert_eq!(error_of("p \"a\".count(\"a\", 1)"), "TypeError: `count` expects a string, got Int");
}

#[test]
fn string_methods() {
    let out = lines(
        "\
p [\"v1.2\".delete_prefix(\"v\"), \"v1.2\".delete_prefix(\"x\"), \"a.rb\".delete_suffix(\".rb\")]
p [\"ab\".center(6, \"*\"), \"ab\".center(7), \"ab\".ljust(6, \"-=\"), \"ab\".rjust(5, \"0\"), \"abc\".center(2)]
p [\"hello\".count(\"l\"), \"hello\".count(\"a-y\"), \"hello\".count(\"^l\")]
p [\"hello\".tr(\"el\", \"ip\"), \"hello\".tr(\"a-z\", \"A-Z\"), \"hello\".tr(\"^l\", \"*\"), \"hello\".delete(\"l\")]
p [\"aaa  bbb\".squeeze, \"aaa  bbb\".squeeze(\" \"), \"Hello World\".swapcase]
p [\"abc\".start_with?(\"x\", \"ab\"), \"abc\".end_with?(\"x\", \"y\"), \"abc\".start_with?(\"a\")]
",
    );
    assert_eq!(
        out,
        [
            "[\"1.2\", \"v1.2\", \"a\"]",
            "[\"**ab**\", \"  ab   \", \"ab-=-=\", \"000ab\", \"abc\"]",
            "[2, 5, 3]",
            "[\"hippo\", \"HELLO\", \"**ll*\", \"heo\"]",
            "[\"a b\", \"aaa bbb\", \"hELLO wORLD\"]",
            "[true, false, true]",
        ]
    );
    assert_eq!(error_of("p \"a\".ljust(3, \"\")"), "ArgumentError: `ljust`: the padding is empty");
}

#[test]
fn array_methods() {
    let out = lines(
        "\
xs = [3, 1, 4, 1, 5]
p [xs.each_slice(2).to_a, xs.each_cons(4).to_a, [1, 2].each_cons(3).to_a]
seen = []
p xs.each_slice(2) { |pair| seen << pair.sum }
p seen
p [xs.max(2), xs.min(3), xs.max(0), [].max(1)]
p [xs.minmax, [].minmax, xs.rotate, xs.rotate(-1), xs.rotate(7), [].rotate]
p [xs.take_while { |x| x < 4 }, xs.drop_while { |x| x < 4 }, xs.one? { |x| x > 4 }, [nil, 1].one?]
p [xs.find_index { |x| x > 3 }, xs.find_index { |x| x > 9 }, xs.count(1), xs.uniq { |x| x % 2 }]
p [xs.sort { |a, b| b <=> a }, [1, nil, 2].filter_map { |x| x && x * 10 }]
p xs.each_with_object([]) { |x, acc| acc << x * 2 }
p [[1, 2].product([3, 4]), [1, 2].product([3], [5, 6]), [1, 2].product]
p [[1, 2].zip([3, 4], [5]), [[:a, 1], [:b, 2]].to_h, [1, 2].to_h { |x| [x, x * x] }]
p [{a: [10, {b: 20}]}.dig(:a, 1, :b), [[1, [2]]].dig(0, 1, 0), {a: 1}.dig(:z, :y)]
",
    );
    assert_eq!(
        out,
        [
            "[[[3, 1], [4, 1], [5]], [[3, 1, 4, 1], [1, 4, 1, 5]], []]",
            "[3, 1, 4, 1, 5]",
            "[4, 5, 5]",
            "[[5, 4], [1, 1, 3], [], []]",
            "[[1, 5], [nil, nil], [1, 4, 1, 5, 3], [5, 3, 1, 4, 1], [4, 1, 5, 3, 1], []]",
            "[[3, 1], [4, 1, 5], true, true]",
            "[2, nil, 2, [3, 4]]",
            "[[5, 4, 3, 1, 1], [10, 20]]",
            "[6, 2, 8, 2, 10]",
            "[[[1, 3], [1, 4], [2, 3], [2, 4]], [[1, 3, 5], [1, 3, 6], [2, 3, 5], [2, 3, 6]], [[1], [2]]]",
            "[[[1, 3, 5], [2, 4, nil]], {a: 1, b: 2}, {1 => 1, 2 => 4}]",
            "[20, 2, nil]",
        ]
    );
    assert_eq!(error_of("p [1].each_slice(0).to_a"), "ArgumentError: `each_slice` expects a positive size, got 0");
    assert_eq!(
        error_of("p [2, 1].sort { |a, b| a > b }"),
        "TypeError: `sort`: the block compares as `<=>` does (an integer), got Bool"
    );
    assert_eq!(error_of("p [1].to_h"), "TypeError: `to_h` expects [key, value] pairs, got 1");
}

#[test]
fn reduce_takes_an_operator() {
    let out = lines(
        "\
xs = [1, 2, 3, 4]
p [xs.reduce(:+), xs.inject(:*), xs.reduce(10, :+), xs.inject(&:+), [\"a\", \"b\"].reduce(:+), [].reduce(:+)]
p [(1..4).reduce(:*), xs.reduce(1) { |a, x| a * x }, [[1], [2]].reduce(:+), [3, 1, 2].sort(&:<=>)]
",
    );
    assert_eq!(out, ["[10, 24, 20, 10, \"ab\", nil]", "[24, 24, [1, 2], [1, 2, 3]]"]);
    assert_eq!(error_of("p [1].reduce"), "ArgumentError: `reduce` expects a block or an operator (`reduce(:+)`)");
}

#[test]
fn hash_methods() {
    let out = lines(
        "\
h = {a: 1, b: 2}
p [h.transform_values { |v| v * 10 }, h.transform_keys { |k| k.to_s }, h.transform_keys { |k| :same }]
p [h.filter_map { |k, v| k if v > 1 }, h.count { |k, v| v > 1 }, h.none? { |k, v| v > 5 }, h.to_h]
p h.each_with_object({}) { |pair, acc| acc[pair[1]] = pair[0] }
p [h.merge({a: 5, c: 3}) { |k, old, new| old + new }, h.merge({a: 5})]
p [h.fetch(:z) { |k| k.to_s }, h.fetch(:a) { |k| 0 }, h.fetch(:z, 9)]
p [{a: {b: [1, 2]}}.dig(:a, :b, -1), h.dig(:z), h.flat_map { |k, v| [v, v] }]
seen = []
h.each_pair { |k, v| seen << k }
p seen
",
    );
    assert_eq!(
        out,
        [
            "[{a: 10, b: 20}, {\"a\" => 1, \"b\" => 2}, {same: 2}]",
            "[[:b], 1, true, {a: 1, b: 2}]",
            "{1 => :a, 2 => :b}",
            "[{a: 6, b: 2, c: 3}, {a: 5, b: 2}]",
            "[\"z\", 1, 9]",
            "[2, nil, [1, 1, 2, 2]]",
            "[:a, :b]",
        ]
    );
}

#[test]
fn then_passes_the_receiver_to_its_block() {
    assert_eq!(
        lines("p 5.then { |x| x + 1 }\np \"a\".yield_self { |s| s.upcase }\np [1].then(&:size)\n"),
        ["6", "\"A\"", "1"]
    );
    assert_eq!(error_of("p 5.then"), "ArgumentError: `then` expects a block");
}

#[test]
fn format_writes_numbers_and_text() {
    let out = lines(
        "\
p format(\"%.2f\", 3.14159), format(\"%05d|%-4s|%x|%+d\", 42, \"ab\", 255, 3), format(\"%s and %p\", :a, \"b\")
p \"%.1f%%\" % 12.345, \"%s-%s\" % [\"a\", 1], \"%08.3f\" % 3.14159, \"%e\" % 1234.5, \"%d\" % 3.99
",
    );
    assert_eq!(
        out,
        [
            "\"3.14\"",
            "\"00042|ab  |ff|+3\"",
            "\"a and \\\"b\\\"\"",
            "\"12.3%\"",
            "\"a-1\"",
            "\"0003.142\"",
            "\"1.234500e+03\"",
            "\"3\"",
        ]
    );
    assert_eq!(error_of("p format(\"%d %d\", 1)"), "ArgumentError: too few arguments for the format");
    assert_eq!(error_of("p format(\"%d\", \"x\")"), "ArgumentError: `%d` expects an integer, got \"x\"");
    assert_eq!(error_of("p format(1)"), "TypeError: `format` expects a format string, got Int");
}

#[test]
fn what_comes_from_untrusted_text_is_untrusted() {
    let src = format!(
        "{UNTRUSTED}s = say(\"x\")
p [s[0, 2].tainted?, s[1..].tainted?, s.slice(0).tainted?, s.delete_prefix(\"a\").tainted?, s.tr(\"a\", \"b\").tainted?]
p [\"a\".sub(\"a\", s).tainted?, \"ab\".ljust(5, s).tainted?, \"ab\".center(9, s).tainted?, \"a\".tr(\"a\", s).tainted?]
p [format(\"%s\", s).tainted?, (\"%s!\" % s).tainted?, (\"%s!\" % [s]).tainted?, format(\"%d\", 1).tainted?]
xs = [[s], [\"b\"]]
p [xs.flatten.map(&:tainted?), [1].zip([s]).flatten.map(&:tainted?), [s].each_slice(1).to_a.flatten.map(&:tainted?)]
p [s.then {{ |v| v.size }}.tainted?, {{a: s}}.transform_values {{ |v| v.size }}.values.map(&:tainted?)]
p [{{a: 1}}.merge({{b: s}}).values.map(&:tainted?), [[s, 1]].to_h.keys.map(&:tainted?), s.chars.reduce(:+).tainted?]
p [s[0, 2].trust!, \"ab\".tainted?]
"
    );
    let out = run_with(&src, vec![Response::text_reply("abc")], &[]).ok();
    assert_eq!(
        out,
        "\
[true, true, true, true, true]
[true, true, true, true]
[true, true, true, false]
[[true, false], [false, true], [true]]
[true, [true]]
[[false, true], [true], true]
[\"ab\", false]
"
    );
}

#[test]
fn a_secret_is_never_sliced_and_formats_into_a_secret() {
    let src = format!("{CREDENTIALS}t = Credentials.fetch(:api, :token)\n");
    for (expr, error) in [
        ("t[0, 4]", "SecretError: a secret cannot be indexed nor sliced"),
        ("t[0..3]", "SecretError: a secret cannot be indexed nor sliced"),
        ("t.slice(0, 4)", "SecretError: a secret has no method `slice`"),
        ("t.delete_prefix(\"sk-\")", "SecretError: a secret has no method `delete_prefix`"),
        ("t.then { |x| x }", "SecretError: a secret has no method `then`"),
    ] {
        let message = error_of(&format!("{src}p {expr}\n"));
        assert!(message.starts_with(error), "{expr}: {message}");
        assert!(!message.contains("sk-1234"), "{expr}: {message}");
    }
    let out = run(&format!(
        "{src}h = format(\"Bearer %s\", t)\np h, h == \"Bearer sk-12345678\", \"%s\" % t, \"%s\" % [t]\n"
    ));
    assert_eq!(out, "[secret]\ntrue\n[secret]\n[secret]\n");
}
