//! `///` comments, read from `#[doc = "…"]` attributes: the documentation
//! Grenat shows (`##` comments of the generated declarations).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Expr, ExprLit, Lit, Meta};

/// The text of the doc comments, lines joined; `None` without any.
pub(crate) fn doc(attrs: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(nv) if nv.path.is_ident("doc") => match &nv.value {
                Expr::Lit(ExprLit { lit: Lit::Str(text), .. }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
        .map(|line| line.strip_prefix(' ').unwrap_or(&line).trim_end().to_string())
        .collect();
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// `Some("…".to_string())` or `None`, as an expression.
pub(crate) fn option_tokens(doc: Option<String>) -> TokenStream {
    match doc {
        Some(text) => quote!(::core::option::Option::Some(::std::string::String::from(#text))),
        None => quote!(::core::option::Option::None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_comments_are_joined() {
        let item: syn::ItemFn = syn::parse_quote! {
            /// Adds two numbers.
            ///
            /// Never overflows.
            #[inline]
            fn add() {}
        };
        assert_eq!(doc(&item.attrs).as_deref(), Some("Adds two numbers.\n\nNever overflows."));
        let bare: syn::ItemFn = syn::parse_quote!(
            fn f() {}
        );
        assert_eq!(doc(&bare.attrs), None);
    }
}
