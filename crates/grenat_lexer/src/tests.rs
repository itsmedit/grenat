use super::*;
use TokenKind::*;

fn kinds(src: &str) -> Vec<TokenKind> {
    let lexed = lex(src);
    assert!(lexed.errors.is_empty(), "erreurs inattendues : {:?}", lexed.errors);
    lexed.tokens.into_iter().map(|t| t.kind).collect()
}

fn ident(s: &str) -> TokenKind {
    Ident(s.into())
}

fn lit(s: &str) -> StrPart {
    StrPart::Lit(s.into())
}

#[test]
fn labels_symbols_and_scope() {
    assert_eq!(
        kinds("model :fast, name: x, to:, A::B"),
        vec![
            ident("model"),
            Symbol("fast".into()),
            Comma,
            Label("name".into()),
            ident("x"),
            Comma,
            Label("to".into()),
            Comma,
            Const("A".into()),
            ColonColon,
            Const("B".into()),
            Eof
        ]
    );
}

#[test]
fn predicate_and_bang_methods() {
    assert_eq!(
        kinds("a.empty? b.save! c != d"),
        vec![ident("a"), Dot, ident("empty?"), ident("b"), Dot, ident("save!"), ident("c"), NotEq, ident("d"), Eof]
    );
    // constant followed by `?`: optional type
    assert_eq!(kinds("User?"), vec![Const("User".into()), Question, Eof]);
    // `?` after `)`: the try operator
    assert_eq!(kinds("f(x)?"), vec![ident("f"), LParen, ident("x"), RParen, Question, Eof]);
}

#[test]
fn numbers_and_method_calls_on_numbers() {
    assert_eq!(
        kinds("200_000 2.50 3.days 1e3 0xff"),
        vec![Int(200_000), Float(2.5), Int(3), Dot, ident("days"), Float(1000.0), Int(255), Eof]
    );
    assert_eq!(kinds("1..10"), vec![Int(1), DotDot, Int(10), Eof]);
}

#[test]
fn interpolation_nests_strings_and_braces() {
    let toks = kinds(r##""n°##{t.id} #{h.map { |x| "#{x}" }}!""##);
    let [Str(parts), Eof] = toks.as_slice() else { panic!("{toks:?}") };
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0], lit("n°#"));
    assert!(matches!(&parts[1], StrPart::Interp(t, _) if t.len() == 4));
    assert_eq!(parts[2], lit(" "));
    assert!(matches!(&parts[3], StrPart::Interp(t, _) if t.iter().any(|t| matches!(t.kind, Str(_)))));
    assert_eq!(parts[4], lit("!"));
}

#[test]
fn escapes() {
    assert_eq!(kinds(r#""a\n\t\"\u{e9}\#{x}""#), vec![Str(vec![lit("a\n\t\"é#{x}")]), Eof]);
    assert_eq!(kinds(r"'a\nb\'c'"), vec![Str(vec![lit("a\\nb'c")]), Eof]);
}

#[test]
fn squiggly_heredoc_dedents_and_resumes_after_terminator() {
    let src = "run <<~T, 1\n    Hello #{name}\n      indented\n  T\nnext_line\n";
    let toks = kinds(src);
    let Str(parts) = &toks[1] else { panic!("{toks:?}") };
    assert_eq!(parts[0], lit("Hello "));
    assert!(matches!(parts[1], StrPart::Interp(..)));
    assert_eq!(parts[2], lit("\n  indented\n"));
    assert_eq!(&toks[2..], &[Comma, Int(1), Newline, ident("next_line"), Newline, Eof]);
}

#[test]
fn raw_heredoc_does_not_interpolate() {
    let toks = kinds("x = <<~'SQL'\n  #{not_interpolated}\nSQL\n");
    assert_eq!(toks[2], Str(vec![lit("#{not_interpolated}\n")]));
}

#[test]
fn leading_dot_continues_previous_line() {
    assert_eq!(
        kinds("a\n  .b\n  # comment\n  &.c\nd"),
        vec![ident("a"), Dot, ident("b"), SafeDot, ident("c"), Newline, ident("d"), Eof]
    );
}

#[test]
fn comments_are_split_into_docs_and_trailing() {
    let lexed = lex("## The tool's doc\ntool x\n  title: String ## Title\n# simple\n");
    let docs: Vec<_> = lexed.comments.iter().map(|c| (c.text.as_str(), c.doc, c.trailing)).collect();
    assert_eq!(docs, vec![("The tool's doc", true, false), ("Title", true, true), ("simple", false, false)]);
}

#[test]
fn space_before_is_tracked() {
    let toks = lex("foo [1]\nfoo[1]").tokens;
    assert!(toks[1].space_before);
    assert!(!toks[6].space_before);
}

#[test]
fn operators() {
    assert_eq!(
        kinds("a ||= b && c <=> d -> e => f << g **h"),
        vec![
            ident("a"),
            OrOrEq,
            ident("b"),
            AndAnd,
            ident("c"),
            Cmp,
            ident("d"),
            Arrow,
            ident("e"),
            FatArrow,
            ident("f"),
            Shl,
            ident("g"),
            StarStar,
            ident("h"),
            Eof
        ]
    );
}

#[test]
fn reports_unterminated_constructs() {
    assert!(!lex("\"abc").errors.is_empty());
    assert!(!lex("\"#{abc\"").errors.is_empty());
    assert!(!lex("x = <<~EOS\nabc\n").errors.is_empty());
    assert!(!lex("x = $").errors.is_empty());
}

#[test]
fn unterminated_string_recovers_at_end_of_line() {
    let lexed = lex("a \"#{x.y\"\nnext_line\n");
    assert_eq!(lexed.errors.len(), 1, "{:?}", lexed.errors);
    assert!(lexed.tokens.iter().any(|t| t.kind == ident("next_line")));
}

#[test]
fn symbol_after_block_pass() {
    assert_eq!(kinds("map(&:upcase)"), vec![ident("map"), LParen, Amp, Symbol("upcase".into()), RParen, Eof]);
}
