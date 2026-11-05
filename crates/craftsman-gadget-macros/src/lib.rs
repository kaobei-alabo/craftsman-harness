//! `#[gadget_call]`: the attribute that makes a function callable through a
//! gadget's standard entry point (`craftsman:gadget/guest.call`).
//!
//! ```ignore
//! /// Counts the words in a text.
//! #[gadget_call]
//! fn count_words(args: CountWords) -> Result<Count, GadgetError> { ... }
//! ```
//!
//! The function keeps its body and signature. Beside it the attribute emits a
//! dispatcher (parse the JSON arguments, call, serialize the result) and
//! submits `{name, description, input schema, dispatcher}` to the
//! `craftsman_gadget` registry, which `describe` and `call` read. The function
//! takes zero arguments or one (`Deserialize + JsonSchema`) and returns
//! `Result<T, GadgetError>` with `T: Serialize`. The description is the doc
//! comment unless `description = "..."` is given; the name is the function's
//! unless `name = "..."` is given.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Expr, ExprLit, FnArg, ItemFn, Lit, LitStr, Meta, MetaNameValue, Token, parse_macro_input,
};

struct Options {
    name: Option<LitStr>,
    description: Option<LitStr>,
}

impl Parse for Options {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut options = Options {
            name: None,
            description: None,
        };
        let pairs = Punctuated::<MetaNameValue, Token![,]>::parse_terminated(input)?;
        for pair in pairs {
            let Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) = &pair.value
            else {
                return Err(syn::Error::new_spanned(&pair.value, "expected a string"));
            };
            let slot = if pair.path.is_ident("name") {
                &mut options.name
            } else if pair.path.is_ident("description") {
                &mut options.description
            } else {
                return Err(syn::Error::new_spanned(
                    &pair.path,
                    "unknown option: expected `name` or `description`",
                ));
            };
            if slot.is_some() {
                return Err(syn::Error::new_spanned(&pair.path, "option given twice"));
            }
            *slot = Some(value.clone());
        }
        Ok(options)
    }
}

/// The doc comment of `function`, one line per `///` line, trimmed.
fn doc_comment(function: &ItemFn) -> String {
    let lines: Vec<String> = function
        .attrs
        .iter()
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(MetaNameValue {
                path,
                value:
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(text),
                        ..
                    }),
                ..
            }) if path.is_ident("doc") => Some(text.value().trim().to_string()),
            _ => None,
        })
        .collect();
    lines.join("\n").trim().to_string()
}

/// A function name the host accepts: what `craftsman_gadget` checks at run time,
/// caught here at compile time.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[proc_macro_attribute]
pub fn gadget_call(attr: TokenStream, item: TokenStream) -> TokenStream {
    let options = parse_macro_input!(attr as Options);
    let function = parse_macro_input!(item as ItemFn);
    match expand(options, function) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(options: Options, function: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let sig = &function.sig;
    if let Some(asyncness) = &sig.asyncness {
        return Err(syn::Error::new_spanned(
            asyncness,
            "a gadget function is synchronous: remove `async`",
        ));
    }
    if sig.unsafety.is_some() || sig.abi.is_some() || sig.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            sig,
            "a gadget function must be a safe Rust function",
        ));
    }
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &sig.generics,
            "a gadget function cannot be generic",
        ));
    }
    if matches!(sig.output, syn::ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            sig,
            "a gadget function returns Result<T, GadgetError>",
        ));
    }
    let ident = &sig.ident;
    let name = match &options.name {
        Some(name) => name.value(),
        None => ident.to_string(),
    };
    if !valid_name(&name) {
        let span = options
            .name
            .as_ref()
            .map(LitStr::span)
            .unwrap_or_else(|| ident.span());
        return Err(syn::Error::new(
            span,
            "a gadget function name is lowercase ASCII letters, digits and `_`, \
             starts with a letter and has at most 64 characters",
        ));
    }
    let description = match &options.description {
        Some(text) => text.value(),
        None => doc_comment(&function),
    };
    if description.trim().is_empty() {
        return Err(syn::Error::new_spanned(
            ident,
            "describe the function: add a `///` doc comment or `description = \"...\"`; \
             the agent reads it to decide when to call the function",
        ));
    }

    let inputs: Vec<&FnArg> = sig.inputs.iter().collect();
    let (call, schema) = match inputs.as_slice() {
        [] => (
            quote! {
                ::craftsman_gadget::__private::parse_no_args(args)?;
                ::craftsman_gadget::__private::encode(#ident())
            },
            quote! { ::craftsman_gadget::__private::no_args_schema() },
        ),
        [FnArg::Typed(arg)] => {
            let ty = &arg.ty;
            (
                quote! {
                    let parsed: #ty = ::craftsman_gadget::__private::parse_args(args)?;
                    ::craftsman_gadget::__private::encode(#ident(parsed))
                },
                quote! { ::craftsman_gadget::__private::schema_of::<#ty>() },
            )
        }
        [FnArg::Receiver(receiver), ..] => {
            return Err(syn::Error::new_spanned(
                receiver,
                "a gadget function is a free function, not a method",
            ));
        }
        [_, extra, ..] => {
            return Err(syn::Error::new_spanned(
                extra,
                "a gadget function takes at most one argument: put the fields in one struct",
            ));
        }
    };

    let name = LitStr::new(&name, Span::call_site());
    let description = LitStr::new(&description, Span::call_site());
    Ok(quote! {
        #function

        const _: () = {
            fn dispatch(
                args: &str,
            ) -> ::core::result::Result<
                ::std::string::String,
                ::craftsman_gadget::__private::CallError,
            > {
                #call
            }

            fn input_schema() -> ::craftsman_gadget::__private::serde_json::Value {
                #schema
            }

            ::craftsman_gadget::__private::inventory::submit! {
                ::craftsman_gadget::__private::Function {
                    name: #name,
                    description: #description,
                    input_schema,
                    dispatch,
                }
            }
        };
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expansion(options: &str, function: &str) -> syn::Result<proc_macro2::TokenStream> {
        expand(syn::parse_str(options)?, syn::parse_str(function)?)
    }

    #[test]
    fn registers_documented_zero_and_one_argument_functions() {
        for function in [
            "/// A ping.\nfn ping() -> Result<bool, GadgetError> { Ok(true) }",
            "/// Count.\nfn count(args: Input) -> Result<Output, GadgetError> { todo!() }",
        ] {
            let tokens = expansion("", function).unwrap().to_string();
            assert!(tokens.contains("inventory :: submit"));
            assert!(tokens.contains("fn dispatch"));
            assert!(tokens.contains("fn input_schema"));
        }
    }

    #[test]
    fn rejects_invalid_options_and_signatures() {
        let documented = "/// A ping.\nfn ping() -> Result<bool, GadgetError> { Ok(true) }";
        for options in [
            "unknown = \"x\"",
            "name = 42",
            "name = \"a\", name = \"b\"",
            "name = \"UPPER\"",
            "name = \"with-hyphen\"",
            "name = \"9bad\"",
            "description = \" \"",
        ] {
            assert!(expansion(options, documented).is_err(), "{options}");
        }
        for function in [
            "fn ping() -> Result<(), GadgetError> { Ok(()) }",
            "/// A ping.\nasync fn ping() -> Result<(), GadgetError> { Ok(()) }",
            "/// A ping.\nunsafe fn ping() -> Result<(), GadgetError> { Ok(()) }",
            "/// A ping.\nfn ping<T>(args: T) -> Result<(), GadgetError> { Ok(()) }",
            "/// A ping.\nfn ping() {}",
            "/// A ping.\nfn ping(a: A, b: B) -> Result<(), GadgetError> { Ok(()) }",
            "/// A ping.\nfn ping(&self) -> Result<(), GadgetError> { Ok(()) }",
        ] {
            assert!(expansion("", function).is_err(), "{function}");
        }
    }

    #[test]
    fn doc_comments_and_name_boundaries() {
        let function: ItemFn = syn::parse_str("/// First.\n///  Second.\nfn ping() {} ").unwrap();
        assert_eq!(doc_comment(&function), "First.\nSecond.");
        for name in ["a", "abc_123", &"a".repeat(64)] {
            assert!(valid_name(name));
        }
        for name in ["", "_a", "A", "a-b", "你好", &"a".repeat(65)] {
            assert!(!valid_name(name));
        }
    }
}
