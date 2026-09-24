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
