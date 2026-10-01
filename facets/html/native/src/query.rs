//! Selecting elements of a document with a CSS selector, and what is read
//! from each match, in document order: its text, an attribute, its HTML.

use scraper::{ElementRef, Html};

use crate::{selector, text};

/// What `read` gives of each element `css` matches in `html`; an element
/// for which it gives `None` is left out.
fn each<F>(html: &str, css: &str, read: F) -> Result<Vec<String>, String>
where
    F: Fn(ElementRef<'_>) -> Option<String>,
{
    let selector = selector::parse(css)?;
    let document = Html::parse_document(html);
    Ok(document.select(&selector).filter_map(read).collect())
}

/// The text of each match, blank space collapsed.
pub fn texts(html: &str, css: &str) -> Result<Vec<String>, String> {
    each(html, css, |element| Some(text::of(element)))
}

/// The value of the attribute `name` of each match that has it.
pub fn attributes(html: &str, css: &str, name: &str) -> Result<Vec<String>, String> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return Err("an attribute name is needed: `href`, `src`…".to_string());
    }
    each(html, css, |element| element.attr(&name).map(str::to_string))
}

/// The outer HTML of each match: the element, its tags included.
pub fn outer_html(html: &str, css: &str) -> Result<Vec<String>, String> {
    each(html, css, |element| Some(element.html()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<html><head><title>Shop</title></head><body>\
        <h1 class=\"title\">  Our   shop </h1>\
        <ul><li class=\"item\">Tea</li><li class=\"item sale\">Coffee <b>-20%</b></li><li>Water</li></ul>\
        <img src=\"/a.png\" alt=\"A\"><img alt=\"no source\"><img src=\"b.png\">\
        </body></html>";

    #[test]
    fn texts_of_the_matches_in_document_order() {
        assert_eq!(texts(PAGE, "li.item").unwrap(), ["Tea", "Coffee -20%"]);
        assert_eq!(texts(PAGE, "h1.title").unwrap(), ["Our shop"]);
        assert_eq!(texts(PAGE, "title").unwrap(), ["Shop"]);
        assert!(texts(PAGE, "table").unwrap().is_empty());
    }

    #[test]
    fn a_fragment_is_a_document_too() {
        assert_eq!(texts("<p>one</p><p>two</p>", "p").unwrap(), ["one", "two"]);
        assert!(texts("", "p").unwrap().is_empty());
        assert_eq!(texts("not html at all", "body").unwrap(), ["not html at all"]);
    }

    #[test]
    fn attributes_of_the_matches_that_have_them() {
        assert_eq!(attributes(PAGE, "img", "src").unwrap(), ["/a.png", "b.png"]);
        assert_eq!(attributes(PAGE, "img", "SRC").unwrap(), ["/a.png", "b.png"]);
        assert_eq!(attributes(PAGE, "li", "class").unwrap(), ["item", "item sale"]);
        assert!(attributes(PAGE, "img", "title").unwrap().is_empty());
        assert_eq!(attributes(PAGE, "img", " ").unwrap_err(), "an attribute name is needed: `href`, `src`…");
    }

    #[test]
    fn outer_html_of_the_matches() {
        assert_eq!(outer_html(PAGE, "li.sale").unwrap(), ["<li class=\"item sale\">Coffee <b>-20%</b></li>"]);
        assert_eq!(outer_html(PAGE, "img[alt=A]").unwrap(), ["<img src=\"/a.png\" alt=\"A\">"]);
    }

    #[test]
    fn an_invalid_selector_is_an_error() {
        assert!(texts(PAGE, "li[").unwrap_err().starts_with("invalid CSS selector `li[`: "));
        assert!(attributes(PAGE, "::", "src").unwrap_err().starts_with("invalid CSS selector `::`: "));
        assert!(outer_html(PAGE, "").unwrap_err().starts_with("invalid CSS selector ``"));
    }
}
