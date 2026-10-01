//! The text of an element as a reader sees it: its text nodes joined,
//! entities decoded (by the parser), every run of blank space one space,
//! none at either end.
//!
//! What a browser does not show is left out: the code of a `<script>`, the
//! rules of a `<style>`, the inert content of a `<template>`, the fallback
//! of a `<noscript>` — unless the element read is one of them (the JSON of
//! a `<script type="application/ld+json">` is its text). A line break
//! (`<br>`) and the edges of a block (a paragraph, an item, a cell…)
//! separate words, as on the screen: `<li>Tea</li><li>Coffee</li>` reads
//! `Tea Coffee`, not `TeaCoffee`.

use ego_tree::iter::Edge;
use scraper::{ElementRef, Node};

/// The elements whose content is not shown.
const HIDDEN: &[&str] = &["script", "style", "template", "noscript"];

/// The elements that break a line: blocks, cells and `<br>`.
const BREAKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "caption",
    "dd",
    "details",
    "dialog",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "legend",
    "li",
    "main",
    "menu",
    "nav",
    "ol",
    "option",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

/// The text of `element` and its descendants, blank space collapsed.
pub fn of(element: ElementRef<'_>) -> String {
    let mut out = String::new();
    let mut hidden = None;
    for edge in element.traverse() {
        match edge {
            Edge::Open(node) if hidden.is_none() => match node.value() {
                Node::Text(text) => out.push_str(text),
                Node::Element(e) if node.id() != element.id() && HIDDEN.contains(&e.name()) => {
                    hidden = Some(node.id());
                }
                Node::Element(e) if BREAKS.contains(&e.name()) => out.push(' '),
                _ => {}
            },
            Edge::Close(node) if hidden == Some(node.id()) => hidden = None,
            Edge::Close(node) if hidden.is_none() => {
                if let Node::Element(e) = node.value()
                    && BREAKS.contains(&e.name())
                {
                    out.push(' ');
                }
            }
            _ => {}
        }
    }
    collapse(&out)
}

/// `text` with each run of white space as one space, trimmed.
pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use scraper::{Html, Selector};

    use super::*;

    #[test]
    fn blank_space_is_collapsed() {
        assert_eq!(collapse("  a \n\t b  c "), "a b c");
        assert_eq!(collapse(" \n "), "");
    }

    /// The text of each element `css` matches in `html`.
    fn texts(html: &str, css: &str) -> Vec<String> {
        let page = Html::parse_document(html);
        page.select(&Selector::parse(css).unwrap()).map(of).collect()
    }

    #[test]
    fn an_elements_text_includes_its_descendants() {
        assert_eq!(texts("<p>Hello <b>big</b>\n  &amp; <i>wide</i> world</p>", "p"), ["Hello big & wide world"]);
        assert_eq!(texts("<p>un<b>bold</b>ed <a href=x>link</a>s</p>", "p"), ["unbolded links"]);
    }

    #[test]
    fn what_a_browser_does_not_show_is_left_out() {
        let page = "<body><p>Hi</p><script>var token = 1;</script><style>p { color: red }</style>\
                    <noscript>Enable JavaScript</noscript><div><template>hidden</template>shown</div></body>";
        assert_eq!(texts(page, "body"), ["Hi shown"]);
        assert_eq!(texts(page, "div"), ["shown"]);
    }

    #[test]
    fn a_hidden_element_read_itself_gives_its_text() {
        let page = "<script type=\"application/ld+json\">{\"name\": \"Tea\"}</script><template>t</template>";
        assert_eq!(texts(page, "script"), ["{\"name\": \"Tea\"}"]);
        assert_eq!(texts(page, "template"), ["t"]);
    }

    #[test]
    fn line_breaks_and_blocks_separate_words() {
        assert_eq!(texts("<ul><li>Tea</li><li>Coffee</li></ul><p>a<br>b</p>", "ul, p"), ["Tea Coffee", "a b"]);
        assert_eq!(texts("<div><h1>Title</h1><p>Body</p></div>", "div"), ["Title Body"]);
        assert_eq!(texts("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>", "table"), ["a b c"]);
    }
}
