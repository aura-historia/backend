use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprArray, FnArg, ItemFn, LitBool, Token, parse_macro_input};

struct Arguments {
    services: Vec<Expr>,
    isolated: bool,
    exclusive: bool,
}

impl Parse for Arguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut services = None;
        let mut isolated = None;
        let mut exclusive = None;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "services" if services.is_none() => {
                    let array: ExprArray = input.parse()?;
                    services = Some(array.elems.into_iter().collect());
                }
                "isolated" if isolated.is_none() => {
                    isolated = Some(input.parse::<LitBool>()?.value)
                }
                "exclusive" if exclusive.is_none() => {
                    exclusive = Some(input.parse::<LitBool>()?.value)
                }
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown or duplicate integration-test option",
                    ));
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(Self {
            services: services.ok_or_else(|| input.error("expected services = [...]"))?,
            isolated: isolated.unwrap_or(false),
            exclusive: exclusive.unwrap_or(false),
        })
    }
}

fn service_ident(index: usize) -> syn::Ident {
    syn::Ident::new(&format!("__svc_{index}"), proc_macro2::Span::call_site())
}

/// Owns service setup/body/teardown and, with `isolated = true`, a per-test TestContext.
/// `exclusive = true` requests a scoped context excluding all other test reservations.
/// An optional `context: TestContext` argument receives that scope. Legacy callers retain
/// process-local serialization and reserve the actual cross-process exclusive gate.
///
/// A standalone service fixture can expose these lifecycle methods:
///
/// ```rust
/// pub struct S3;
///
/// impl S3 {
///     pub fn service_names() -> &'static [&'static str] {
///         &["s3"]
///     }
///
///     pub async fn set_up() {
///         // setup logic
///     }
///
///     pub async fn tear_down() {
///         // teardown logic
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn aura_integration_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as Arguments);
    let input = parse_macro_input!(item as ItemFn);
    match expand(args, input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(args: Arguments, input: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    if input.sig.asyncness.is_none() || input.sig.inputs.len() > 1 {
        return Err(syn::Error::new_spanned(
            &input.sig,
            "expected an async test with zero arguments or one TestContext argument",
        ));
    }
    let scoped = args.isolated || args.exclusive;
    let context_argument = match input.sig.inputs.first() {
        Some(FnArg::Typed(argument)) if scoped => {
            let pattern = &argument.pat;
            let ty = &argument.ty;
            quote! {let #pattern: #ty=__context.clone();}
        }
        Some(_) => {
            return Err(syn::Error::new_spanned(
                &input.sig,
                "a context argument requires isolated = true or exclusive = true",
            ));
        }
        None => quote! {},
    };
    let lifecycle = lifecycle(&args, &input, context_argument);
    let attrs = &input.attrs;
    let name = &input.sig.ident;
    let (serial, run) = execution(&args, name, lifecycle);
    Ok(quote! {#( #attrs )* #[tokio::test] #serial async fn #name() {#run}})
}

fn lifecycle(
    args: &Arguments,
    input: &ItemFn,
    context_argument: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let bindings = args.services.iter().enumerate().map(|(index, expression)| {
        let ident = service_ident(index);
        quote! {let #ident=#expression;}
    });
    let identifiers: Vec<_> = (0..args.services.len()).map(service_ident).collect();
    let setup = identifiers
        .iter()
        .map(|ident| quote! {#ident.set_up().await;__completed+=1;});
    let teardown=identifiers.iter().enumerate().rev().map(|(index,ident)|quote! {
        if __completed>#index {
            if let Err(panic)=test_api::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(#ident.tear_down())).await {
                if __teardown_panic.is_none() {__teardown_panic=Some(panic);} else {eprintln!("another integration service teardown panicked");}
            }
        }
    });
    let body = &input.block;
    let mutable = if identifiers.is_empty() {
        quote! {}
    } else {
        quote! {mut}
    };
    quote! {
        #( #bindings )*
        let __service_declarations: &[&[&str]] = &[ #( #identifiers.service_names() ),* ];
        let __services:Vec<&str> = __service_declarations.iter().flat_map(|names|names.iter().copied()).collect();
        let __environment_declarations: &[Vec<(&str,&str)>] = &[ #( #identifiers.env_vars() ),* ];
        let __env:Vec<(&str,&str)> = __environment_declarations.concat();
        let __aws=if __services.is_empty() {None} else {Some(test_api::aws::get_aws_emulator(&__services,&__env).await)};
        let #mutable __completed=0usize;
        let __setup=test_api::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async { #( #setup )* })).await;
        let __body=match __setup {Ok(())=>test_api::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {#context_argument #body})).await,Err(panic)=>Err(panic)};
        let #mutable __teardown_panic: Option<Box<dyn std::any::Any+Send>>=None;
        #( #teardown )*
        drop(__aws);
        match (__body,__teardown_panic) {
            (Err(panic),cleanup)=>{if cleanup.is_some() {eprintln!("integration service teardown also panicked");} std::panic::resume_unwind(panic);},
            (Ok(()),Some(panic))=>std::panic::resume_unwind(panic),
            (Ok(()),None)=>{},
        }
    }
}

fn execution(
    args: &Arguments,
    name: &syn::Ident,
    lifecycle: proc_macro2::TokenStream,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    if args.isolated || args.exclusive {
        let isolation = if args.exclusive {
            quote! {test_api::Isolation::Exclusive}
        } else {
            quote! {test_api::Isolation::Isolated}
        };
        (
            quote! {},
            quote! {
                let __session=test_api::TestContext::acquire(concat!(module_path!(),"::",stringify!(#name)),#isolation).await.expect("acquire test context");
                let __context=__session.context().clone();
                let __result=test_api::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(__context.scope(async {#lifecycle}))).await;
                let __cleanup=__session.finish().await;
                match (__result,__cleanup) {
                    (Err(panic),cleanup)=>{if let Err(error)=cleanup {eprintln!("test context cleanup also failed: {error}");}std::panic::resume_unwind(panic);},
                    (Ok(()),Err(error))=>panic!("test context cleanup failed: {error}"),
                    (Ok(()),Ok(()))=>{},
                }
            },
        )
    } else {
        (
            quote! {#[test_api::serial]},
            quote! {
                let __exclusive=test_api::reserve_exclusive().await.expect("reserve legacy integration fixture");
                __exclusive.scope(async {#lifecycle}).await;
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_reject_unknown_duplicate_or_nonboolean_options() {
        for text in [
            "services = [], typo = true",
            "services = [], isolated = true, isolated = false",
            "services = [], isolated = 1",
        ] {
            assert!(syn::parse_str::<Arguments>(text).is_err());
        }
        assert!(
            syn::parse_str::<Arguments>(
                "services = [Postgres::new(\"migrations\")], isolated = true"
            )
            .unwrap()
            .isolated
        );
    }
}
