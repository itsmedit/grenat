//! La machine de lecture : parcours du source, identifiants, nombres, ponctuation, commentaires.

use crate::*;

pub fn lex(src: &str) -> Lexed {
    let mut lx = Lexer::new(src, 0, false);
    lx.run();
    lx.push(TokenKind::Eof, src.len(), src.len());
    Lexed { tokens: join_continuations(lx.tokens), comments: lx.comments, errors: lx.errors }
}

/// Supprime les fins de ligne suivies d'un `.` ou `&.` : `answer\n  .check { … }`.
pub(crate) fn join_continuations(tokens: Vec<Token>) -> Vec<Token> {
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

pub(crate) fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

pub(crate) fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

pub(crate) struct Lexer<'s> {
    pub(crate) src: &'s str,
    pub(crate) pos: usize,
    pub(crate) tokens: Vec<Token>,
    pub(crate) comments: Vec<Comment>,
    pub(crate) errors: Vec<LexError>,
    /// Après un heredoc, où reprendre à la prochaine fin de ligne (après son terminateur).
    pub(crate) heredoc_resume: Option<usize>,
    /// Sous-lexer d'une interpolation : s'arrête sur la `}` fermante.
    pub(crate) in_interp: bool,
    pub(crate) brace_depth: u32,
    pub(crate) space_before: bool,
    pub(crate) line_has_token: bool,
}

impl<'s> Lexer<'s> {
    pub(crate) fn new(src: &'s str, pos: usize, in_interp: bool) -> Self {
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

    pub(crate) fn rest(&self) -> &'s str {
        &self.src[self.pos..]
    }

    pub(crate) fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    pub(crate) fn peek_at(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    pub(crate) fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    pub(crate) fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn line_end(&self, from: usize) -> usize {
        self.src[from..].find('\n').map_or(self.src.len(), |i| from + i)
    }

    pub(crate) fn error(&mut self, start: usize, end: usize, message: impl Into<String>) {
        self.errors.push(LexError { span: Span::new(start, end), message: message.into() });
    }

    pub(crate) fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token { kind, span: Span::new(start, end), space_before: self.space_before });
        self.line_has_token = true;
    }

    pub(crate) fn run(&mut self) {
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
    pub(crate) fn skip_blanks(&mut self) -> bool {
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

    pub(crate) fn push_newline(&mut self, start: usize) {
        let redundant = matches!(self.tokens.last(), None | Some(Token { kind: TokenKind::Newline, .. }));
        if !redundant {
            self.push(TokenKind::Newline, start, start + 1);
        }
        self.line_has_token = false;
    }

    pub(crate) fn newline(&mut self, start: usize) {
        self.push_newline(start);
        if let Some(resume) = self.heredoc_resume.take() {
            self.pos = resume;
        }
    }

    pub(crate) fn comment(&mut self) {
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

    pub(crate) fn take_ident_chars(&mut self) {
        while let Some(c) = self.peek() {
            if !is_ident_continue(c) {
                break;
            }
            self.bump();
        }
    }

    /// `?`/`!` final d'un nom de méthode, sauf `a!=b` ou `a?=…`.
    pub(crate) fn take_predicate_suffix(&mut self) {
        if matches!(self.peek(), Some('?' | '!')) && self.peek_at(1) != Some('=') {
            self.bump();
        }
    }

    pub(crate) fn ident(&mut self) {
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

    pub(crate) fn ivar(&mut self) {
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

    pub(crate) fn colon(&mut self) {
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
                TokenKind::LParen
                    | TokenKind::LBracket
                    | TokenKind::LBrace
                    | TokenKind::Comma
                    | TokenKind::Pipe
                    | TokenKind::Amp
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

    pub(crate) fn number(&mut self) {
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

    pub(crate) fn punct(&mut self) {
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
