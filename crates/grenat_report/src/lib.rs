//! Diagnostic rendering, rustc style.

use grenat_ast::{Diagnostic, Span};

/// Line and column (in characters), 1-based.
pub fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let before = &src[..offset.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, col)
}

struct Painter(bool);

impl Painter {
    fn paint(&self, ansi: &str, text: &str) -> String {
        if self.0 { format!("\x1b[{ansi}m{text}\x1b[0m") } else { text.to_string() }
    }
}

pub fn render(path: &str, src: &str, diag: &Diagnostic, color: bool) -> String {
    let p = Painter(color);
    let title = match diag.code {
        Some(code) => format!("error[{code}]"),
        None => "error".into(),
    };
    let mut out = format!("{}: {}\n", p.paint("1;31", &title), p.paint("1", &diag.message));
    snippet(&mut out, &p, path, src, diag.span, "1;31");
    for (span, note) in &diag.notes {
        out.push_str(&format!("{}: {note}\n", p.paint("1;36", "note")));
        snippet(&mut out, &p, path, src, *span, "1;36");
    }
    if let Some(help) = &diag.help {
        out.push_str(&format!("  = {}: {help}\n", p.paint("1", "help")));
    }
    out.push('\n');
    out
}

fn snippet(out: &mut String, p: &Painter, path: &str, src: &str, span: Span, ansi: &str) {
    let (line, col) = line_col(src, span.start as usize);
    let text = src.lines().nth(line - 1).unwrap_or("");
    let gutter = " ".repeat(line.to_string().len());
    let bar = p.paint("1;34", "|");

    let start = (span.start as usize).min(src.len());
    let line_end = src[start..].find('\n').map_or(src.len(), |i| start + i);
    let end = (span.end as usize).clamp(start, line_end);
    let width = src[start..end].chars().count().max(1);

    out.push_str(&format!("{gutter}{} {path}:{line}:{col}\n", p.paint("1;34", "-->")));
    out.push_str(&format!("{gutter} {bar}\n"));
    out.push_str(&format!("{} {bar} {text}\n", p.paint("1;34", &line.to_string())));
    out.push_str(&format!("{gutter} {bar} {}{}\n", " ".repeat(col - 1), p.paint(ansi, &"^".repeat(width))));
}
