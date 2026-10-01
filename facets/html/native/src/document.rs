//! Parsing a page into a document, as a browser parses it — within bounds.
//!
//! An HTML parser keeps a stack of the elements still open, and looks
//! through it at nearly every tag: a page whose elements nest deeper and
//! deeper (`<div>` a hundred thousand times, a few hundred kilobytes) takes
//! a time that grows with the square of its depth, minutes of native code
//! no Grenat timeout can stop. So the page is parsed a chunk at a time, the
//! depth of every node the parser places is measured, and a page nesting
//! more than `MAX_DEPTH` elements inside one another is refused once the
//! chunk that goes past it is parsed: an error, in a time that grows with
//! the size of the page only. Browsers stop nesting at the same depth (512
//! in Chromium), so no page a browser shows faithfully is refused.
//!
//! Also here: whether an element is part of a `<template>`'s content, which
//! a browser parses but neither shows nor follows.

use std::borrow::Cow;
use std::cell::{Cell, Ref};

use html5ever::driver::{self, ParseOpts};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{Attribute, QualName};
use scraper::{ElementRef, Html, HtmlTreeSink};

/// How many elements may nest inside one another.
pub const MAX_DEPTH: usize = 512;

/// How much of the page is parsed before its depth is looked at again: a
/// page going past `MAX_DEPTH` is parsed at most this much further.
const CHUNK: usize = 4096;

/// `html` parsed as a browser parses a page, or why it is refused.
pub fn parse(html: &str) -> Result<Html, String> {
    let mut parser = driver::parse_document(Bounded::new(), ParseOpts::default());
    for chunk in chunks(html, CHUNK) {
        parser.process(StrTendril::from_slice(chunk));
        if parser.tokenizer.sink.sink.too_deep() {
            return Err(too_deep());
        }
    }
    parser.finish()
}

fn too_deep() -> String {
    format!("the HTML nests too deeply: more than {MAX_DEPTH} elements inside one another")
}

/// Whether `element` is in the content of a `<template>`: inert, neither
/// shown nor followed by a browser.
pub fn is_inert(element: ElementRef<'_>) -> bool {
    element.ancestors().filter_map(ElementRef::wrap).any(|e| e.value().name() == "template")
}

/// `text` cut into pieces of about `size` bytes, each ending on a
/// character's boundary.
fn chunks(text: &str, size: usize) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut end = size.min(rest.len());
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (chunk, tail) = rest.split_at(end);
        rest = tail;
        Some(chunk)
    })
}

/// scraper's tree sink, measuring the depth of every node it is given a
/// place for.
struct Bounded {
    inner: HtmlTreeSink,
    deepest: Cell<usize>,
}

impl Bounded {
    fn new() -> Self {
        Self { inner: HtmlTreeSink::new(Html::new_document()), deepest: Cell::new(0) }
    }

    fn too_deep(&self) -> bool {
        self.deepest.get() > MAX_DEPTH
    }

    /// A node placed under `parent`: the depth it is at noted.
    fn placed_under(&self, parent: &<Self as TreeSink>::Handle) {
        let html = self.inner.0.borrow();
        let depth = html.tree.get(*parent).map_or(0, |node| node.ancestors().count() + 1);
        self.deepest.set(self.deepest.get().max(depth));
    }

    /// A node placed next to `sibling`: as deep as it.
    fn placed_beside(&self, sibling: &<Self as TreeSink>::Handle) {
        let html = self.inner.0.borrow();
        let depth = html.tree.get(*sibling).map_or(0, |node| node.ancestors().count());
        self.deepest.set(self.deepest.get().max(depth));
    }
}

/// Every method is scraper's own; placing a node measures it too.
impl TreeSink for Bounded {
    type Output = Result<Html, String>;
    type Handle = <HtmlTreeSink as TreeSink>::Handle;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Self::Output {
        if self.too_deep() { Err(too_deep()) } else { Ok(self.inner.finish()) }
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        self.inner.parse_error(msg);
    }

    fn get_document(&self) -> Self::Handle {
        self.inner.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        self.inner.elem_name(target)
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Self::Handle {
        self.inner.create_element(name, attrs, flags)
    }

    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        self.inner.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        self.inner.create_pi(target, data)
    }

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        self.inner.append(parent, child);
        self.placed_under(parent);
    }

    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        prev_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        self.inner.append_based_on_parent_node(element, prev_element, child);
        self.placed_beside(element);
        self.placed_under(prev_element);
    }

    fn append_doctype_to_document(&self, name: StrTendril, public_id: StrTendril, system_id: StrTendril) {
        self.inner.append_doctype_to_document(name, public_id, system_id);
    }

    fn mark_script_already_started(&self, node: &Self::Handle) {
        self.inner.mark_script_already_started(node);
    }

    fn pop(&self, node: &Self::Handle) {
        self.inner.pop(node);
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        self.inner.get_template_contents(target)
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        self.inner.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.inner.set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &Self::Handle, new_node: NodeOrText<Self::Handle>) {
        self.inner.append_before_sibling(sibling, new_node);
        self.placed_beside(sibling);
    }

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        self.inner.add_attrs_if_missing(target, attrs);
    }

    fn associate_with_form(
        &self,
        target: &Self::Handle,
        form: &Self::Handle,
        nodes: (&Self::Handle, Option<&Self::Handle>),
    ) {
        self.inner.associate_with_form(target, form, nodes);
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        self.inner.remove_from_parent(target);
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        self.inner.reparent_children(node, new_parent);
        self.placed_under(new_parent);
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Self::Handle) -> bool {
        self.inner.is_mathml_annotation_xml_integration_point(handle)
    }

    fn set_current_line(&self, line_number: u64) {
        self.inner.set_current_line(line_number);
    }

    fn allow_declarative_shadow_roots(&self, intended_parent: &Self::Handle) -> bool {
        self.inner.allow_declarative_shadow_roots(intended_parent)
    }

    fn attach_declarative_shadow(&self, location: &Self::Handle, template: &Self::Handle, attrs: &[Attribute]) -> bool {
        self.inner.attach_declarative_shadow(location, template, attrs)
    }

    fn maybe_clone_an_option_into_selectedcontent(&self, option: &Self::Handle) {
        self.inner.maybe_clone_an_option_into_selectedcontent(option);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use scraper::Selector;

    use super::*;

    fn nested(depth: usize) -> String {
        format!("{}<p>deep</p>{}", "<div>".repeat(depth), "</div>".repeat(depth))
    }

    #[test]
    fn a_page_parses_as_scraper_parses_it() {
        let html = "<!doctype html><title>T</title><table><tr><td>a<b>b<i>c</b>d</i></table><template><p>t</p></template>\
                    <svg viewBox=\"0 0 1 1\"><path/></svg><p>caf&eacute;";
        assert_eq!(parse(html).unwrap().html(), Html::parse_document(html).html());
        assert_eq!(parse("").unwrap().html(), Html::parse_document("").html());
    }

    #[test]
    fn a_page_as_deep_as_a_browser_allows_parses() {
        let page = parse(&nested(MAX_DEPTH - 10)).unwrap();
        let p = Selector::parse("p").unwrap();
        assert_eq!(page.select(&p).count(), 1);
    }

    #[test]
    fn a_deeper_page_is_refused_quickly() {
        let started = Instant::now();
        for depth in [MAX_DEPTH + 10, 100_000] {
            let e = parse(&nested(depth)).unwrap_err();
            assert_eq!(e, "the HTML nests too deeply: more than 512 elements inside one another");
        }
        // unclosed formatting elements and table cells nest too
        assert!(parse(&"<b><i>".repeat(50_000)).is_err());
        assert!(parse(&"<table><tr><td>".repeat(50_000)).is_err());
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    }

    #[test]
    fn a_wide_page_is_not_deep() {
        let html = "<p>para".repeat(20_000) + &"<li>item".repeat(20_000) + &"<div>x</div>".repeat(20_000);
        let page = parse(&html).unwrap();
        assert_eq!(page.select(&Selector::parse("div").unwrap()).count(), 20_000);
    }

    #[test]
    fn chunks_end_on_character_boundaries() {
        let text = "é".repeat(5);
        let pieces: Vec<&str> = chunks(&text, 3).collect();
        assert_eq!(pieces.concat(), text);
        assert!(pieces.iter().all(|p| !p.is_empty() && p.len() <= 4));
        assert_eq!(chunks("", 3).count(), 0);
    }

    #[test]
    fn template_content_is_inert() {
        let page = Html::parse_document("<template><a id=in>x</a></template><a id=out>y</a>");
        let a = Selector::parse("a").unwrap();
        let inert: Vec<bool> = page.select(&a).map(is_inert).collect();
        assert_eq!(inert, [true, false]);
    }
}
