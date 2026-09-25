//! Expanding macros: where they are invoked, what they produce, what goes wrong.

use grenat_ast::{Diagnostic, Item, Member, Program};

fn expand(src: &str) -> (Program, Vec<Diagnostic>) {
    let mut parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let diagnostics = grenat_macros::expand(&mut parsed.program, src);
    (parsed.program, diagnostics)
}

/// The names of the functions and types, in order (members as `Type.name`).
fn names(program: &Program) -> Vec<String> {
    let mut out = Vec::new();
    for item in &program.items {
        match item {
            Item::Fn(def) => out.push(def.name.name.clone()),
            Item::Type(def) => {
                out.push(def.name.name.clone());
                for member in &def.members {
                    match member {
                        Member::Method(m) => out.push(format!("{}.{}", def.name.name, m.name.name)),
                        Member::Field(f) => out.push(format!("{}.{}", def.name.name, f.name.name)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The single diagnostic, and the text it points at.
fn error(src: &str) -> (String, &str) {
    let (_, diagnostics) = expand(src);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    let d = &diagnostics[0];
    (d.message.clone(), &src[d.span.start as usize..d.span.end as usize])
}

#[test]
fn top_level_and_member_invocations_are_replaced_in_place() {
    let src = "\
macro pair(a, b)
  def {{a}} = 1
  def {{b}} = 2
end
macro reader(name)
  def {{name}} = @{{name}}
end
def first = 0
pair :x, :y
class C
  @v: Int = 1
  reader :v
  def after = 3
end
";
    let (program, diagnostics) = expand(src);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert_eq!(names(&program), ["first", "x", "y", "C", "C.v", "C.v", "C.after"]);
    // the expanded code is where it was invoked
    let Item::Fn(x) = &program.items[3] else { panic!() };
    assert_eq!(&src[x.span.start as usize..x.span.end as usize], "pair :x, :y");
    assert!(grenat_types::check(&program).is_empty());
}

#[test]
fn arguments_symbols_text_named_and_variadic() {
    let src = "\
macro make(name, value, *tags)
  def {{name}} = {{value}}
  def {{name}}_tags = [{{tags}}]
  {% for t in tags %}
  def has_{{t}} = true
  {% end %}
end
make :limit, 2 * 21, :a, :b
make value: \"x\", name: :label
";
    let (program, diagnostics) = expand(src);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert_eq!(names(&program), ["limit", "limit_tags", "has_a", "has_b", "label", "label_tags"]);
}

#[test]
fn macros_can_use_macros_even_in_the_types_they_generate() {
    let src = "\
macro reader(name)
  def {{name}} = @{{name}}
end
macro record(type, *fields)
  class {{type}}
    {% for f in fields %}
    @{{f}}: Int = 0
    reader :{{f}}
    {% end %}
  end
end
macro records(*types)
  {% for t in types %}
  record {{t}}, :id
  {% end %}
end
records User, Order
";
    let (program, diagnostics) = expand(src);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert_eq!(names(&program), ["User", "User.id", "User.id", "Order", "Order.id", "Order.id"]);
}

#[test]
fn errors_are_reported_at_the_invocation() {
    let head = "macro m(a, b)\n  def {{a}} = {{b}}\nend\n";
    let (message, at) = error(format!("{head}m :x\n").leak());
    assert_eq!((message.as_str(), at), ("macro `m`: missing argument `b`", "m :x"));
    let (message, _) = error(format!("{head}m :x, 1, 2\n").leak());
    assert_eq!(message, "macro `m` takes 2 argument(s), got 3");
    let (message, _) = error(format!("{head}m :x, c: 1\n").leak());
    assert_eq!(message, "macro `m` has no parameter `c`");
    // expanded code that does not parse
    let (message, at) = error("macro bad(a)\n  def {{a}} =\nend\nbad :x\n");
    assert!(message.starts_with("in the expansion of `bad`: expected an expression"), "{message}");
    assert_eq!(at, "bad :x");
    // template errors
    let (message, _) = error("macro t(a)\n  def {{b}} = 1\nend\nt :x\n");
    assert_eq!(message, "in macro `t`: unknown name `b` in `{{b}}`");
    // not a macro, in a struct
    let (message, at) = error("struct S\n  x: Int\n  nope :x\nend\n");
    assert_eq!((message.as_str(), at), ("unknown macro `nope`", "nope"));
    // defined twice
    let (message, _) = error("macro d\nend\nmacro d\nend\n");
    assert_eq!(message, "macro `d` is defined twice");
}

#[test]
fn a_recursive_macro_stops() {
    let (message, at) = error("macro again(n)\n  again {{n}}\nend\nagain 1\n");
    assert!(message.contains("expansions nested too deep (is it recursive?)"), "{message}");
    assert_eq!(at, "again 1");
}

#[test]
fn checker_errors_in_expanded_code_point_at_the_invocation() {
    let src = "macro broken(name)\n  def {{name}} -> Int = \"text\"\nend\nbroken :f\n";
    let (program, diagnostics) = expand(src);
    assert!(diagnostics.is_empty());
    let errors = grenat_types::check(&program);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(&src[errors[0].span.start as usize..errors[0].span.end as usize], "broken :f");
}

#[test]
fn agents_keep_their_directives() {
    let src = "\
macro fast_model
  model :fast
end
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
agent A
  fast_model
  max_turns 3
  on Ping -> Int
    1
  end
end
";
    let (program, diagnostics) = expand(src);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let Item::Type(agent) = &program.items[2] else { panic!() };
    let directives: Vec<&str> = agent
        .members
        .iter()
        .filter_map(|m| match m {
            Member::Directive(d) => Some(d.name.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(directives, ["model", "max_turns"]);
}
