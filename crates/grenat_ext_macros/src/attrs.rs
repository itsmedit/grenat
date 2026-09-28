//! The options of `#[export(…)]`: `effects = "fs.read, net"`, `pure`,
//! `error = "SheetError"`.

use proc_macro2::{Span, TokenStream};
use syn::LitStr;
use syn::meta::ParseNestedMeta;
use syn::parse::Parser;

/// The error type of an `Err`, unless `error = "…"` names another.
pub(crate) const DEFAULT_ERROR: &str = "NativeError";

#[derive(Debug, PartialEq)]
pub(crate) struct Options {
    /// Each effect as Grenat writes it: `fs.read`, `net("api.x.com")`.
    pub effects: Vec<String>,
    /// The result depends on the arguments only: trusted, not tainted.
    pub pure: bool,
    /// The Grenat error an `Err` raises.
    pub error: String,
}

impl Options {
    pub(crate) fn parse(attr: TokenStream) -> syn::Result<Options> {
        let mut options = Options { effects: Vec::new(), pure: false, error: DEFAULT_ERROR.into() };
        let parser = syn::meta::parser(|meta: ParseNestedMeta| {
            if meta.path.is_ident("pure") {
                options.pure = true;
                Ok(())
            } else if meta.path.is_ident("effects") {
                let text: LitStr = meta.value()?.parse()?;
                options.effects = effects(&text)?;
                Ok(())
            } else if meta.path.is_ident("error") {
                let text: LitStr = meta.value()?.parse()?;
                if !is_type_name(&text.value()) || !text.value().ends_with("Error") {
                    return Err(syn::Error::new(
                        text.span(),
                        "an error type is a capitalized name ending in `Error`: `SheetError`",
                    ));
                }
                options.error = text.value();
                Ok(())
            } else {
                Err(meta.error("unknown option: use `effects = \"…\"`, `pure` or `error = \"…\"`"))
            }
        });
        parser.parse2(attr)?;
        if options.pure && !options.effects.is_empty() {
            return Err(syn::Error::new(
                Span::call_site(),
                "a `pure` function has no effects: its result depends on its arguments only",
            ));
        }
        Ok(options)
    }
}

/// `"fs.read, net(\"api.x.com\")"` → each effect, checked for its shape.
fn effects(text: &LitStr) -> syn::Result<Vec<String>> {
    let mut effects = Vec::new();
    for effect in text.value().split(',').map(str::trim).filter(|e| !e.is_empty()) {
        if !is_effect(effect) {
            return Err(syn::Error::new(
                text.span(),
                format!("`{effect}` is not an effect: write them as Grenat does, `fs.read, net(\"api.x.com\")`"),
            ));
        }
        effects.push(effect.to_string());
    }
    Ok(effects)
}

/// `fs.read`, or `net("api.x.com")`: a dotted name and one quoted restriction.
fn is_effect(effect: &str) -> bool {
    let (path, restriction) = match effect.split_once('(') {
        Some((path, rest)) => (path, Some(rest)),
        None => (effect, None),
    };
    let path_ok = !path.is_empty()
        && path.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    let restriction_ok = restriction.is_none_or(|rest| {
        rest.strip_suffix(')')
            .and_then(|quoted| quoted.strip_prefix('"')?.strip_suffix('"'))
            .is_some_and(|inner| !inner.is_empty() && !inner.contains(['"', '\\', '\n']))
    });
    path_ok && restriction_ok
}

pub(crate) fn is_type_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase()) && name.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn options_are_read() {
        let options = Options::parse(quote!(effects = "fs.read, net(\"api.x.com\")", error = "SheetError")).unwrap();
        assert_eq!(options.effects, ["fs.read", "net(\"api.x.com\")"]);
        assert_eq!(options.error, "SheetError");
        assert!(!options.pure);
        let options = Options::parse(quote!(pure)).unwrap();
        assert!(options.pure && options.effects.is_empty());
        assert_eq!(options.error, DEFAULT_ERROR);
        assert_eq!(Options::parse(TokenStream::new()).unwrap().error, DEFAULT_ERROR);
    }

    #[test]
    fn wrong_options_are_errors() {
        let err = |attr| Options::parse(attr).unwrap_err().to_string();
        assert!(err(quote!(pure, effects = "fs.read")).contains("a `pure` function has no effects"));
        assert!(err(quote!(effects = "fs read")).contains("`fs read` is not an effect"));
        assert!(err(quote!(effects = "net(api)")).contains("is not an effect"));
        assert!(err(quote!(error = "oops")).contains("capitalized"));
        assert!(err(quote!(error = "Sheet")).contains("ending in `Error`"));
        assert!(err(quote!(fast)).contains("unknown option"));
    }
}
