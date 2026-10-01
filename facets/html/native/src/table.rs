//! The rows of an HTML table: the first `<table>` a selector matches, each
//! of its rows (`<tr>`, in `<thead>`, `<tbody>`, `<tfoot>` or directly in
//! it — not those of a table nested in a cell) as the text of its cells
//! (`<th>` and `<td>`), blank space collapsed. The rows of a `<template>`
//! in the table are inert, as in a browser: not the table's.

use scraper::{ElementRef, Selector};

use crate::{document, selector, text};

/// The rows of the first table `css` matches in `html`: none if nothing
/// matches; an error if what it matches first is not a `<table>`.
pub fn rows(html: &str, css: &str) -> Result<Vec<Vec<String>>, String> {
    let selector = selector::parse(css)?;
    let document = document::parse(html)?;
    let Some(table) = document.select(&selector).next() else { return Ok(Vec::new()) };
    let tag = table.value().name();
    if tag != "table" {
        return Err(format!("`{css}` matches a <{tag}>, not a <table>"));
    }
    let tr = Selector::parse("tr").expect("a valid selector");
    Ok(table.select(&tr).filter(|row| owner(*row).is_some_and(|t| t.id() == table.id())).map(cells).collect())
}

/// The table a row belongs to: its nearest `<table>` ancestor — none if a
/// `<template>` comes first, whose rows are inert.
fn owner(row: ElementRef<'_>) -> Option<ElementRef<'_>> {
    row.ancestors()
        .filter_map(ElementRef::wrap)
        .find(|e| matches!(e.value().name(), "table" | "template"))
        .filter(|e| e.value().name() == "table")
}

/// The text of each cell of a row.
fn cells(row: ElementRef<'_>) -> Vec<String> {
    row.children()
        .filter_map(ElementRef::wrap)
        .filter(|cell| matches!(cell.value().name(), "td" | "th"))
        .map(text::of)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<table id=\"prices\">\
        <thead><tr><th>Item</th><th>Price</th></tr></thead>\
        <tbody><tr><td> Tea </td><td>3.50</td></tr>\
        <tr><td>Coffee <small>(large)</small></td><td>4</td></tr></tbody>\
        <tfoot><tr><td>Total</td><td>7.50</td></tr></tfoot>\
        </table>\
        <table class=\"other\"><tr><td>x</td></tr></table>";

    #[test]
    fn rows_of_a_table() {
        assert_eq!(
            rows(PAGE, "#prices").unwrap(),
            [vec!["Item", "Price"], vec!["Tea", "3.50"], vec!["Coffee (large)", "4"], vec!["Total", "7.50"]]
        );
        assert_eq!(rows(PAGE, "table.other").unwrap(), [vec!["x"]]);
    }

    #[test]
    fn the_first_match_only() {
        assert_eq!(rows(PAGE, "table").unwrap().len(), 4);
    }

    #[test]
    fn a_nested_tables_rows_are_its_own() {
        let html = "<table id=\"outer\"><tr><td>a</td><td><table id=\"inner\"><tr><td>b</td></tr></table></td></tr>\
                    <tr><td>c</td></tr></table>";
        assert_eq!(rows(html, "#outer").unwrap(), [vec!["a", "b"], vec!["c"]]);
        assert_eq!(rows(html, "#inner").unwrap(), [vec!["b"]]);
    }

    #[test]
    fn a_templates_rows_are_not_the_tables() {
        let html = "<table><tr><td>a</td></tr><template><tr><td>T</td></tr></template></table>";
        assert_eq!(rows(html, "table").unwrap(), [vec!["a"]]);
        let html = "<template><table id=\"t\"><tr><td>in</td></tr></table></template>";
        assert_eq!(rows(html, "#t").unwrap(), [vec!["in"]]);
    }

    #[test]
    fn a_cells_line_breaks_separate_its_words() {
        assert_eq!(rows("<table><tr><td>a<br>b</td></tr></table>", "table").unwrap(), [vec!["a b"]]);
    }

    #[test]
    fn a_page_too_deep_is_an_error() {
        let e = rows(&"<table><tr><td>".repeat(1000), "table").unwrap_err();
        assert_eq!(e, "the HTML nests too deeply: more than 512 elements inside one another");
    }

    #[test]
    fn nothing_matched_is_no_rows() {
        assert!(rows(PAGE, "#missing").unwrap().is_empty());
        assert!(rows("<table></table>", "table").unwrap().is_empty());
    }

    #[test]
    fn a_match_that_is_not_a_table_is_an_error() {
        assert_eq!(rows("<div id=\"x\"></div>", "#x").unwrap_err(), "`#x` matches a <div>, not a <table>");
        assert!(rows(PAGE, "table[").unwrap_err().starts_with("invalid CSS selector `table[`: "));
    }
}
