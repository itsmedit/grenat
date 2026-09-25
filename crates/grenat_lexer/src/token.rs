//! Tokens produced by the lexer: positions, keywords, token kinds, comments, errors.

use std::fmt;
use std::ops::Range;

/// Position in the source, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start: start as u32, end: end as u32 }
    }

    /// Smallest span covering `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span { start: self.start.min(other.start), end: self.end.max(other.end) }
    }

    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

macro_rules! keywords {
    ($($variant:ident => $text:literal,)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Keyword { $($variant,)* }

        impl Keyword {
            pub fn lookup(s: &str) -> Option<Keyword> {
                match s { $($text => Some(Keyword::$variant),)* _ => None }
            }

            pub fn as_str(self) -> &'static str {
                match self { $(Keyword::$variant => $text,)* }
            }
        }
    };
}

keywords! {
    Abstract => "abstract",
    Agent => "agent",
    And => "and",
    Begin => "begin",
    Break => "break",
    Case => "case",
    Class => "class",
    Def => "def",
    Do => "do",
    Else => "else",
    Elsif => "elsif",
    End => "end",
    Ensure => "ensure",
    Enum => "enum",
    False => "false",
    If => "if",
    In => "in",
    Module => "module",
    Next => "next",
    Nil => "nil",
    Not => "not",
    Or => "or",
    Prompt => "prompt",
    Rescue => "rescue",
    Return => "return",
    SelfKw => "self",
    Struct => "struct",
    Supervisor => "supervisor",
    Then => "then",
    Tool => "tool",
    True => "true",
    Unless => "unless",
    Until => "until",
    When => "when",
    While => "while",
    Workflow => "workflow",
}

/// A piece of a string: literal text, or a `#{…}` interpolation already split into tokens.
#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    Interp(Vec<Token>, Span),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Int(i64),
    Float(f64),
    Str(Vec<StrPart>),
    Symbol(String),
    Label(String),
    Ident(String),
    Const(String),
    IVar(String),
    Kw(Keyword),

    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    SafeDot,
    DotDot,
    DotDotDot,
    Colon,
    ColonColon,
    Question,
    Pipe,
    Tilde,
    Arrow,
    FatArrow,

    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Cmp,
    Match,
    AndAnd,
    OrOr,
    Bang,
    Amp,
    Caret,
    Shl,
    Shr,

    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    OrOrEq,
    AndAndEq,

    /// The body of a `macro`, as written: raw text up to its `end`.
    MacroBody(String),

    Newline,
    Eof,
}

impl TokenKind {
    /// Human-readable description for error messages.
    pub fn describe(&self) -> String {
        use TokenKind::*;
        match self {
            Int(n) => format!("number `{n}`"),
            Float(n) => format!("number `{n}`"),
            Str(_) => "string".into(),
            Symbol(s) => format!("symbol `:{s}`"),
            Label(s) => format!("`{s}:`"),
            Ident(s) => format!("identifier `{s}`"),
            Const(s) => format!("constant `{s}`"),
            IVar(s) => format!("`@{s}`"),
            Kw(k) => format!("keyword `{}`", k.as_str()),
            MacroBody(_) => "macro body".into(),
            Newline => "end of line".into(),
            Eof => "end of file".into(),
            other => format!("`{}`", other.punct()),
        }
    }

    fn punct(&self) -> &'static str {
        use TokenKind::*;
        match self {
            LParen => "(",
            RParen => ")",
            LBracket => "[",
            RBracket => "]",
            LBrace => "{",
            RBrace => "}",
            Comma => ",",
            Dot => ".",
            SafeDot => "&.",
            DotDot => "..",
            DotDotDot => "...",
            Colon => ":",
            ColonColon => "::",
            Question => "?",
            Pipe => "|",
            Tilde => "~",
            Arrow => "->",
            FatArrow => "=>",
            Plus => "+",
            Minus => "-",
            Star => "*",
            StarStar => "**",
            Slash => "/",
            Percent => "%",
            EqEq => "==",
            NotEq => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            Cmp => "<=>",
            Match => "=~",
            AndAnd => "&&",
            OrOr => "||",
            Bang => "!",
            Amp => "&",
            Caret => "^",
            Shl => "<<",
            Shr => ">>",
            Eq => "=",
            PlusEq => "+=",
            MinusEq => "-=",
            StarEq => "*=",
            SlashEq => "/=",
            OrOrEq => "||=",
            AndAndEq => "&&=",
            _ => "?",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// Preceded by whitespace (or first on its line): tells `foo [1]` from `foo[1]`,
    /// `foo (x)` from `foo(x)`, `x ?` from `x?`.
    pub space_before: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub span: Span,
    pub text: String,
    /// `## …`: a documentation comment.
    pub doc: bool,
    /// At the end of a line, after code.
    pub trailing: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub comments: Vec<Comment>,
    pub errors: Vec<LexError>,
}
