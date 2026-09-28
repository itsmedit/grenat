//! `#[derive(GrenatType)]`: a struct with named fields becomes a Grenat
//! `struct` of the same name — its fields' types asked of their Rust types,
//! its `///` comments kept — declared by the manifest of any library whose
//! functions take or return it.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Fields};

use crate::attrs::is_type_name;
use crate::docs::{doc, option_tokens};

pub(crate) fn expand(item: TokenStream) -> TokenStream {
    match try_expand(item) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn try_expand(item: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(item)?;
    let ident = &input.ident;
    let name = ident.unraw().to_string();
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(&input.generics, "a Grenat struct cannot be generic"));
    }
    if !is_type_name(&name) {
        return Err(syn::Error::new_spanned(ident, "a Grenat struct's name is capitalized, letters and digits only"));
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(ident, "`GrenatType` is derived for structs with named fields"));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(ident, "`GrenatType` is derived for structs with named fields"));
    };
    let fields: Vec<_> = fields
        .named
        .iter()
        .map(|f| (f.ident.as_ref().expect("named").unraw().to_string(), &f.ty, doc(&f.attrs)))
        .collect();
    let declared = fields.iter().map(|(fname, ty, fdoc)| {
        let fdoc = option_tokens(fdoc.clone());
        quote!(::grenat_ext::manifest::Field {
            name: ::std::string::String::from(#fname),
            ty: <#ty as ::grenat_ext::GrenatType>::grenat_type(),
            doc: #fdoc,
        })
    });
    let nested = fields.iter().map(|(_, ty, _)| quote!(<#ty as ::grenat_ext::GrenatType>::structs(out);));
    let sdoc = option_tokens(doc(&input.attrs));
    Ok(quote! {
        impl ::grenat_ext::GrenatType for #ident {
            fn grenat_type() -> ::std::string::String {
                ::std::string::String::from(#name)
            }

            fn structs(out: &mut ::std::vec::Vec<::grenat_ext::manifest::Struct>) {
                if out.iter().any(|s| s.name == #name) {
                    return;
                }
                out.push(::grenat_ext::manifest::Struct {
                    name: ::std::string::String::from(#name),
                    doc: #sdoc,
                    fields: ::std::vec![#(#declared),*],
                });
                #(#nested)*
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_struct_declares_its_fields() {
        let out = expand(quote! {
            /// A cell of a sheet.
            struct Cell {
                /// Its row, from 0.
                row: i64,
                text: String,
            }
        })
        .to_string();
        assert!(out.contains("impl :: grenat_ext :: GrenatType for Cell"), "{out}");
        assert!(out.contains("String :: from (\"Cell\")"), "{out}");
        assert!(out.contains("\"A cell of a sheet.\""), "{out}");
        assert!(out.contains("\"Its row, from 0.\""), "{out}");
        assert!(out.contains("< i64 as :: grenat_ext :: GrenatType > :: structs (out)"), "{out}");
    }

    #[test]
    fn what_grenat_cannot_declare_is_refused() {
        for item in [
            quote!(
                struct Pair(i64, i64);
            ),
            quote!(
                enum Color {
                    Red,
                }
            ),
            quote!(
                struct Boxed<T> {
                    item: T,
                }
            ),
            quote!(
                struct snake_case {
                    x: i64,
                }
            ),
        ] {
            let out = expand(item).to_string();
            assert!(out.contains("compile_error"), "{out}");
        }
    }
}
