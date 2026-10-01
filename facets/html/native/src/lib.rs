//! The native part of Grenat's official facet `html`: CSS selectors on
//! HTML, with the `scraper` crate (the parser of the Servo browser engine).
//!
//! Every function is `pure`: no effect, its result depends on its arguments
//! only, so it is as trusted as the HTML it is given — the text of a page
//! fetched with `Http` stays untrusted. A selector that does not parse, a
//! base URL that is not one, raise an `HtmlError` naming them; so do a
//! selector too long or nested too deeply, a page nested too deeply (see
//! `selector` and `document`): bounds that keep a hostile selector or page
//! from overflowing the stack or running for minutes.

mod document;
mod links;
mod query;
mod selector;
mod table;
mod text;

use grenat_ext::export;

use crate::links::Keep;

/// The text of each element `selector` matches in `html`, in document
/// order, blank space collapsed.
#[export(pure, error = "HtmlError")]
pub fn select(html: String, selector: String) -> Result<Vec<String>, String> {
    query::texts(&html, &selector)
}

/// The value of the attribute `attribute` (`href`, `src`…) of each element
/// `selector` matches in `html` that has it, in document order.
#[export(pure, error = "HtmlError")]
pub fn select_attr(html: String, selector: String, attribute: String) -> Result<Vec<String>, String> {
    query::attributes(&html, &selector, &attribute)
}

/// The outer HTML of each element `selector` matches in `html`, in
/// document order.
#[export(pure, error = "HtmlError")]
pub fn select_html(html: String, selector: String) -> Result<Vec<String>, String> {
    query::outer_html(&html, &selector)
}

/// The absolute URLs of the links (`<a href>`) of `html`, a page at
/// `base_url`, each once, in document order: links to pages only, `http:`
/// and `https:` (no `javascript:`, `mailto:`, `tel:`, `data:`…).
#[export(pure, error = "HtmlError")]
pub fn links(html: String, base_url: String) -> Result<Vec<String>, String> {
    links::links(&html, &base_url, Keep::Pages)
}

/// The absolute URLs of all the links (`<a href>`) of `html`, a page at
/// `base_url`, each once, in document order, whatever their scheme
/// (`javascript:`, `mailto:`, `tel:`… included).
#[export(pure, error = "HtmlError")]
pub fn all_links(html: String, base_url: String) -> Result<Vec<String>, String> {
    links::links(&html, &base_url, Keep::All)
}

/// The rows of the first table `selector` matches in `html`, each as the
/// text of its cells (`<th>`, `<td>`); no rows if nothing matches.
#[export(pure, error = "HtmlError")]
pub fn table(html: String, selector: String) -> Result<Vec<Vec<String>>, String> {
    table::rows(&html, &selector)
}

#[cfg(test)]
mod tests {
    use grenat_ext::manifest;

    #[test]
    fn the_manifest_declares_pure_functions_raising_html_errors() {
        let manifest = manifest();
        let mut names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["all_links", "links", "select", "select_attr", "select_html", "table"]);
        for function in &manifest.functions {
            assert!(function.pure, "{}", function.name);
            assert!(function.effects.is_empty(), "{}", function.name);
            assert_eq!(function.error, "HtmlError", "{}", function.name);
            assert!(!function.doc.as_deref().unwrap_or("").is_empty(), "{}", function.name);
        }
        let table = manifest.functions.iter().find(|f| f.name == "table").unwrap();
        assert_eq!(table.returns, "Array(Array(String))");
    }
}
