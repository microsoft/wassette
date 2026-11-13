use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{ItemFn, parse_macro_input, spanned::Spanned};

#[proc_macro_attribute]
pub fn attr_macro_main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    if input.sig.asyncness.is_none() {
        return quote_spanned! { input.sig.fn_token.span()=>
            compile_error!("fn must be `async fn`");
        }
        .into();
    }

    if input.sig.ident != "main" {
        return quote_spanned! { input.sig.ident.span()=>
            compile_error!("only `async fn main` can be used for #[wstd::main]");
        }
        .into();
    }

    if !input.sig.inputs.is_empty() {
        return quote_spanned! { input.sig.inputs.span()=>
            compile_error!("arguments to main are not supported");
        }
        .into();
    }
    let attrs = input.attrs;
    let output = input.sig.output;
    let block = input.block;
    quote! {
        ::wstd::__main_export! {
            output { #output }
            run { #(#attrs)* async fn __run() #output #block }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn attr_macro_test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    if input.sig.asyncness.is_none() {
        return quote_spanned! { input.sig.fn_token.span()=>
            compile_error!("fn must be `async fn`");
        }
        .into();
    }

    let name = input.sig.ident;

    if !input.sig.inputs.is_empty() {
        return quote_spanned! { input.sig.inputs.span()=>
            compile_error!("arguments to main are not supported");
        }
        .into();
    }
    let attrs = input.attrs;
    let output = input.sig.output;
    let block = input.block;
    quote! {
        #(#attrs)*
        #[::core::prelude::v1::test]
        pub fn #name() #output {

            async fn __run() #output {
                #block
            }

            ::wstd::runtime::block_on(async {
                __run().await
            })
        }
    }
    .into()
}

/// Enables a HTTP server main function, for creating [HTTP servers].
///
/// [HTTP servers]: https://docs.rs/wstd/latest/wstd/http/server/index.html
///
/// # Examples
///
/// ```ignore
/// #[wstd::http_server]
/// async fn main(request: Request<Body>) -> Result<Response<Body>> {
///     Ok(Response::new("Hello!\n".into()))
/// }
/// ```
#[proc_macro_attribute]
pub fn attr_macro_http_server(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    let output = &input.sig.output;
    let inputs = &input.sig.inputs;
    let name = &input.sig.ident;
    let body = &input.block;
    let attrs = &input.attrs;
    let vis = &input.vis;

    if name != "main" {
        return quote_spanned! { input.sig.ident.span()=>
            compile_error!("only `async fn main` can be used for #[wstd::http_server]");
        }
        .into();
    }

    // Delegate to wstd's conditionally-compiled declarative macro.
    // The `cfg` checks in `__http_server_export!` run in wstd's context,
    // so consumers don't need to define wasip2/wasip3 features themselves.
    let asyncness = if input.sig.asyncness.is_some() {
        quote!(@async)
    } else {
        quote!(@sync)
    };

    let run_async = if input.sig.asyncness.is_some() {
        quote!(async)
    } else {
        quote!()
    };

    quote! {
        ::wstd::__http_server_export! {
            #asyncness
            { #(#attrs)* #vis #run_async fn __run(#inputs) #output { #body } }
        }

        fn main() {
            unreachable!("HTTP server components should be run with `handle` rather than `run`")
        }
    }
    .into()
}
