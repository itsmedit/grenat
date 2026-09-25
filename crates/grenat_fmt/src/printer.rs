//! The output being written: lines and indentation, comments put back where
//! they were, blank lines kept (one at most), heredoc bodies after their line.

use grenat_ast::Span;
use grenat_lexer::Comment;

/// Lines longer than this are broken (arrays, hashes, arguments).
pub(crate) const WIDTH: usize = 100;
const INDENT: &str = "  ";

pub(crate) struct Printer<'s> {
    pub src: &'s str,
    comments: Vec<Comment>,
    /// Comments already printed are before this index.
    next_comment: usize,
    pub out: String,
    pub indent: usize,
    /// Heredoc bodies to write after the current line: (indentation, lines, terminator).
    heredocs: Vec<(usize, Vec<String>, String)>,
    /// End of the last source element printed (to see blank lines).
    last_pos: usize,
    /// Nothing printed yet in the current block: no blank line.
    block_start: bool,
    /// For measuring only: comments are not printed.
    pub measuring: bool,
    /// Lines ending with a comment: (line number, width of the code before it).
    trailing: Vec<(usize, usize)>,
}

impl<'s> Printer<'s> {
    pub fn new(src: &'s str, comments: Vec<Comment>) -> Printer<'s> {
        Printer {
            src,
            comments,
            next_comment: 0,
            out: String::new(),
            indent: 0,
            heredocs: Vec::new(),
            last_pos: 0,
            block_start: true,
            measuring: false,
            trailing: Vec::new(),
        }
    }

    /// A printer writing on one line, to measure how wide something is.
    pub fn measurer(&self) -> Printer<'s> {
        let mut p = Printer::new(self.src, Vec::new());
        p.measuring = true;
        p.indent = self.indent;
        p
    }

    // ── Text ─────────────────────────────────────────────────

    pub fn write(&mut self, text: &str) {
        // a measure is taken from the middle of a line: no indentation
        if self.at_line_start() && !text.is_empty() && !self.measuring {
            for _ in 0..self.indent {
                self.out.push_str(INDENT);
            }
        }
        self.out.push_str(text);
    }

    pub fn at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n')
    }

    /// Characters on the current line.
    pub fn column(&self) -> usize {
        let line = self.out.rsplit('\n').next().unwrap_or("");
        let indent = if line.is_empty() { self.indent * INDENT.len() } else { 0 };
        line.chars().count() + indent
    }

    /// Ends the line (then writes the heredoc bodies opened on it).
    pub fn newline(&mut self) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        self.out.push('\n');
        for (indent, lines, terminator) in std::mem::take(&mut self.heredocs) {
            for line in lines {
                if !line.is_empty() {
                    for _ in 0..indent {
                        self.out.push_str(INDENT);
                    }
                }
                self.out.push_str(&line);
                self.out.push('\n');
            }
            for _ in 0..indent.saturating_sub(1) {
                self.out.push_str(INDENT);
            }
            self.out.push_str(&terminator);
            self.out.push('\n');
        }
    }

    /// A heredoc body, written after the current line at `indent`.
    pub fn defer_heredoc(&mut self, lines: Vec<String>, terminator: String, indent: usize) {
        self.heredocs.push((indent, lines, terminator));
    }

    /// A `<<-` heredoc body: written exactly as it was.
    pub fn defer_heredoc_raw(&mut self, lines: Vec<String>, terminator: String) {
        self.heredocs.push((0, lines, terminator));
    }

    pub fn indented(&mut self, f: impl FnOnce(&mut Self)) {
        self.indent += 1;
        let outer = std::mem::replace(&mut self.block_start, true);
        f(self);
        self.block_start = outer;
        self.indent -= 1;
    }

    /// The source text of `span`.
    pub fn source(&self, span: Span) -> &'s str {
        &self.src[span.start as usize..span.end as usize]
    }

    // ── Comments and blank lines ─────────────────────────────

    /// Before an element starting a line at `start`: the comments above it,
    /// then a blank line if the source had one.
    pub fn line_element(&mut self, start: u32) {
        if self.measuring {
            return;
        }
        while let Some(comment) = self.comments.get(self.next_comment).cloned() {
            if comment.span.start >= start {
                break;
            }
            self.next_comment += 1;
            self.blank_line_before(comment.span.start);
            let text = self.source(comment.span).trim_end().to_string();
            self.write(&text);
            self.newline();
            self.last_pos = comment.span.end as usize;
            self.block_start = false;
        }
        self.blank_line_before(start);
        self.block_start = false;
        self.last_pos = self.last_pos.max(start as usize);
    }

    /// After an element ending at `end`: a comment that followed it on the
    /// same line stays at the end of the line.
    pub fn line_end(&mut self, end: u32) {
        if self.measuring {
            return;
        }
        self.last_pos = self.last_pos.max(end as usize);
        if let Some(comment) = self.comments.get(self.next_comment).cloned()
            && comment.trailing
            && comment.span.start >= end
            && !self.src[end as usize..comment.span.start as usize].contains('\n')
        {
            self.next_comment += 1;
            let text = self.source(comment.span).trim_end().to_string();
            let line = self.out.matches('\n').count();
            self.trailing.push((line, self.column()));
            self.write("  ");
            self.write(&text);
            self.last_pos = comment.span.end as usize;
        }
    }

    /// The comments left before `end` (the end of an enclosing construct).
    pub fn comments_before(&mut self, end: u32) {
        if self.measuring {
            return;
        }
        while let Some(comment) = self.comments.get(self.next_comment).cloned() {
            if comment.span.start >= end {
                break;
            }
            self.line_element(comment.span.end);
        }
    }

    /// A single blank line if the source has one between the last element and `start`.
    fn blank_line_before(&mut self, start: u32) {
        let start = start as usize;
        if self.block_start || self.out.is_empty() || start <= self.last_pos || !self.at_line_start() {
            return;
        }
        let between = &self.src[self.last_pos..start];
        let lines: Vec<&str> = between.split('\n').collect();
        // the first piece ends the previous element's line, the last one starts this one
        let blank = lines.len() > 2 && lines[1..lines.len() - 1].iter().any(|l| l.trim().is_empty());
        if blank && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    /// The comments not printed yet (at the end of the file), and end-of-line
    /// comments of consecutive lines aligned.
    pub fn finish(mut self) -> String {
        self.comments_before(u32::MAX);
        let mut lines: Vec<String> = self.out.split('\n').map(str::to_string).collect();
        let mut group: Vec<(usize, usize)> = Vec::new();
        let groups = self.trailing.iter().fold(Vec::new(), |mut groups: Vec<Vec<(usize, usize)>>, &(line, width)| {
            match groups.last_mut() {
                Some(last) if last.last().is_some_and(|&(l, _)| l + 1 == line) => last.push((line, width)),
                _ => groups.push(vec![(line, width)]),
            }
            groups
        });
        for g in groups {
            group.clear();
            group.extend(g);
            let column = group.iter().map(|&(_, w)| w).max().unwrap_or(0);
            for &(line, width) in &group {
                let text = &lines[line];
                // `width` counts characters: split at the matching byte
                let at = text.char_indices().nth(width).map_or(text.len(), |(i, _)| i);
                let (code, comment) = text.split_at(at);
                lines[line] = format!("{code}{}  {}", " ".repeat(column - width), comment.trim_start());
            }
        }
        self.out = lines.join("\n");
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }
}
