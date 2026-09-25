//! Grenat's syntax tree, as produced by the parser.
//!
//! The AST stays close to the source: parenthesis-free calls, blocks and
//! punned named arguments (`topic:`) are kept as written.
//! Desugaring happens later, in the HIR.

pub use grenat_lexer::Span;

/// An error reported by a compiler stage (parser, checker).
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    /// Stable code, e.g. `E0412` for a tainted value.
    pub code: Option<&'static str>,
    /// Secondary locations ("`def` opened here").
    pub notes: Vec<(Span, String)>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Diagnostic { span, message: message.into(), code: None, notes: Vec::new(), help: None }
    }

    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_note(mut self, span: Span, message: impl Into<String>) -> Self {
        self.notes.push((span, message.into()));
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Fn(Box<FnDef>),
    Type(TypeDef),
    Model(ModelDecl),
    /// `macro name(params) … end`: a template of code, expanded where it is
    /// invoked (see `grenat_macros`).
    Macro(MacroDef),
    /// Top-level script code (`test "…" do`, assignments…).
    Stmt(Expr),
}

// ── Functions ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    Def,
    /// Implemented by an LLM: `prompt summarize(…) -> ~Summary using :fast`.
    Prompt,
    /// Callable by an LLM; a trust boundary.
    Tool,
    /// Durable: every `step` is journaled.
    Workflow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDef {
    pub kind: FnKind,
    /// `##` comments preceding the definition.
    pub doc: Option<String>,
    pub is_abstract: bool,
    /// `def self.name`: class method.
    pub on_self: bool,
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    /// `uses llm, net("host"), fs.read("./docs")`
    pub effects: Vec<Effect>,
    /// `using :fast` (prompts)
    pub model: Option<Expr>,
    pub body: Body,
    /// Short form `def name = expr`.
    pub short: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Ident,
    pub ty: Option<Type>,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    /// `fs.read` → `["fs", "read"]`
    pub path: Vec<Ident>,
    /// Restriction : `net("smtp.mail.com")`, `fs.read("./docs")`.
    pub args: Vec<Expr>,
    pub span: Span,
}

// ── Named types: struct, class, module, enum, agent, supervisor ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Struct,
    Class,
    Module,
    Enum,
    Agent,
    Supervisor,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    pub kind: TypeKind,
    pub doc: Option<String>,
    pub name: Ident,
    /// `supervisor Desk, strategy: :one_for_one, …`
    pub options: Vec<Arg>,
    pub members: Vec<Member>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    Field(Field),
    Method(FnDef),
    Include(Type),
    /// Enum variant: `Circle(radius: Float)`.
    Variant(Variant),
    /// Agent : `on Research(topic: String) -> ~Report … end`.
    Handler(Handler),
    /// Declarative configuration: `model :smart`, `tools a, b`, `child Writer, count: 4`.
    Directive(Directive),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub doc: Option<String>,
    pub name: Ident,
    /// `@count` (class/agent state) rather than `count:` (struct field).
    pub is_ivar: bool,
    pub ty: Option<Type>,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub doc: Option<String>,
    pub name: Ident,
    pub fields: Vec<Field>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Handler {
    pub doc: Option<String>,
    pub message: Ident,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub body: Body,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Directive {
    pub name: Ident,
    pub args: Vec<Arg>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelDecl {
    pub name: Ident,
    pub options: Vec<Arg>,
    pub span: Span,
}

// ── Macros ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct MacroDef {
    pub doc: Option<String>,
    pub name: Ident,
    pub params: Vec<MacroParam>,
    /// The template, as written (between the header line and `end`).
    pub body: String,
    pub body_span: Span,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroParam {
    pub name: Ident,
    /// `*names`: the remaining arguments.
    pub variadic: bool,
}

// ── Types ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// `String`, `Array(Note)`, `Result(Config, IoError)`, `Http::Client`
    Named { path: Vec<Ident>, args: Vec<Type>, span: Span },
    /// `User?`
    Optional(Box<Type>, Span),
    /// `~Summary`: produced by an LLM, not validated.
    Tainted(Box<Type>, Span),
}

impl Type {
    pub fn span(&self) -> Span {
        match self {
            Type::Named { span, .. } | Type::Optional(_, span) | Type::Tainted(_, span) => *span,
        }
    }
}

// ── Bodies, blocks, arguments ────────────────────────────────

/// Statement list with optional `rescue`/`ensure` clauses
/// (body of a `def`, of a `do … end` block, of `begin`).
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    pub stmts: Vec<Expr>,
    pub rescues: Vec<Rescue>,
    pub ensure: Option<Vec<Expr>>,
    pub span: Span,
}

impl Body {
    pub fn new(stmts: Vec<Expr>, span: Span) -> Self {
        Body { stmts, rescues: Vec::new(), ensure: None, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rescue {
    pub types: Vec<Type>,
    pub binding: Option<Ident>,
    pub body: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub params: Vec<Param>,
    pub body: Body,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Pos(Expr),
    /// `topic: t`, or bare `topic:` (value = the variable of the same name).
    Named {
        name: Ident,
        value: Option<Expr>,
    },
    /// `&blk`, `&:upcase`
    BlockPass(Expr),
}

// ── Expressions ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Expr { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrSeg {
    Lit(String),
    Interp(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Cmp,
    Match,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(Vec<StrSeg>),
    Symbol(String),
    Bool(bool),
    Nil,
    SelfRef,
    Array(Vec<Expr>),
    Hash(Vec<(Expr, Expr)>),
    Range {
        lo: Box<Expr>,
        hi: Box<Expr>,
        inclusive: bool,
    },

    /// Bare identifier: local variable or argument-less call (resolved later).
    Var(String),
    Const(Vec<Ident>),
    IVar(String),
    /// Implicit parameter of a short block `&.sent?`.
    It,

    Call {
        recv: Option<Box<Expr>>,
        name: Ident,
        args: Vec<Arg>,
        block: Option<Box<Block>>,
        /// `&.`
        safe: bool,
        /// Call written with parentheses.
        parens: bool,
    },
    Index {
        recv: Box<Expr>,
        args: Vec<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `expr?`: propagates the error.
    Try(Box<Expr>),

    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `x += 1`, `x ||= y`
    OpAssign {
        op: BinOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `a, b = pair`
    MultiAssign {
        targets: Vec<Expr>,
        value: Box<Expr>,
    },

    If {
        cond: Box<Expr>,
        then: Vec<Expr>,
        else_: Option<Vec<Expr>>,
    },
    While {
        cond: Box<Expr>,
        body: Vec<Expr>,
    },
    Case {
        subject: Option<Box<Expr>>,
        arms: Vec<CaseArm>,
        else_: Option<Vec<Expr>>,
    },
    Begin(Body),
    Return(Option<Box<Expr>>),
    Break(Option<Box<Expr>>),
    Next(Option<Box<Expr>>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArmTest {
    /// `in Pattern`
    In(Pattern),
    /// `when a, b`
    When(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseArm {
    pub test: ArmTest,
    pub guard: Option<Expr>,
    pub body: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    Wildcard,
    Bind(String),
    Lit(Expr),
    /// `Circle(r)`, `Triage(category: Spam)`, `Sentiment::Positive`
    Const {
        path: Vec<Ident>,
        fields: Option<Vec<PatField>>,
    },
    Array(Vec<Pattern>),
    Or(Vec<Pattern>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatField {
    /// `None`: positional field.
    pub name: Option<Ident>,
    pub pattern: Pattern,
}
