//! Lexer de Grenat.
//!
//! Écrit à la main plutôt qu'avec un générateur : l'interpolation `"#{…}"`
//! et les heredocs `<<~ID` demandent des modes qu'un lexer régulier gère mal.
//!
//! Conventions à la Ruby gérées ici :
//! - `nom:` collé est un **label** (argument nommé, champ), `:nom` est un **symbole** ;
//! - `?` et `!` en fin d'identifiant minuscule font partie du nom (`empty?`, `save!`) ;
//! - une fin de ligne suivie de `.` ou `&.` continue l'expression (chaînage multi-ligne) ;
//! - les commentaires sont retirés du flux de tokens mais conservés à part
//!   (les `##` sont des commentaires de documentation, transmis aux LLM).

use std::fmt;
use std::ops::Range;

#[cfg(test)]
mod tests;

/// Position dans le source, en octets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start: start as u32, end: end as u32 }
    }

    /// Plus petit span couvrant `self` et `other`.
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

/// Morceau d'une chaîne : texte littéral ou interpolation `#{…}` déjà découpée en tokens.
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

    Newline,
    Eof,
}

impl TokenKind {
    /// Description lisible pour les messages d'erreur.
    pub fn describe(&self) -> String {
        use TokenKind::*;
        match self {
            Int(n) => format!("nombre `{n}`"),
            Float(n) => format!("nombre `{n}`"),
            Str(_) => "chaîne".into(),
            Symbol(s) => format!("symbole `:{s}`"),
            Label(s) => format!("`{s}:`"),
            Ident(s) => format!("identifiant `{s}`"),
            Const(s) => format!("constante `{s}`"),
            IVar(s) => format!("`@{s}`"),
            Kw(k) => format!("mot-clé `{}`", k.as_str()),
            Newline => "fin de ligne".into(),
            Eof => "fin du fichier".into(),
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
    /// Précédé d'un blanc (ou premier de sa ligne) : distingue `foo [1]` de `foo[1]`,
    /// `foo (x)` de `foo(x)`, `x ?` de `x?`.
    pub space_before: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub span: Span,
    pub text: String,
    /// `## …` : commentaire de documentation.
    pub doc: bool,
    /// En fin de ligne, après du code.
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

pub fn lex(src: &str) -> Lexed {
    let mut lx = Lexer::new(src, 0, false);
    lx.run();
    lx.push(TokenKind::Eof, src.len(), src.len());
    Lexed { tokens: join_continuations(lx.tokens), comments: lx.comments, errors: lx.errors }
}

/// Supprime les fins de ligne suivies d'un `.` ou `&.` : `answer\n  .check { … }`.
fn join_continuations(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut iter = tokens.into_iter().peekable();
    while let Some(tok) = iter.next() {
        let continues = matches!(iter.peek(), Some(next) if matches!(next.kind, TokenKind::Dot | TokenKind::SafeDot));
        if tok.kind == TokenKind::Newline && continues {
            continue;
        }
        out.push(tok);
    }
    out
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

enum Term {
    Quote(char),
    End(usize),
}

struct Lexer<'s> {
    src: &'s str,
    pos: usize,
    tokens: Vec<Token>,
    comments: Vec<Comment>,
    errors: Vec<LexError>,
    /// Après un heredoc, où reprendre à la prochaine fin de ligne (après son terminateur).
    heredoc_resume: Option<usize>,
    /// Sous-lexer d'une interpolation : s'arrête sur la `}` fermante.
    in_interp: bool,
    brace_depth: u32,
    space_before: bool,
    line_has_token: bool,
}

impl<'s> Lexer<'s> {
    fn new(src: &'s str, pos: usize, in_interp: bool) -> Self {
        Lexer {
            src,
            pos,
            tokens: Vec::new(),
            comments: Vec::new(),
            errors: Vec::new(),
            heredoc_resume: None,
            in_interp,
            brace_depth: 0,
            space_before: true,
            line_has_token: false,
        }
    }

    fn rest(&self) -> &'s str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn line_end(&self, from: usize) -> usize {
        self.src[from..].find('\n').map_or(self.src.len(), |i| from + i)
    }

    fn error(&mut self, start: usize, end: usize, message: impl Into<String>) {
        self.errors.push(LexError { span: Span::new(start, end), message: message.into() });
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token { kind, span: Span::new(start, end), space_before: self.space_before });
        self.line_has_token = true;
    }

    fn run(&mut self) {
        loop {
            let skipped = self.skip_blanks();
            let line_start = matches!(self.tokens.last(), None | Some(Token { kind: TokenKind::Newline, .. }));
            self.space_before = skipped || line_start;

            let start = self.pos;
            let Some(c) = self.peek() else { return };
            match c {
                '\n' => {
                    self.bump();
                    self.newline(start);
                }
                ';' => {
                    self.bump();
                    self.push_newline(start);
                }
                '#' => self.comment(),
                '"' => {
                    self.bump();
                    let parts = self.string_parts(Term::Quote('"'), true, start);
                    self.push(TokenKind::Str(parts), start, self.pos);
                }
                '\'' => {
                    self.bump();
                    let parts = self.string_parts(Term::Quote('\''), false, start);
                    self.push(TokenKind::Str(parts), start, self.pos);
                }
                '@' => self.ivar(),
                ':' => self.colon(),
                '<' if self.at_heredoc() => self.heredoc(),
                '}' if self.in_interp && self.brace_depth == 0 => return,
                c if c.is_ascii_digit() => self.number(),
                c if is_ident_start(c) => self.ident(),
                _ => self.punct(),
            }
        }
    }

    /// Espaces, tabulations et continuation `\` en fin de ligne.
    fn skip_blanks(&mut self) -> bool {
        let before = self.pos;
        loop {
            match self.peek() {
                Some(' ' | '\t' | '\r') => {
                    self.bump();
                }
                Some('\\') if matches!(self.peek_at(1), Some('\n')) => {
                    self.pos += 2;
                }
                _ => break,
            }
        }
        self.pos != before
    }

    fn push_newline(&mut self, start: usize) {
        let redundant = matches!(self.tokens.last(), None | Some(Token { kind: TokenKind::Newline, .. }));
        if !redundant {
            self.push(TokenKind::Newline, start, start + 1);
        }
        self.line_has_token = false;
    }

    fn newline(&mut self, start: usize) {
        self.push_newline(start);
        if let Some(resume) = self.heredoc_resume.take() {
            self.pos = resume;
        }
    }

    fn comment(&mut self) {
        let start = self.pos;
        let end = self.rest().find('\n').map_or(self.src.len(), |i| start + i);
        let raw = &self.src[start..end];
        let doc = raw.starts_with("##") && !raw.starts_with("###");
        self.comments.push(Comment {
            span: Span::new(start, end),
            text: raw.trim_start_matches('#').trim().to_string(),
            doc,
            trailing: self.line_has_token,
        });
        self.pos = end;
    }

    fn take_ident_chars(&mut self) {
        while let Some(c) = self.peek() {
            if !is_ident_continue(c) {
                break;
            }
            self.bump();
        }
    }

    /// `?`/`!` final d'un nom de méthode, sauf `a!=b` ou `a?=…`.
    fn take_predicate_suffix(&mut self) {
        if matches!(self.peek(), Some('?' | '!')) && self.peek_at(1) != Some('=') {
            self.bump();
        }
    }

    fn ident(&mut self) {
        let start = self.pos;
        self.take_ident_chars();
        let is_const = self.src[start..].starts_with(|c: char| c.is_uppercase());
        if !is_const {
            self.take_predicate_suffix();
        }
        let text = &self.src[start..self.pos];

        if !is_const && self.peek() == Some(':') && self.peek_at(1) != Some(':') {
            self.bump();
            self.push(TokenKind::Label(text.to_string()), start, self.pos);
            return;
        }

        let kind = if is_const {
            TokenKind::Const(text.to_string())
        } else if let Some(kw) = Keyword::lookup(text) {
            TokenKind::Kw(kw)
        } else {
            TokenKind::Ident(text.to_string())
        };
        self.push(kind, start, self.pos);
    }

    fn ivar(&mut self) {
        let start = self.pos;
        self.bump();
        let name_start = self.pos;
        self.take_ident_chars();
        if self.pos == name_start {
            self.error(start, self.pos, "nom de variable d'instance attendu après `@`");
            return;
        }
        let name = self.src[name_start..self.pos].to_string();
        self.push(TokenKind::IVar(name), start, self.pos);
    }

    fn colon(&mut self) {
        let start = self.pos;
        self.bump();
        if self.eat(':') {
            self.push(TokenKind::ColonColon, start, self.pos);
            return;
        }

        // `:nom` est un symbole s'il commence un terme : `f :x`, `(:x`, `[:x`, `, :x`
        let after_opener = matches!(
            self.tokens.last().map(|t| &t.kind),
            None | Some(
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace | TokenKind::Comma | TokenKind::Pipe
            )
        );
        if self.peek().is_some_and(is_ident_start) && (self.space_before || after_opener) {
            let name_start = self.pos;
            self.take_ident_chars();
            self.take_predicate_suffix();
            let name = self.src[name_start..self.pos].to_string();
            self.push(TokenKind::Symbol(name), start, self.pos);
        } else {
            self.push(TokenKind::Colon, start, self.pos);
        }
    }

    fn number(&mut self) {
        let start = self.pos;
        if self.rest().starts_with("0x") {
            self.pos += 2;
            let digits_start = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_hexdigit() || c == '_') {
                self.bump();
            }
            let digits = self.src[digits_start..self.pos].replace('_', "");
            match i64::from_str_radix(&digits, 16) {
                Ok(n) => self.push(TokenKind::Int(n), start, self.pos),
                Err(_) => self.error(start, self.pos, "nombre hexadécimal invalide"),
            }
            return;
        }

        let digits = |lx: &mut Self| {
            while lx.peek().is_some_and(|c| c.is_ascii_digit() || c == '_') {
                lx.bump();
            }
        };
        digits(self);
        let mut is_float = false;
        // `3.days` est un appel de méthode, `3.5` un flottant
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            digits(self);
            is_float = true;
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let signed = matches!(self.peek_at(1), Some('+' | '-'));
            let digit_at = if signed { 2 } else { 1 };
            if self.peek_at(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                self.pos += digit_at;
                digits(self);
                is_float = true;
            }
        }

        let text = self.src[start..self.pos].replace('_', "");
        let kind =
            if is_float { text.parse().map(TokenKind::Float).ok() } else { text.parse().map(TokenKind::Int).ok() };
        match kind {
            Some(kind) => self.push(kind, start, self.pos),
            None => self.error(start, self.pos, "nombre hors limites"),
        }
    }

    /// Lit le contenu d'une chaîne jusqu'au terminateur. `self.pos` est juste après
    /// le guillemet ouvrant (ou au début d'une ligne de heredoc).
    fn string_parts(&mut self, term: Term, interpolate: bool, open: usize) -> Vec<StrPart> {
        let mut parts = Vec::new();
        let mut buf = String::new();
        loop {
            if let Term::End(end) = term
                && self.pos >= end
            {
                break;
            }
            let Some(c) = self.peek() else {
                // reprise à la fin de la ligne d'ouverture : le reste du fichier reste analysable
                self.error(open, self.pos, "chaîne non terminée");
                self.pos = self.line_end(open);
                break;
            };
            if let Term::Quote(q) = term
                && c == q
            {
                self.bump();
                break;
            }

            if c == '\\' {
                self.escape(&mut buf, interpolate);
            } else if interpolate && c == '#' && self.peek_at(1) == Some('{') {
                if !buf.is_empty() {
                    parts.push(StrPart::Lit(std::mem::take(&mut buf)));
                }
                if !self.interpolation(&mut parts) {
                    break;
                }
            } else {
                buf.push(c);
                self.bump();
            }
        }
        if !buf.is_empty() {
            parts.push(StrPart::Lit(buf));
        }
        parts
    }

    fn escape(&mut self, buf: &mut String, full: bool) {
        let start = self.pos;
        self.bump();
        let Some(c) = self.bump() else { return };
        if !full {
            // chaîne brute : seuls `\\` et `\'` sont des échappements
            match c {
                '\\' | '\'' => buf.push(c),
                other => {
                    buf.push('\\');
                    buf.push(other);
                }
            }
            return;
        }
        match c {
            'n' => buf.push('\n'),
            't' => buf.push('\t'),
            'r' => buf.push('\r'),
            '0' => buf.push('\0'),
            'e' => buf.push('\x1b'),
            's' => buf.push(' '),
            'u' => {
                let braced = self.eat('{');
                let hex_start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) && (braced || self.pos - hex_start < 4) {
                    self.bump();
                }
                let hex = &self.src[hex_start..self.pos];
                if braced && !self.eat('}') {
                    self.error(start, self.pos, "`}` attendu pour fermer `\\u{…}`");
                }
                match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                    Some(ch) => buf.push(ch),
                    None => self.error(start, self.pos, "échappement unicode invalide"),
                }
            }
            other => buf.push(other),
        }
    }

    /// `#{ … }` : lance un sous-lexer qui s'arrête sur la `}` fermante.
    fn interpolation(&mut self, parts: &mut Vec<StrPart>) -> bool {
        let open = self.pos;
        let mut sub = Lexer::new(self.src, open + 2, true);
        sub.run();
        if sub.peek() != Some('}') {
            // les erreurs du sous-lexer ne sont que des conséquences de celle-ci
            let end = self.line_end(open);
            self.error(open, end, "interpolation non terminée : `}` attendu");
            self.pos = end;
            return false;
        }
        self.errors.append(&mut sub.errors);
        let close = sub.pos;
        let mut tokens = join_continuations(sub.tokens);
        tokens.push(Token { kind: TokenKind::Eof, span: Span::new(close, close), space_before: false });
        parts.push(StrPart::Interp(tokens, Span::new(open, close + 1)));
        self.pos = close + 1;
        true
    }

    fn at_heredoc(&self) -> bool {
        let rest = self.rest();
        (rest.starts_with("<<~") || rest.starts_with("<<-"))
            && rest[3..].starts_with(|c: char| c == '\'' || c == '_' || c.is_ascii_uppercase())
    }

    /// `<<~ID` : le corps commence à la ligne suivante et va jusqu'à `ID` seul sur sa ligne.
    /// `~` retire l'indentation commune ; `<<~'ID'` désactive l'interpolation.
    fn heredoc(&mut self) {
        let start = self.pos;
        let squiggly = self.rest().starts_with("<<~");
        self.pos += 3;
        let raw = self.eat('\'');
        let id_start = self.pos;
        self.take_ident_chars();
        let id = self.src[id_start..self.pos].to_string();
        if raw && !self.eat('\'') {
            self.error(start, self.pos, "`'` attendu pour fermer l'identifiant du heredoc");
        }
        let opener_end = self.pos;

        let body_start =
            self.heredoc_resume.unwrap_or_else(|| self.rest().find('\n').map_or(self.src.len(), |i| self.pos + i + 1));

        let mut lines = Vec::new();
        let mut line_start = body_start;
        let mut resume = None;
        while line_start < self.src.len() {
            let line_end = self.src[line_start..].find('\n').map_or(self.src.len(), |i| line_start + i);
            if self.src[line_start..line_end].trim() == id {
                resume = Some((line_end + 1).min(self.src.len()));
                break;
            }
            lines.push((line_start, line_end));
            line_start = line_end + 1;
        }
        let resume = resume.unwrap_or_else(|| {
            self.error(start, opener_end, format!("heredoc non terminé : `{id}` attendu seul sur une ligne"));
            self.src.len()
        });

        let leading_ws = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
        let indent = if squiggly {
            lines
                .iter()
                .map(|&(s, e)| &self.src[s..e])
                .filter(|line| !line.trim().is_empty())
                .map(leading_ws)
                .min()
                .unwrap_or(0)
        } else {
            0
        };

        let saved = self.pos;
        let mut parts: Vec<StrPart> = Vec::new();
        for (line_start, line_end) in lines {
            self.pos = line_start + leading_ws(&self.src[line_start..line_end]).min(indent);
            let line_parts = self.string_parts(Term::End(line_end), !raw, start);
            for part in line_parts.into_iter().chain([StrPart::Lit("\n".into())]) {
                match (parts.last_mut(), part) {
                    (Some(StrPart::Lit(prev)), StrPart::Lit(text)) => prev.push_str(&text),
                    (_, part) => parts.push(part),
                }
            }
        }
        self.pos = saved;
        self.heredoc_resume = Some(resume);
        self.push(TokenKind::Str(parts), start, opener_end);
    }

    fn punct(&mut self) {
        use TokenKind::*;
        let start = self.pos;
        let c = self.bump().expect("punct appelé en fin de source");
        let kind = match c {
            '(' => LParen,
            ')' => RParen,
            '[' => LBracket,
            ']' => RBracket,
            '{' => {
                if self.in_interp {
                    self.brace_depth += 1;
                }
                LBrace
            }
            '}' => {
                if self.in_interp {
                    self.brace_depth -= 1;
                }
                RBrace
            }
            ',' => Comma,
            '~' => Tilde,
            '^' => Caret,
            '?' => Question,
            '%' => Percent,
            '.' if self.eat('.') => {
                if self.eat('.') {
                    DotDotDot
                } else {
                    DotDot
                }
            }
            '.' => Dot,
            '&' if self.eat('&') => {
                if self.eat('=') {
                    AndAndEq
                } else {
                    AndAnd
                }
            }
            '&' if self.eat('.') => SafeDot,
            '&' => Amp,
            '|' if self.eat('|') => {
                if self.eat('=') {
                    OrOrEq
                } else {
                    OrOr
                }
            }
            '|' => Pipe,
            '=' if self.eat('=') => EqEq,
            '=' if self.eat('~') => Match,
            '=' if self.eat('>') => FatArrow,
            '=' => Eq,
            '!' if self.eat('=') => NotEq,
            '!' => Bang,
            '<' if self.eat('=') => {
                if self.eat('>') {
                    Cmp
                } else {
                    Le
                }
            }
            '<' if self.eat('<') => Shl,
            '<' => Lt,
            '>' if self.eat('=') => Ge,
            '>' if self.eat('>') => Shr,
            '>' => Gt,
            '+' if self.eat('=') => PlusEq,
            '+' => Plus,
            '-' if self.eat('>') => Arrow,
            '-' if self.eat('=') => MinusEq,
            '-' => Minus,
            '*' if self.eat('*') => StarStar,
            '*' if self.eat('=') => StarEq,
            '*' => Star,
            '/' if self.eat('=') => SlashEq,
            '/' => Slash,
            other => {
                self.error(start, self.pos, format!("caractère inattendu `{other}`"));
                return;
            }
        };
        self.push(kind, start, self.pos);
    }
}
