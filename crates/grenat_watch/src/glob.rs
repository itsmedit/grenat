//! Glob patterns: `inbox/*.pdf`, `drive/**/*.{pdf,PDF}`, `scans/2026-??/*`.
//!
//! A pattern is a directory written out (its *base*: the segments before
//! the first one holding `*`, `?`, `[` or `{`) and what files under it must
//! match. In a segment, `*` matches any characters but `/`, `?` one, and
//! `[abc]`, `[a-z]`, `[!a-z]` one of a set (or not); `{pdf,PDF}` is either
//! alternative (nested ones too, [`MAX_ALTERNATIVES`] at most); `**`, a
//! segment of its own, matches any number of directories, none included.
//! There is no escaping: a name holding `*` is matched by a wildcard.
//!
//! Hidden names (`.~lock.a.pdf#`, `.dropbox.cache`) are left out, as a
//! shell leaves them out: a wildcard matches a name starting with `.` only
//! when the pattern's segment starts with a written `.`, and `**` never
//! goes through such a directory.

use std::path::{Path, PathBuf};

/// How many patterns `{…}` may expand to.
pub const MAX_ALTERNATIVES: usize = 64;

/// A pattern, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct Glob {
    pattern: String,
    base: PathBuf,
    /// Each alternative: the segments below `base`.
    alternatives: Vec<Vec<Segment>>,
}

#[derive(Debug, Clone, PartialEq)]
enum Segment {
    /// `**`: any number of directories.
    Directories,
    Name(Vec<Token>),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Char(char),
    /// `*`
    Any,
    /// `?`
    One,
    /// `[a-z]`, `[!x]`
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl Glob {
    /// Parses `pattern`; a file's path relative or absolute, never empty
    /// nor ending with `/`.
    pub fn parse(pattern: &str) -> Result<Glob, String> {
        let invalid = |why: &str| Err(format!("invalid file pattern {pattern:?}: {why}"));
        if pattern.trim().is_empty() {
            return invalid("it is empty");
        }
        if pattern.ends_with('/') {
            return invalid("it names a directory: say which files (`inbox/*`)");
        }
        if pattern.contains("//") {
            return invalid("it holds an empty segment (`//`)");
        }
        if pattern.contains('\0') {
            return invalid("it holds a NUL character");
        }
        let segments: Vec<&str> = pattern.split('/').collect();
        // the base: the segments written out, but never the last one
        let written = segments[..segments.len() - 1].iter().take_while(|s| !is_pattern(s)).count();
        let base = match segments[..written].join("/") {
            b if b.is_empty() && pattern.starts_with('/') => "/".to_string(),
            b if b.is_empty() => ".".to_string(),
            b => b,
        };
        let rest = segments[written..].join("/");
        let mut alternatives = Vec::new();
        for expanded in expand(&rest).map_err(|why| format!("invalid file pattern {pattern:?}: {why}"))? {
            let mut parsed = Vec::new();
            for segment in expanded.split('/') {
                parsed.push(match segment {
                    "" => return invalid("it holds an empty segment (`//`)"),
                    "**" => Segment::Directories,
                    s if s.contains("**") => return invalid("`**` is a segment of its own (`a/**/b`)"),
                    s => Segment::Name(tokens(s).map_err(|why| format!("invalid file pattern {pattern:?}: {why}"))?),
                });
            }
            if matches!(parsed.last(), Some(Segment::Directories)) {
                return invalid("it ends with `**`: say which files (`**/*`)");
            }
            alternatives.push(parsed);
        }
        Ok(Glob { pattern: pattern.to_string(), base: PathBuf::from(base), alternatives })
    }

    /// The pattern as written.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// The directory written out, where the files are looked for
    /// (`inbox` for `inbox/*.pdf`, `.` for `*.pdf`).
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// How deep under the base a file may be: `None` without limit (`**`),
    /// `Some(1)` for the base's own files.
    pub fn depth(&self) -> Option<usize> {
        let mut deepest = 0;
        for alternative in &self.alternatives {
            if alternative.contains(&Segment::Directories) {
                return None;
            }
            deepest = deepest.max(alternative.len());
        }
        Some(deepest)
    }

    /// Whether the file `relative` (its names under the base) matches.
    pub fn matches_names(&self, relative: &[&str]) -> bool {
        self.alternatives.iter().any(|alternative| matches_from(alternative, relative))
    }

    /// Whether files under the directory `relative` (its names under the
    /// base) may match: a scan goes through it.
    pub fn may_enter(&self, relative: &[&str]) -> bool {
        self.alternatives.iter().any(|alternative| enters(alternative, relative))
    }

    /// Whether the file at `path` (as the pattern writes it: under the
    /// base, `inbox/a.pdf` for `inbox/*.pdf`) matches.
    pub fn matches(&self, path: &str) -> bool {
        let base = self.base.to_string_lossy();
        let rest = match base.as_ref() {
            "." => path.strip_prefix("./").unwrap_or(path),
            "/" => match path.strip_prefix('/') {
                Some(rest) => rest,
                None => return false,
            },
            base => match path.strip_prefix(base).and_then(|r| r.strip_prefix('/')) {
                Some(rest) => rest,
                None => return false,
            },
        };
        let names: Vec<&str> = rest.split('/').collect();
        !names.iter().any(|n| n.is_empty() || *n == "." || *n == "..") && self.matches_names(&names)
    }
}

fn is_pattern(segment: &str) -> bool {
    segment.contains(['*', '?', '[', '{'])
}

/// `{a,b}` expanded: every pattern the text stands for.
fn expand(text: &str) -> Result<Vec<String>, String> {
    let Some(open) = text.find('{') else {
        if text.contains('}') {
            return Err("a `}` closes no `{`".into());
        }
        return Ok(vec![text.to_string()]);
    };
    // the matching brace, and the commas at its own depth
    let (mut depth, mut close, mut commas) = (0, None, Vec::new());
    for (i, c) in text[open..].char_indices().map(|(i, c)| (open + i, c)) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            ',' if depth == 1 => commas.push(i),
            _ => {}
        }
    }
    let Some(close) = close else { return Err("a `{` is never closed".into()) };
    let (head, tail) = (&text[..open], &text[close + 1..]);
    let mut bounds = vec![open];
    bounds.extend(commas);
    bounds.push(close);
    let mut out = Vec::new();
    for pair in bounds.windows(2) {
        let choice = &text[pair[0] + 1..pair[1]];
        for expanded in expand(&format!("{head}{choice}{tail}"))? {
            out.push(expanded);
            if out.len() > MAX_ALTERNATIVES {
                return Err(format!("`{{…}}` stands for over {MAX_ALTERNATIVES} patterns"));
            }
        }
    }
    Ok(out)
}

fn tokens(segment: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(match c {
            '*' => Token::Any,
            '?' => Token::One,
            '[' => {
                let negated = chars.next_if(|c| *c == '!' || *c == '^').is_some();
                let mut ranges = Vec::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    // `]` first is a member: `[]]`
                    if c == ']' && !ranges.is_empty() {
                        closed = true;
                        break;
                    }
                    if chars.peek() == Some(&'-') {
                        chars.next();
                        match chars.next() {
                            Some(']') => {
                                ranges.extend([(c, c), ('-', '-')]);
                                closed = true;
                                break;
                            }
                            Some(end) if end >= c => ranges.push((c, end)),
                            Some(end) => return Err(format!("the range `{c}-{end}` is reversed")),
                            None => break,
                        }
                    } else {
                        ranges.push((c, c));
                    }
                }
                if !closed {
                    return Err("a `[` is never closed".into());
                }
                Token::Class { negated, ranges }
            }
            c => Token::Char(c),
        });
    }
    Ok(out)
}

fn matches_from(segments: &[Segment], names: &[&str]) -> bool {
    match (segments.first(), names.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(Segment::Directories), _) => {
            // none, or one more directory (never a hidden one), then the rest
            matches_from(&segments[1..], names)
                || (names.len() > 1 && !names[0].starts_with('.') && matches_from(segments, &names[1..]))
        }
        (Some(Segment::Name(_)), None) => false,
        (Some(Segment::Name(tokens)), Some(name)) => {
            let hidden_ok = !name.starts_with('.') || matches!(tokens.first(), Some(Token::Char('.')));
            hidden_ok
                && matches_name(tokens, &name.chars().collect::<Vec<_>>())
                && matches_from(&segments[1..], &names[1..])
        }
    }
}

fn enters(segments: &[Segment], names: &[&str]) -> bool {
    match (segments.first(), names.first()) {
        // a segment is left for the files
        (_, None) => !segments.is_empty(),
        (None, Some(_)) => false,
        (Some(Segment::Directories), Some(name)) => {
            enters(&segments[1..], names) || (!name.starts_with('.') && enters(segments, &names[1..]))
        }
        (Some(Segment::Name(tokens)), Some(name)) => {
            let hidden_ok = !name.starts_with('.') || matches!(tokens.first(), Some(Token::Char('.')));
            hidden_ok && matches_name(tokens, &name.chars().collect::<Vec<_>>()) && enters(&segments[1..], &names[1..])
        }
    }
}

/// One name against one segment: wildcards with backtracking on the last `*`.
fn matches_name(tokens: &[Token], name: &[char]) -> bool {
    let (mut t, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match tokens.get(t) {
            Some(Token::Any) => {
                star = Some((t, n));
                t += 1;
            }
            Some(token) if fits(token, name[n]) => {
                t += 1;
                n += 1;
            }
            _ => match star {
                Some((st, sn)) => {
                    t = st + 1;
                    n = sn + 1;
                    star = Some((st, sn + 1));
                }
                None => return false,
            },
        }
    }
    tokens[t..].iter().all(|token| *token == Token::Any)
}

fn fits(token: &Token, c: char) -> bool {
    match token {
        Token::Char(x) => *x == c,
        Token::One => true,
        Token::Any => false,
        Token::Class { negated, ranges } => ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)) != *negated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob(p: &str) -> Glob {
        Glob::parse(p).unwrap()
    }

    #[test]
    fn the_base_is_the_directory_written_out() {
        assert_eq!(glob("inbox/*.pdf").base(), Path::new("inbox"));
        assert_eq!(glob("*.pdf").base(), Path::new("."));
        assert_eq!(glob("/srv/drop/**/*.pdf").base(), Path::new("/srv/drop"));
        assert_eq!(glob("/*.pdf").base(), Path::new("/"));
        assert_eq!(glob("a/b/{x,y}/*.pdf").base(), Path::new("a/b"));
        // a file written out: its directory
        assert_eq!(glob("inbox/report.csv").base(), Path::new("inbox"));
        assert_eq!(glob("../shared/*").base(), Path::new("../shared"));
    }

    #[test]
    fn wildcards_match_within_a_name() {
        let g = glob("inbox/*.pdf");
        assert!(g.matches("inbox/a.pdf"));
        assert!(!g.matches("inbox/.pdf"));
        assert!(!g.matches("inbox/a.PDF"));
        assert!(!g.matches("inbox/sub/a.pdf"));
        assert!(!g.matches("other/a.pdf"));
        assert!(!g.matches("inboxa.pdf"));
        assert!(glob("scans/2026-??/*").matches("scans/2026-03/x"));
        assert!(!glob("scans/2026-??/*").matches("scans/2026-3/x"));
        assert!(glob("a/*b*c").matches("a/xxbyyc"));
        assert!(glob("a/*b*c").matches("a/bc"));
        assert!(!glob("a/*b*c").matches("a/bcx"));
        assert!(glob("[a-c]?.txt").matches("b1.txt"));
        assert!(!glob("[!a-c]?.txt").matches("b1.txt"));
        assert!(glob("[]x]").matches("]"));
        assert!(glob("[a-]").matches("-"));
        assert!(glob("*.pdf").matches("a.pdf") && glob("*.pdf").matches("./a.pdf"));
    }

    #[test]
    fn alternatives_and_directories() {
        let g = glob("drive/**/*.{pdf,PDF}");
        assert!(g.matches("drive/a.pdf"));
        assert!(g.matches("drive/2026/03/a.PDF"));
        assert!(!g.matches("drive/2026/a.txt"));
        assert_eq!(g.depth(), None);
        assert_eq!(glob("inbox/*.pdf").depth(), Some(1));
        assert_eq!(glob("inbox/{*,*/*}.pdf").depth(), Some(2));
        assert!(glob("a/{x,y{1,2}}/*").matches("a/y2/f"));
        assert!(glob("a/**/z/*").matches("a/z/f"));
        assert!(glob("a/**/z/*").matches("a/b/c/z/f"));
        // which directories a scan goes through
        let g = glob("a/**/z/*.pdf");
        assert!(g.may_enter(&["b", "c"]) && g.may_enter(&["z"]) && !g.may_enter(&[".git"]));
        assert!(!glob("a/*.pdf").may_enter(&["b"]));
        assert!(glob("a/2026-*/*.pdf").may_enter(&["2026-03"]) && !glob("a/2026-*/*.pdf").may_enter(&["2025-03"]));
        assert!(!glob("a/2026-*/*.pdf").may_enter(&["2026-03", "x"]));
        assert!(glob("a/.c*/*").may_enter(&[".cache"]) && !glob("a/*/*").may_enter(&[".cache"]));
    }

    #[test]
    fn hidden_names_need_a_written_dot() {
        assert!(!glob("inbox/*").matches("inbox/.~lock.a.pdf#"));
        assert!(glob("inbox/.*").matches("inbox/.hidden"));
        assert!(!glob("drive/**/*.pdf").matches("drive/.cache/a.pdf"));
        assert!(!glob("drive/*/*.pdf").matches("drive/.cache/a.pdf"));
        assert!(!glob("inbox/*").matches("inbox/../secret"));
    }

    #[test]
    fn invalid_patterns_say_why() {
        for (pattern, why) in [
            ("", "empty"),
            ("inbox/", "names a directory"),
            ("a//*.pdf", "empty segment"),
            ("a/**.pdf", "segment of its own"),
            ("a/**", "ends with `**`"),
            ("a/[bc", "never closed"),
            ("a/[z-a]", "reversed"),
            ("a/{b,c", "never closed"),
            ("a/b}*", "closes no"),
        ] {
            let e = Glob::parse(pattern).unwrap_err();
            assert!(e.contains(why), "{pattern}: {e}");
        }
        let many = "{a,b}{c,d}{e,f}{g,h}{i,j}{k,l}{m,n}";
        assert!(Glob::parse(many).unwrap_err().contains("over 64"));
    }
}
