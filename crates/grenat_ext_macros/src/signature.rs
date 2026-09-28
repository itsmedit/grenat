//! What an exported function takes and returns, and what it may not be:
//! generic, async, a method, or taking references (arguments arrive as
//! JSON, decoded into owned values).

use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{FnArg, GenericArgument, Ident, ItemFn, Pat, PathArguments, ReturnType, Type};

use crate::grenat::check_function_name;

/// A parameter: its name (as Grenat sees it) and Rust type.
pub(crate) struct Param {
    pub name: String,
    pub ty: Type,
}

/// What the function returns.
pub(crate) enum Returns {
    /// Nothing: Grenat gets `nil`.
    Unit,
    Plain(Type),
    /// `Result<T, E>`: `T`, or the error `E` (displayed) raised in Grenat.
    Result(Type),
}

pub(crate) struct Signature {
    /// The function's name, without `r#`.
    pub name: String,
    pub ident: Ident,
    pub params: Vec<Param>,
    pub returns: Returns,
}

impl Signature {
    pub(crate) fn read(item: &ItemFn) -> syn::Result<Signature> {
        let sig = &item.sig;
        let refuse = |span: proc_macro2::Span, why: &str| Err(syn::Error::new(span, why.to_string()));
        if let Some(asyncness) = &sig.asyncness {
            return refuse(asyncness.span(), "an exported function cannot be `async`");
        }
        if let Some(unsafety) = &sig.unsafety {
            return refuse(unsafety.span(), "an exported function cannot be `unsafe`");
        }
        if let Some(abi) = &sig.abi {
            return refuse(abi.span(), "an exported function is a plain Rust function: the macro writes its C entry");
        }
        if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
            return refuse(sig.generics.span(), "an exported function cannot be generic");
        }
        if let Some(variadic) = &sig.variadic {
            return refuse(variadic.span(), "an exported function cannot be variadic");
        }
        let mut params = Vec::new();
        for input in &sig.inputs {
            let FnArg::Typed(typed) = input else {
                return refuse(input.span(), "an exported function is not a method: it takes no `self`");
            };
            let Pat::Ident(pat) = typed.pat.as_ref() else {
                return refuse(typed.pat.span(), "an exported function's parameters are plain names");
            };
            if pat.by_ref.is_some() || pat.subpat.is_some() {
                return refuse(typed.pat.span(), "an exported function's parameters are plain names");
            }
            owned(&typed.ty)?;
            params.push(Param { name: pat.ident.unraw().to_string(), ty: (*typed.ty).clone() });
        }
        let returns = match &sig.output {
            ReturnType::Default => Returns::Unit,
            ReturnType::Type(_, ty) => {
                owned(ty)?;
                match result_ok(ty) {
                    Some(ok) => Returns::Result(ok),
                    None if matches!(ty.as_ref(), Type::Tuple(t) if t.elems.is_empty()) => Returns::Unit,
                    None => Returns::Plain((**ty).clone()),
                }
            }
        };
        let name = sig.ident.unraw().to_string();
        check_function_name(&name).map_err(|why| syn::Error::new(sig.ident.span(), why))?;
        Ok(Signature { name, ident: sig.ident.clone(), params, returns })
    }
}

/// Refuses references and `impl Trait` anywhere in `ty`.
fn owned(ty: &Type) -> syn::Result<()> {
    match ty {
        Type::Reference(r) => Err(syn::Error::new(
            r.span(),
            "an exported function takes and returns owned values: `String`, not `&str`; `Vec<T>`, not `&[T]`",
        )),
        Type::ImplTrait(t) => Err(syn::Error::new(t.span(), "an exported function names its types: no `impl Trait`")),
        Type::Path(path) => {
            for segment in &path.path.segments {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    for arg in &args.args {
                        if let GenericArgument::Type(inner) = arg {
                            owned(inner)?;
                        }
                    }
                }
            }
            Ok(())
        }
        Type::Paren(p) => owned(&p.elem),
        Type::Group(g) => owned(&g.elem),
        _ => Ok(()),
    }
}

/// `T` of `Result<T, E>` (or of a `Result<T>` alias such as `io::Result<T>`).
fn result_ok(ty: &Type) -> Option<Type> {
    let Type::Path(path) = ty else { return None };
    let last = path.path.segments.last()?;
    if last.ident != "Result" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else { return None };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(ok) => Some(ok.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;

    fn read(item: ItemFn) -> syn::Result<Signature> {
        Signature::read(&item)
    }

    #[test]
    fn parameters_and_results_are_read() {
        let sig = read(syn::parse_quote!(
            fn r#type(mut a: i64, b: Vec<String>) -> std::io::Result<Vec<i64>> {}
        ))
        .unwrap();
        assert_eq!(sig.name, "type");
        let names: Vec<&str> = sig.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(matches!(&sig.returns, Returns::Result(ok) if ok.to_token_stream().to_string() == "Vec < i64 >"));
        let unit = read(syn::parse_quote!(
            fn f() -> () {}
        ))
        .unwrap();
        assert!(matches!(unit.returns, Returns::Unit));
        let plain = read(syn::parse_quote!(
            fn f() -> Option<String> {}
        ))
        .unwrap();
        assert!(matches!(plain.returns, Returns::Plain(_)));
    }

    #[test]
    fn a_parameter_may_be_named_as_a_keyword() {
        // a label in Grenat (`f(next: 1)`), unlike a function's name
        let sig = read(syn::parse_quote!(
            fn f(next: i64, end: i64) {}
        ))
        .unwrap();
        assert_eq!(sig.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["next", "end"]);
    }

    #[test]
    fn what_cannot_cross_the_boundary_is_refused() {
        let err = |item: ItemFn| read(item).err().expect("refused").to_string();
        // Grenat's keywords are legal Rust names
        assert_eq!(
            err(syn::parse_quote!(
                fn begin() {}
            )),
            "`begin` is a Grenat keyword: Grenat code could not name it"
        );

        assert!(
            err(syn::parse_quote!(
                fn f(a: &str) {}
            ))
            .contains("owned values")
        );
        assert!(
            err(syn::parse_quote!(
                fn f(a: Vec<&str>) {}
            ))
            .contains("owned values")
        );
        assert!(
            err(syn::parse_quote!(
                fn f<T>(a: T) {}
            ))
            .contains("generic")
        );
        assert!(
            err(syn::parse_quote!(
                async fn f() {}
            ))
            .contains("async")
        );
        assert!(
            err(syn::parse_quote!(
                fn f(&self) {}
            ))
            .contains("no `self`")
        );
        assert!(
            err(syn::parse_quote!(
                fn f((a, b): (i64, i64)) {}
            ))
            .contains("plain names")
        );
        assert!(
            err(syn::parse_quote!(
                extern "C" fn f() {}
            ))
            .contains("C entry")
        );
        assert!(
            err(syn::parse_quote!(
                fn f() -> impl Iterator<Item = i64> {}
            ))
            .contains("impl Trait")
        );
    }
}
