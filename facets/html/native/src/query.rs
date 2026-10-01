//! Selecting elements of a document with a CSS selector, and what is read
//! from each match, in document order: its text, an attribute, its HTML.

use scraper::ElementRef;

use crate::{document, selector, text};

/// What `read` gives of each element `css` matches in `html`; an element
/// for which it gives `None` is left out.
fn each<F>(html: &str, css: &str, read: F) -> Result<Vec<String>, String>
where
    F: Fn(ElementRef<'_>) -> Option<String>,
{
    let selector = selector::parse(css)?;
    let document = document::parse(html)?;
    Ok(document.select(&selector).filter_map(read).collect())
}

/// The text of each match, blank space collapsed.
pub fn texts(html: &str, css: &str) -> Result<Vec<String>, String> {
    each(html, css, |element| Some(text::of(element)))
}

/// The value of the attribute `name` of each match that has it.
pub fn attributes(html: &str, css: &str, name: &str) -> Result<Vec<String>, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("an attribute name is needed: `href`, `src`…".to_string());
    }
    each(html, css, |element| attribute(element, name).map(str::to_string))
}

/// The attribute `name` of `element`: named exactly so, or else in another
/// case — the parser lowercases an HTML element's attributes (`SRC` is
/// `src`) but keeps an SVG element's in camel case (`viewBox`).
fn attribute<'a>(element: ElementRef<'a>, name: &str) -> Option<&'a str> {
    let value = element.value();
    value.attr(name).or_else(|| value.attrs().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v))
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
    fn camel_case_svg_attributes_are_read() {
        let svg = "<svg viewBox=\"0 0 10 10\"><linearGradient gradientUnits=\"userSpaceOnUse\"/></svg>";
        assert_eq!(attributes(svg, "svg", "viewBox").unwrap(), ["0 0 10 10"]);
        assert_eq!(attributes(svg, "svg", "viewbox").unwrap(), ["0 0 10 10"]);
        assert_eq!(attributes(svg, "linearGradient", "gradientUnits").unwrap(), ["userSpaceOnUse"]);
        assert!(attributes(svg, "svg", "view").unwrap().is_empty());
    }

    #[test]
    fn the_largest_selectors_on_the_deepest_pages_do_not_overflow_the_stack() {
        let depth = crate::document::MAX_DEPTH - 8;
        let page = format!("{}<p>deep</p>{}", "<div>".repeat(depth), "</div>".repeat(depth));
        let chain = format!("{}p", "div ".repeat((crate::selector::MAX_LENGTH - 1) / 4));
        assert!(texts(&page, &chain).unwrap().is_empty());
        let chain = format!("{}p", "div ".repeat(depth - 2));
        assert_eq!(texts(&page, &chain).unwrap(), ["deep"]);
        let n = crate::selector::MAX_NESTING;
        let not = format!("{}p{}", ":not(".repeat(n), ")".repeat(n));
        assert_eq!(texts(&page, &not).unwrap(), ["deep"]);
        let is = format!("{}p{}", ":is(div ".repeat(n), ")".repeat(n));
        assert_eq!(texts(&page, &is).unwrap(), ["deep"]);
        assert_eq!(texts(&page, "div:has(div div div p)").unwrap().len(), depth - 3);
    }

    #[test]
    fn a_hostile_selector_or_page_is_an_error_not_a_crash() {
        let n = 200_000;
        let selector = format!("{}p{}", ":not(".repeat(n), ")".repeat(n));
        assert!(texts("<p>x</p>", &selector).unwrap_err().ends_with("it is too long (more than 4096 bytes)"));
        let selector = format!("{}p{}", ":not(".repeat(1000), ")".repeat(1000));
        assert!(texts("<p>x</p>", &selector).unwrap_err().starts_with("invalid CSS selector `:not(:not("));
        let deep = "<div>".repeat(100_000);
        let started = std::time::Instant::now();
        assert_eq!(
            texts(&deep, "p").unwrap_err(),
            "the HTML nests too deeply: more than 512 elements inside one another"
        );
        assert!(attributes(&deep, "a", "href").is_err());
        assert!(outer_html(&deep, "a").is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
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
