//! The text of an element as a reader sees it: its text nodes joined,
//! entities decoded (by the parser), every run of blank space one space,
//! none at either end.

use scraper::ElementRef;

/// The text of `element` and its descendants, blank space collapsed.
pub fn of(element: ElementRef<'_>) -> String {
    collapse(&element.text().collect::<String>())
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

    #[test]
    fn an_elements_text_includes_its_descendants() {
        let page = Html::parse_document("<p>Hello <b>big</b>\n  &amp; <i>wide</i> world</p>");
        let p = page.select(&Selector::parse("p").unwrap()).next().unwrap();
        assert_eq!(of(p), "Hello big & wide world");
    }
}
