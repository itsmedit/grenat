//! AST shape for constructs where the grammar is subtle.

use grenat_ast::*;
use grenat_parser::parse;

fn program(src: &str) -> Program {
    let parsed = parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    parsed.program
}

fn stmt(src: &str) -> ExprKind {
    match program(src).items.remove(0) {
        Item::Stmt(e) => e.kind,
        other => panic!("expected a statement: {other:?}"),
    }
}

fn call_parts(kind: &ExprKind) -> (&str, &[Arg], Option<&Block>) {
    match kind {
        ExprKind::Call { name, args, block, .. } => (&name.name, args, block.as_deref()),
        other => panic!("expected a call: {other:?}"),
    }
}

#[test]
fn do_block_binds_to_the_outer_command() {
    // `do` belongs to `within`, not to `budget(…)`
    let kind = stmt("within budget(usd: 1.00) do\n  work\nend\n");
    let (name, args, block) = call_parts(&kind);
    assert_eq!(name, "within");
    assert!(block.is_some());
    let Arg::Pos(inner) = &args[0] else { panic!() };
    let (inner_name, _, inner_block) = call_parts(&inner.kind);
    assert_eq!(inner_name, "budget");
    assert!(inner_block.is_none());
}

#[test]
fn brace_after_constant_argument_goes_to_the_command() {
    let kind = stmt("assert_raises ApprovalDenied { boom }\n");
    let (name, args, block) = call_parts(&kind);
    assert_eq!(name, "assert_raises");
    assert!(matches!(&args[0], Arg::Pos(Expr { kind: ExprKind::Const(_), .. })));
    assert!(block.is_some());
}

#[test]
fn command_call_vs_binary_operator() {
    assert!(matches!(stmt("x - 1"), ExprKind::Binary { op: BinOp::Sub, .. }));
    let kind = stmt("spawn Researcher");
    assert_eq!(call_parts(&kind).0, "spawn");
    assert!(matches!(stmt("foo[1]"), ExprKind::Index { .. }));
    let kind = stmt("foo [1]");
    assert!(matches!(call_parts(&kind).1[0], Arg::Pos(Expr { kind: ExprKind::Array(_), .. })));
}

#[test]
fn punned_named_args_and_try() {
    let ExprKind::Try(inner) = stmt("Research(topic:, limit: 3)?") else { panic!() };
    let (name, args, _) = call_parts(&inner.kind);
    assert_eq!(name, "Research");
    assert!(matches!(&args[0], Arg::Named { name, value: None } if name.name == "topic"));
    assert!(matches!(&args[1], Arg::Named { value: Some(_), .. }));
}

#[test]
fn short_block_with_implicit_it() {
    let kind = stmt("outcomes.partition(&.sent?)");
    let (_, args, block) = call_parts(&kind);
    assert!(args.is_empty());
    let block = block.expect("short block");
    assert_eq!(block.params[0].name.name, "it");
    let (name, ..) = call_parts(&block.body.stmts[0].kind);
    assert_eq!(name, "sent?");
}

#[test]
fn multi_assign_and_op_assign() {
    assert!(matches!(stmt("a, @b = pair"), ExprKind::MultiAssign { targets, .. } if targets.len() == 2));
    assert!(matches!(stmt("@count += 1"), ExprKind::OpAssign { op: BinOp::Add, .. }));
    assert!(matches!(stmt("x ||= 2"), ExprKind::OpAssign { op: BinOp::Or, .. }));
}

#[test]
fn modifiers_wrap_the_statement() {
    let ExprKind::If { then, .. } = stmt("return n if n < 2") else { panic!() };
    assert!(matches!(then[0].kind, ExprKind::Return(Some(_))));
}

#[test]
fn precedence() {
    // 1 + (2 * 3)
    let ExprKind::Binary { op: BinOp::Add, rhs, .. } = stmt("1 + 2 * 3") else { panic!() };
    assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::Mul, .. }));
    // -(2 ** 2)
    let ExprKind::Unary { op: UnOp::Neg, expr } = stmt("-2 ** 2") else { panic!() };
    assert!(matches!(expr.kind, ExprKind::Binary { op: BinOp::Pow, .. }));
    // (a = b) and c
    assert!(matches!(stmt("a = b and c"), ExprKind::Binary { op: BinOp::And, .. }));
}

#[test]
fn case_in_patterns() {
    let src = "case t\nin Triage(category: Spam) then 1\nin Circle(r) | Rect(_, _) if r > 0\n  2\nelse\nend\n";
    let ExprKind::Case { arms, else_, .. } = stmt(src) else { panic!() };
    assert_eq!(arms.len(), 2);
    assert!(else_.is_some());
    let ArmTest::In(Pattern { kind: PatternKind::Const { fields: Some(fields), .. }, .. }) = &arms[0].test else {
        panic!()
    };
    assert_eq!(fields[0].name.as_ref().unwrap().name, "category");
    assert!(matches!(&arms[1].test, ArmTest::In(Pattern { kind: PatternKind::Or(alts), .. }) if alts.len() == 2));
    assert!(arms[1].guard.is_some());
}

#[test]
fn prompt_with_effects_model_and_docs() {
    let src = "## Summarizes.\n## Two lines.\nprompt summarize(a: String) -> ~Summary? uses llm, net(\"x\") using :fast\n  user a\nend\n";
    let Item::Fn(f) = program(src).items.remove(0) else { panic!() };
    assert_eq!(f.kind, FnKind::Prompt);
    assert_eq!(f.doc.as_deref(), Some("Summarizes.\nTwo lines."));
    assert!(matches!(f.ret, Some(Type::Tainted(ref inner, _)) if matches!(**inner, Type::Optional(..))));
    assert_eq!(f.effects.len(), 2);
    assert_eq!(f.effects[1].args.len(), 1);
    assert!(matches!(f.model, Some(Expr { kind: ExprKind::Symbol(ref s), .. }) if s == "fast"));
}

#[test]
fn agent_members() {
    let src = "\
agent Writer
  model :smart
  tools a, b
  @notes: Array(Note) = []
  on Draft(t: Ticket) -> ~Answer
    run t
  end
  def helper = 1
end
";
    let Item::Type(agent) = program(src).items.remove(0) else { panic!() };
    assert_eq!(agent.kind, TypeKind::Agent);
    let kinds: Vec<&str> = agent
        .members
        .iter()
        .map(|m| match m {
            Member::Directive(_) => "directive",
            Member::Field(_) => "field",
            Member::Handler(_) => "handler",
            Member::Method(_) => "method",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["directive", "directive", "field", "handler", "method"]);
    let Member::Directive(tools) = &agent.members[1] else { panic!() };
    assert_eq!(tools.args.len(), 2);
}

#[test]
fn struct_fields_get_trailing_docs() {
    let src = "struct S\n  ## Above\n  a: String ## On the right\n  b: Int = 3\nend\n";
    let Item::Type(s) = program(src).items.remove(0) else { panic!() };
    let Member::Field(a) = &s.members[0] else { panic!() };
    assert_eq!(a.doc.as_deref(), Some("Above\nOn the right"));
    let Member::Field(b) = &s.members[1] else { panic!() };
    assert!(b.doc.is_none() && b.default.is_some());
}

#[test]
fn string_interpolation_is_parsed() {
    let ExprKind::Str(segs) = stmt("\"a #{x.y(1)} b\"") else { panic!() };
    assert!(matches!(&segs[1], StrSeg::Interp(Expr { kind: ExprKind::Call { .. }, .. })));
}

#[test]
fn errors_are_reported_with_recovery() {
    let parsed = parse("def f(\n  x = )\nend\ny = 1 +\nz = [1, 2\n");
    assert!(parsed.diagnostics.len() >= 2, "{:#?}", parsed.diagnostics);

    let parsed = parse("def f\n  1\n");
    let d = &parsed.diagnostics[0];
    assert!(d.message.contains("expected `end` to close `def`"), "{}", d.message);
    assert_eq!(d.notes.len(), 1);
}

#[test]
fn negative_literals_bind_before_method_calls() {
    // `-3.abs` is `(-3).abs`, as in Ruby
    let ExprKind::Call { recv: Some(recv), .. } = stmt("-3.abs") else { panic!() };
    assert_eq!(recv.kind, ExprKind::Int(-3));
    assert!(matches!(stmt("-1.5"), ExprKind::Float(f) if f == -1.5));
    // … but a detached minus, or one before `**`, stays an operator
    assert!(matches!(stmt("- 3.abs"), ExprKind::Unary { op: UnOp::Neg, .. }));
    assert!(matches!(stmt("-2 ** 2"), ExprKind::Unary { op: UnOp::Neg, .. }));
    assert!(matches!(stmt("x -1"), ExprKind::Binary { op: BinOp::Sub, .. }));
}

#[test]
fn native_functions_have_no_body() {
    let src = "## Reads a sheet.\nnative def read_sheet(path: String) -> ~Array(Array(String)) uses fs.read\nnative def add(a: Int, b: Int) -> Int pure\nnative def tick\nnative = 1\n";
    let mut items = program(src).items;
    let Item::Fn(read) = items.remove(0) else { panic!() };
    assert_eq!(read.kind, FnKind::Native);
    assert_eq!(read.doc.as_deref(), Some("Reads a sheet."));
    assert_eq!((read.params.len(), read.effects.len(), read.pure), (1, 1, false));
    assert!(read.body.stmts.is_empty() && !read.is_abstract);
    let Item::Fn(add) = items.remove(0) else { panic!() };
    assert!(add.pure && add.effects.is_empty());
    assert_eq!(&src[add.span.range()], "native def add(a: Int, b: Int) -> Int pure");
    let Item::Fn(tick) = items.remove(0) else { panic!() };
    assert_eq!((tick.kind, tick.params.len()), (FnKind::Native, 0));
    // `native` is still a name
    assert!(matches!(&items[0], Item::Stmt(Expr { kind: ExprKind::Assign { .. }, .. })));
    // `pure` belongs to native functions
    assert!(matches!(program("def f\n  pure\nend\n").items[0], Item::Fn(ref f) if !f.pure));
}

#[test]
fn a_native_function_is_declared_at_the_top_level_without_a_body() {
    let error = |src: &str| parse(src).diagnostics.first().map(|d| d.message.clone()).unwrap_or_default();
    let no_body = "a `native def` has no body: its facet's native code or bridge implements it";
    assert_eq!(error("native def f = 1\n"), no_body);
    // a body written as a function's: said so, once, where it is
    for (src, body) in [
        ("native def f(x: Int) -> Int pure\n  x\nend\nputs 1\n", "x\nend"),
        ("native def f(x: Int) -> Int pure\n  y = x\n  y * 2\nend\n", "y = x\n  y * 2\nend"),
        ("native def f\nend\n", "end"),
    ] {
        let parsed = parse(src);
        assert_eq!(parsed.diagnostics.len(), 1, "{src}: {:#?}", parsed.diagnostics);
        assert_eq!(parsed.diagnostics[0].message, no_body, "{src}");
        assert_eq!(&src[parsed.diagnostics[0].span.range()], body, "{src}");
    }
    // statements after a `native def`, a `def` with its own `end`: no body
    assert!(parse("native def f\nputs 1\ndef g\n  2\nend\nx = 3\n").diagnostics.is_empty());
    assert_eq!(error("def g\n  2\nend\nend\n"), "`end` without an opening block");
    assert_eq!(error("native def self.f\n"), "a `native def` is a function, declared at the top level");
    assert_eq!(
        error("module Sheets\n  native def f\nend\n"),
        "a `native def` is a function, declared at the top level (not in a type)"
    );
}

#[test]
fn slices_take_two_values_or_a_range_endless_in_an_index() {
    let ExprKind::Index { args, .. } = stmt("s[0, 4]\n") else { panic!() };
    assert_eq!(args.len(), 2);
    for src in ["xs[2..]\n", "xs[2...]\n"] {
        let ExprKind::Index { args, .. } = stmt(src) else { panic!("{src}") };
        let ExprKind::Range { lo, hi, inclusive } = &args[0].kind else { panic!("{src}: {:?}", args[0].kind) };
        // to the end, as `2..-1`
        assert!(matches!((&lo.kind, &hi.kind, inclusive), (ExprKind::Int(2), ExprKind::Int(-1), true)), "{src}");
        assert_eq!(hi.span.start, hi.span.end, "{src}: the missing end is written nowhere");
    }
    // outside an index, a range has an end
    assert!(!parse("r = (2..)\n").diagnostics.is_empty());
    assert!(!parse("xs = [2..]\n").diagnostics.is_empty());
}

#[test]
fn operator_symbols_name_methods() {
    let kind = stmt("xs.reduce(:+)\n");
    let (_, args, _) = call_parts(&kind);
    assert!(matches!(&args[0], Arg::Pos(Expr { kind: ExprKind::Symbol(s), .. }) if s == "+"));
    let kind = stmt("xs.inject(&:*)\n");
    let (_, args, _) = call_parts(&kind);
    assert!(matches!(&args[0], Arg::BlockPass(Expr { kind: ExprKind::Symbol(s), .. }) if s == "*"));
}
