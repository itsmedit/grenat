//! `#[export]`: the function stays as written; next to it come its C entry
//! point (`grenat_ext_v1_<name>`: JSON arguments in, JSON result out, panics
//! caught by `grenat_ext`) and its description, registered for the
//! library's manifest.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ItemFn;

use crate::attrs::Options;
use crate::docs::{doc, option_tokens};
use crate::signature::{Returns, Signature};

/// The prefix of every entry point: the ABI's version is in the symbol
/// (`grenat_ext::abi::ENTRY_PREFIX`, which the tests compare).
pub(crate) const ENTRY_PREFIX: &str = "grenat_ext_v1_";

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    match try_expand(attr, item) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn try_expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let options = Options::parse(attr)?;
    let function: ItemFn = syn::parse2(item)?;
    let sig = Signature::read(&function)?;
    let name = &sig.name;
    let symbol = format!("{ENTRY_PREFIX}{name}");
    let entry = format_ident!("{symbol}");
    let describe = format_ident!("__grenat_describe_{name}");
    let structs = format_ident!("__grenat_structs_{name}");
    let ident = &sig.ident;

    // the entry point: arguments decoded in order, the function called
    let count = sig.params.len();
    let locals: Vec<_> = (0..count).map(|i| format_ident!("__grenat_{i}")).collect();
    let decode = sig.params.iter().zip(&locals).map(|(p, local)| {
        let (ty, pname) = (&p.ty, &p.name);
        quote!(let #local: #ty = __grenat_args.next(#pname)?;)
    });
    let call = quote!(#ident(#(#locals),*));
    let error = &options.error;
    let returned = match &sig.returns {
        Returns::Unit => quote!({ #call; ::grenat_ext::__private::returned(()) }),
        Returns::Plain(_) => quote!(::grenat_ext::__private::returned(#call)),
        Returns::Result(_) => quote!(match #call {
            ::core::result::Result::Ok(value) => ::grenat_ext::__private::returned(value),
            ::core::result::Result::Err(error) => {
                ::core::result::Result::Err(::grenat_ext::__private::raised(error, #error))
            }
        }),
    };

    // the description: Grenat types asked of each Rust type
    let params = sig.params.iter().map(|p| {
        let (ty, pname) = (&p.ty, &p.name);
        quote!(::grenat_ext::manifest::Field {
            name: ::std::string::String::from(#pname),
            ty: <#ty as ::grenat_ext::GrenatType>::grenat_type(),
            doc: ::core::option::Option::None,
        })
    });
    let result_ty = match &sig.returns {
        Returns::Unit => None,
        Returns::Plain(ty) | Returns::Result(ty) => Some(ty),
    };
    let returns = match result_ty {
        Some(ty) => quote!(<#ty as ::grenat_ext::GrenatType>::grenat_type()),
        None => quote!(::std::string::String::from("Nil")),
    };
    let mentioned = sig
        .params
        .iter()
        .map(|p| &p.ty)
        .chain(result_ty)
        .map(|ty| quote!(<#ty as ::grenat_ext::GrenatType>::structs(out);));
    let doc = option_tokens(doc(&function.attrs));
    let effects = &options.effects;
    let pure = options.pure;

    Ok(quote! {
        #function

        /// The C entry point of the function of the same name, for Grenat.
        ///
        /// # Safety
        /// `args` points to `len` readable bytes and `out` to a writable
        /// buffer: the ABI's contract, which Grenat keeps.
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        #[allow(clippy::missing_safety_doc)]
        pub unsafe extern "C" fn #entry(
            args: *const u8,
            len: usize,
            out: *mut ::grenat_ext::abi::Buffer,
        ) -> i32 {
            // SAFETY: the caller's contract, stated above
            unsafe {
                ::grenat_ext::__private::entry(args, len, out, |bytes| {
                    let mut __grenat_args = ::grenat_ext::__private::Args::parse(bytes, #count, #name)?;
                    #(#decode)*
                    #returned
                })
            }
        }

        #[doc(hidden)]
        fn #describe() -> ::grenat_ext::manifest::Function {
            ::grenat_ext::manifest::Function {
                name: ::std::string::String::from(#name),
                symbol: ::std::string::String::from(#symbol),
                doc: #doc,
                params: ::std::vec![#(#params),*],
                returns: #returns,
                effects: ::std::vec![#(::std::string::String::from(#effects)),*],
                pure: #pure,
                error: ::std::string::String::from(#error),
            }
        }

        #[doc(hidden)]
        fn #structs(out: &mut ::std::vec::Vec<::grenat_ext::manifest::Struct>) {
            #(#mentioned)*
        }

        ::grenat_ext::__private::inventory::submit! {
            ::grenat_ext::__private::Export { describe: #describe, structs: #structs }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expanded(attr: TokenStream, item: TokenStream) -> String {
        expand(attr, item).to_string()
    }

    #[test]
    fn an_entry_point_and_a_description_are_generated() {
        let out = expanded(
            quote!(effects = "fs.read", error = "SheetError"),
            quote! {
                /// Reads a sheet.
                pub fn read_sheet(path: String) -> Result<Vec<String>, std::io::Error> { todo!() }
            },
        );
        assert!(out.contains("pub fn read_sheet"), "the function is kept: {out}");
        assert!(out.contains("pub unsafe extern \"C\" fn grenat_ext_v1_read_sheet"), "{out}");
        assert!(out.contains("# [unsafe (no_mangle)]"), "{out}");
        assert!(out.contains("Args :: parse (bytes , 1usize , \"read_sheet\")"), "{out}");
        assert!(out.contains("__grenat_args . next (\"path\")"), "{out}");
        assert!(out.contains("raised (error , \"SheetError\")"), "{out}");
        assert!(out.contains("\"Reads a sheet.\""), "{out}");
        assert!(out.contains("\"fs.read\""), "{out}");
        assert!(out.contains("< Vec < String > as :: grenat_ext :: GrenatType > :: grenat_type ()"), "{out}");
        assert!(out.contains("inventory :: submit !"), "{out}");
    }

    #[test]
    fn a_unit_function_returns_nil() {
        let out = expanded(
            quote!(pure),
            quote!(
                fn tick(n: i64) {}
            ),
        );
        assert!(out.contains("tick (__grenat_0) ; :: grenat_ext :: __private :: returned (())"), "{out}");
        assert!(out.contains("String :: from (\"Nil\")"), "{out}");
        assert!(out.contains("pure : true"), "{out}");
    }

    #[test]
    fn errors_become_compile_errors() {
        let out = expanded(
            quote!(),
            quote!(
                fn f(s: &str) {}
            ),
        );
        assert!(out.contains("compile_error"), "{out}");
        assert!(out.contains("owned values"), "{out}");
        let out = expanded(
            quote!(pure, effects = "net"),
            quote!(
                fn f() {}
            ),
        );
        assert!(out.contains("compile_error") && out.contains("no effects"), "{out}");
        let out = expanded(
            quote!(),
            quote!(
                struct NotAFunction;
            ),
        );
        assert!(out.contains("compile_error"), "{out}");
    }
}
