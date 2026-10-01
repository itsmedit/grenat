//! The links of a page: the `href` of each `<a>`, resolved against the
//! page's URL — or against its `<base href>`, as a browser does — into an
//! absolute URL, each once, in document order.
//!
//! Only `http:` and `https:` links lead to a page: the others —
//! `javascript:`, `mailto:`, `tel:`, `data:`, `ftp:`… — are left out unless
//! all links are asked for. An `href` that cannot be resolved (a broken URL
//! in the page) is left out; a base URL that is not an absolute URL is an
//! error.
//!
//! The content of a `<template>` is inert, as in a browser: neither its
//! links nor its `<base>` count.

use std::collections::HashSet;

use scraper::{Html, Selector};
use url::Url;

use crate::document;

/// The schemes of links to pages, the only ones kept by default.
const PAGES: &[&str] = &["http", "https"];

/// Which links to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// Links to pages: `http:` and `https:` only.
    Pages,
    /// Every link.
    All,
}

/// The absolute URLs of the links of `html`, a page at `base_url`.
pub fn links(html: &str, base_url: &str, keep: Keep) -> Result<Vec<String>, String> {
    let page = Url::parse(base_url.trim()).map_err(|e| format!("invalid base URL `{base_url}`: {e}"))?;
    if page.cannot_be_a_base() {
        return Err(format!("invalid base URL `{base_url}`: links cannot be resolved against it"));
    }
    let document = document::parse(html)?;
    let base = declared_base(&document, &page).unwrap_or(page);
    let anchors = Selector::parse("a[href]").expect("a valid selector");
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for anchor in document.select(&anchors).filter(|a| !document::is_inert(*a)) {
        let Some(url) = anchor.attr("href").and_then(|href| base.join(href.trim()).ok()) else { continue };
        if keep == Keep::Pages && !PAGES.contains(&url.scheme()) {
            continue;
        }
        let url = String::from(url);
        if seen.insert(url.clone()) {
            out.push(url);
        }
    }
    Ok(out)
}

/// The document's first `<base href>` outside a template, resolved
/// against the page's URL.
fn declared_base(document: &Html, page: &Url) -> Option<Url> {
    let selector = Selector::parse("base[href]").expect("a valid selector");
    let href = document.select(&selector).find(|b| !document::is_inert(*b))?.attr("href")?;
    page.join(href.trim()).ok().filter(|url| !url.cannot_be_a_base())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.com/blog/post.html";

    fn pages(html: &str) -> Vec<String> {
        links(html, BASE, Keep::Pages).unwrap()
    }

    #[test]
    fn relative_links_are_resolved_against_the_page() {
        let html = "<a href=\"/about\">About</a> <a href=\"next.html\">Next</a> <a href=\"../up\">Up</a> \
                    <a href=\"https://other.org/x?y=1#z\">Other</a> <a href=\"//cdn.example.com/f\">CDN</a> \
                    <a href=\"?page=2\">2</a> <a href=\"#top\">Top</a> <a>no href</a>";
        assert_eq!(
            pages(html),
            [
                "https://example.com/about",
                "https://example.com/blog/next.html",
                "https://example.com/up",
                "https://other.org/x?y=1#z",
                "https://cdn.example.com/f",
                "https://example.com/blog/post.html?page=2",
                "https://example.com/blog/post.html#top",
            ]
        );
    }

    #[test]
    fn javascript_and_mailto_are_left_out_unless_asked() {
        let html = "<a href=\"javascript:void(0)\">x</a><a href=\"MAILTO:me@example.com\">m</a><a href=\"/a\">a</a>";
        assert_eq!(pages(html), ["https://example.com/a"]);
        assert_eq!(
            links(html, BASE, Keep::All).unwrap(),
            ["javascript:void(0)", "mailto:me@example.com", "https://example.com/a"]
        );
    }

    #[test]
    fn only_http_and_https_links_lead_to_pages() {
        let html = "<a href=\"data:text/html,x\">x</a><a href=\"tel:1\">t</a><a href=\"vbscript:x\">v</a>\
                    <a href=\"ftp://files.example.com/f\">f</a><a href=\"HTTP://example.com/up\">u</a><a href=\"/a\">a</a>";
        assert_eq!(pages(html), ["http://example.com/up", "https://example.com/a"]);
        assert_eq!(links(html, BASE, Keep::All).unwrap().len(), 6);
    }

    #[test]
    fn a_templates_base_and_links_are_inert() {
        let html =
            "<template><base href=\"https://evil.example/\"><a href=\"/hidden\">h</a></template><a href=\"next\">n</a>";
        assert_eq!(pages(html), ["https://example.com/blog/next"]);
        let html =
            "<template><base href=\"https://evil.example/\"></template><base href=\"/docs/\"><a href=\"intro\">i</a>";
        assert_eq!(pages(html), ["https://example.com/docs/intro"]);
    }

    #[test]
    fn a_page_too_deep_is_an_error() {
        let e = links(&"<div>".repeat(10_000), BASE, Keep::Pages).unwrap_err();
        assert_eq!(e, "the HTML nests too deeply: more than 512 elements inside one another");
    }

    #[test]
    fn each_link_once_in_document_order() {
        let html = "<a href=\"/b\">b</a><a href=\"/a\">a</a><a href=\"https://example.com/b\">b again</a>";
        assert_eq!(pages(html), ["https://example.com/b", "https://example.com/a"]);
    }

    #[test]
    fn a_base_element_changes_where_links_lead() {
        let html = "<head><base href=\"/docs/\"></head><a href=\"intro\">Intro</a>";
        assert_eq!(pages(html), ["https://example.com/docs/intro"]);
    }

    #[test]
    fn broken_hrefs_are_left_out_but_a_broken_base_is_an_error() {
        assert_eq!(pages("<a href=\"http://[bad\">x</a><a href=\" /ok \">ok</a>"), ["https://example.com/ok"]);
        let e = links("<a href=\"/a\">a</a>", "/relative/only", Keep::Pages).unwrap_err();
        assert!(e.starts_with("invalid base URL `/relative/only`: "), "{e}");
        let e = links("", "mailto:x@example.com", Keep::Pages).unwrap_err();
        assert_eq!(e, "invalid base URL `mailto:x@example.com`: links cannot be resolved against it");
    }
}
