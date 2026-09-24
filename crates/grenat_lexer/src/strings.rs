//! Chaînes : échappements, interpolation `#{…}` (sous-lexer) et heredocs `<<~ID`.

use crate::*;

pub(crate) enum Term {
    Quote(char),
    End(usize),
}

impl<'s> Lexer<'s> {
    /// Lit le contenu d'une chaîne jusqu'au terminateur. `self.pos` est juste après
    /// le guillemet ouvrant (ou au début d'une ligne de heredoc).
    pub(crate) fn string_parts(&mut self, term: Term, interpolate: bool, open: usize) -> Vec<StrPart> {
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

    pub(crate) fn escape(&mut self, buf: &mut String, full: bool) {
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
    pub(crate) fn interpolation(&mut self, parts: &mut Vec<StrPart>) -> bool {
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

    pub(crate) fn at_heredoc(&self) -> bool {
        let rest = self.rest();
        (rest.starts_with("<<~") || rest.starts_with("<<-"))
            && rest[3..].starts_with(|c: char| c == '\'' || c == '_' || c.is_ascii_uppercase())
    }

    /// `<<~ID` : le corps commence à la ligne suivante et va jusqu'à `ID` seul sur sa ligne.
    /// `~` retire l'indentation commune ; `<<~'ID'` désactive l'interpolation.
    pub(crate) fn heredoc(&mut self) {
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
}
