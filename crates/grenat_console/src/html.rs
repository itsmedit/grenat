//! Pages: escaping, the layout every page shares, and small pieces (times,
//! money, badges, tables).

/// `text` safe in HTML, as content or as an attribute's value.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The sections of the console, as the navigation lists them.
pub const SECTIONS: [(&str, &str); 8] = [
    ("/", "Overview"),
    ("/approvals", "Approvals"),
    ("/jobs", "Jobs"),
    ("/journals", "Journals"),
    ("/costs", "Costs"),
    ("/evals", "Evals"),
    ("/events", "Events"),
    ("/mcp", "MCP"),
];

/// What the layout needs to know besides the page.
pub struct Frame<'a> {
    pub app: &'a str,
    /// The section shown (a path of [`SECTIONS`]).
    pub section: &'a str,
    /// Approvals waiting, shown next to their section.
    pub pending: usize,
    /// Signed in with a token: a sign-out button.
    pub signed_in: bool,
    pub csrf: &'a str,
}

/// A whole page: `body` is HTML already.
pub fn layout(frame: &Frame, title: &str, body: &str) -> String {
    let nav: String = SECTIONS
        .iter()
        .map(|(path, label)| {
            let current = if *path == frame.section { " aria-current=\"page\"" } else { "" };
            let count = if *path == "/approvals" && frame.pending > 0 { format!(" <span class=\"count\">{}</span>", frame.pending) } else { String::new() };
            format!("<a href=\"{path}\"{current}>{label}{count}</a>")
        })
        .collect();
    let sign_out = if frame.signed_in {
        format!("<form method=\"post\" action=\"/logout\">{}<button class=\"link\">Sign out</button></form>", csrf_field(frame.csrf))
    } else {
        String::new()
    };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title} · {app}</title><style>{STYLE}</style></head><body>\
         <header><strong>{app}</strong><nav>{nav}</nav>{sign_out}</header>\
         <main><h1>{title}</h1>{body}</main></body></html>",
        title = escape(title),
        app = escape(frame.app),
    )
}

/// A page without navigation (signing in, errors before it).
pub fn bare(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title><style>{STYLE}</style></head><body><main class=\"narrow\"><h1>{title}</h1>{body}</main></body></html>",
        title = escape(title)
    )
}

pub fn csrf_field(csrf: &str) -> String {
    format!("<input type=\"hidden\" name=\"csrf\" value=\"{}\">", escape(csrf))
}

/// A button posting to `action`.
pub fn button(action: &str, label: &str, class: &str, csrf: &str) -> String {
    format!(
        "<form method=\"post\" action=\"{}\" class=\"inline\">{}<button class=\"{class}\">{}</button></form>",
        escape(action),
        csrf_field(csrf),
        escape(label)
    )
}

/// A table: `head` are titles, `rows` HTML cells; `empty` when there is none.
pub fn table(head: &[&str], rows: &[Vec<String>], empty: &str) -> String {
    if rows.is_empty() {
        return format!("<p class=\"empty\">{}</p>", escape(empty));
    }
    let head: String = head.iter().map(|h| format!("<th>{}</th>", escape(h))).collect();
    let rows: String = rows
        .iter()
        .map(|cells| format!("<tr>{}</tr>", cells.iter().map(|c| format!("<td>{c}</td>")).collect::<String>()))
        .collect();
    format!("<div class=\"scroll\"><table><thead><tr>{head}</tr></thead><tbody>{rows}</tbody></table></div>")
}

pub fn badge(text: &str, tone: &str) -> String {
    format!("<span class=\"badge {tone}\">{}</span>", escape(text))
}

/// `2026-09-25 14:03 UTC`.
pub fn time(t: f64) -> String {
    if t <= 0.0 {
        return "—".into();
    }
    let seconds = t as i64;
    format!("{} {:02}:{:02}", grenat_ops::calls::day(t), seconds.rem_euclid(86_400) / 3600, seconds.rem_euclid(3600) / 60)
}

/// `3 min ago`, `in 2 h`: `t` from `now`, with the time in a tooltip.
pub fn ago(t: f64, now: f64) -> String {
    if t <= 0.0 {
        return "—".into();
    }
    let delta = now - t;
    let span = |s: f64| match s {
        s if s < 60.0 => format!("{} s", s as i64),
        s if s < 3600.0 => format!("{} min", (s / 60.0) as i64),
        s if s < 86_400.0 => format!("{} h", (s / 3600.0) as i64),
        s => format!("{} d", (s / 86_400.0) as i64),
    };
    let text = if delta >= 0.0 { format!("{} ago", span(delta)) } else { format!("in {}", span(-delta)) };
    format!("<time title=\"{} UTC\">{text}</time>", time(t))
}

/// `$0.0123`, `$12.40`.
pub fn money(usd: f64) -> String {
    // an empty sum is -0.0
    let usd = usd + 0.0;
    if usd != 0.0 && usd.abs() < 1.0 { format!("${usd:.4}") } else { format!("${usd:.2}") }
}

/// `text` cut to `max` characters, the whole of it in a tooltip.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return escape(text);
    }
    let cut: String = text.chars().take(max).collect();
    format!("<span title=\"{}\">{}…</span>", escape(text), escape(&cut))
}

/// A value in the runtime's encoding, indented.
pub fn json(value: &serde_json::Value) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    format!("<pre>{}</pre>", escape(&text))
}

/// A notice at the top of a page (after an action).
pub fn notice(text: &str) -> String {
    format!("<p class=\"notice\" role=\"status\">{}</p>", escape(text))
}

const STYLE: &str = "\
:root{--bg:#fbfaf9;--fg:#1d1b1a;--muted:#6b6560;--line:#e6e1dc;--card:#fff;--accent:#8b1e3f;\
--ok:#1f7a4d;--warn:#9a6700;--bad:#b42318;color-scheme:light dark}\
@media (prefers-color-scheme:dark){:root{--bg:#151312;--fg:#ece8e4;--muted:#a39d97;--line:#2e2a27;\
--card:#1d1a18;--accent:#e26d8f;--ok:#4cc38a;--warn:#e0b341;--bad:#f07167}}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);\
font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
header{display:flex;flex-wrap:wrap;gap:12px 24px;align-items:center;padding:12px 16px;\
border-bottom:1px solid var(--line);background:var(--card)}\
header strong{color:var(--accent)}nav{display:flex;flex-wrap:wrap;gap:4px 14px;flex:1}\
nav a{color:var(--muted);text-decoration:none}nav a[aria-current]{color:var(--fg);font-weight:600}\
.count{background:var(--accent);color:#fff;border-radius:9px;padding:0 7px;font-size:12px}\
main{max-width:1100px;margin:0 auto;padding:16px}main.narrow{max-width:420px}\
h1{font-size:22px;margin:8px 0 16px}h2{font-size:17px;margin:28px 0 10px}\
a{color:var(--accent)}.scroll{overflow-x:auto}\
table{width:100%;border-collapse:collapse;background:var(--card);border:1px solid var(--line)}\
th,td{text-align:left;padding:7px 10px;border-bottom:1px solid var(--line);vertical-align:top}\
th{font-size:12px;text-transform:uppercase;letter-spacing:.04em;color:var(--muted)}\
td.num,th.num{text-align:right;font-variant-numeric:tabular-nums}\
.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px}\
.card{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px 14px}\
a.card{text-decoration:none;color:inherit}.card b{display:block;font-size:24px}.card span{color:var(--muted);font-size:13px}\
.badge{display:inline-block;border-radius:4px;padding:0 6px;font-size:12px;border:1px solid currentColor}\
.ok{color:var(--ok)}.warn{color:var(--warn)}.bad{color:var(--bad)}.muted{color:var(--muted)}\
pre{background:var(--card);border:1px solid var(--line);padding:8px 10px;overflow-x:auto;\
white-space:pre-wrap;word-break:break-word;margin:0;font-size:13px}\
form.inline{display:inline}button{font:inherit;border:1px solid var(--line);background:var(--card);\
color:var(--fg);border-radius:6px;padding:3px 10px;cursor:pointer}\
button.primary{background:var(--accent);border-color:var(--accent);color:#fff}\
button.link{border:0;background:none;color:var(--muted);padding:0}\
.tabs{display:flex;flex-wrap:wrap;gap:6px;margin-bottom:12px}.tabs a{padding:2px 10px;border:1px solid var(--line);\
border-radius:14px;text-decoration:none;color:var(--muted)}.tabs a[aria-current]{color:var(--fg);border-color:var(--fg)}\
.notice{background:var(--card);border-left:3px solid var(--ok);padding:8px 12px}\
.empty{color:var(--muted)}.bar{height:8px;background:var(--accent);border-radius:4px;min-width:2px}\
input{font:inherit;padding:6px 8px;width:100%;border:1px solid var(--line);border-radius:6px;\
background:var(--card);color:var(--fg)}label{display:block;margin:12px 0 4px}\
dl{display:grid;grid-template-columns:max-content 1fr;gap:4px 16px}dt{color:var(--muted)}dd{margin:0}\
svg{vertical-align:middle}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces() {
        assert_eq!(escape("<a href=\"x\">'&'</a>"), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
        assert_eq!(time(1_790_000_000.0), "2026-09-21 14:13");
        assert!(ago(100.0, 400.0).contains(">5 min ago<"));
        assert!(ago(500.0, 400.0).contains(">in 1 min<"));
        assert_eq!(money(0.01234), "$0.0123");
        assert_eq!(money(12.4), "$12.40");
        assert_eq!(money(0.0), "$0.00");
        assert_eq!(money(Vec::<f64>::new().into_iter().sum()), "$0.00");
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("<abcdef>", 3), "<span title=\"&lt;abcdef&gt;\">&lt;ab…</span>");
        assert_eq!(table(&["a"], &[], "nothing"), "<p class=\"empty\">nothing</p>");
    }
}
