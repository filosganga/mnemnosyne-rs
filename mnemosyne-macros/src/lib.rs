use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::Parser,
    parse_macro_input, FnArg, ItemFn, Meta, ReturnType,
};

/// Procedural macro to protect async functions with Mnemosyne deduplication.
///
/// # Example
///
/// ```rust,ignore
/// #[protect(mnemosyne = self.cache, id = email.id)]
/// async fn send_email(&self, email: Email) -> Result<String, Error> {
///     // Your processing logic here
///     Ok("sent".to_string())
/// }
/// ```
///
/// This will expand to code that calls `mnemosyne.protect(id, || async { ... })`.
///
/// # Requirements
///
/// - The function must be `async`
/// - The function must return `Result<A, Error>` where `A` matches the type parameter of `Mnemosyne<A>`
/// - `A` must implement `Clone`
/// - The `id` expression must evaluate to a type that implements `Into<Id>`
#[proc_macro_attribute]
pub fn protect(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    // Parse the attribute arguments using syn::parse
    let parser = syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated;
    let args = match parser.parse(attr.clone()) {
        Ok(args) => args,
        Err(e) => return e.to_compile_error().into(),
    };

    // Parse the attribute arguments
    let mut mnemosyne_expr = None;
    let mut id_expr = None;

    for arg in args {
        match arg {
            Meta::NameValue(nv) => {
                let name = nv.path.get_ident().map(|i| i.to_string());
                match name.as_deref() {
                    Some("mnemosyne") => {
                        mnemosyne_expr = Some(nv.value);
                    }
                    Some("id") => {
                        id_expr = Some(nv.value);
                    }
                    _ => {
                        return syn::Error::new_spanned(
                            nv.path,
                            "Unknown attribute parameter. Expected 'mnemosyne' or 'id'",
                        )
                        .to_compile_error()
                        .into();
                    }
                }
            }
            _ => {
                return syn::Error::new_spanned(arg, "Expected name-value pair like `mnemosyne = self.cache` or `id = email.id`")
                    .to_compile_error()
                    .into();
            }
        }
    }

    let mnemosyne = match mnemosyne_expr {
        Some(expr) => expr,
        None => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "Missing required 'mnemosyne' parameter",
            )
            .to_compile_error()
            .into();
        }
    };

    let id = match id_expr {
        Some(expr) => expr,
        None => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "Missing required 'id' parameter",
            )
            .to_compile_error()
            .into();
        }
    };

    // Validate the function
    if input.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            input.sig.fn_token,
            "The #[protect] macro can only be applied to async functions",
        )
        .to_compile_error()
        .into();
    }

    // Extract function components
    let fn_vis = &input.vis;
    let fn_name = &input.sig.ident;
    let fn_generics = &input.sig.generics;
    let fn_inputs = &input.sig.inputs;
    let fn_output = &input.sig.output;
    let fn_block = &input.block;
    let fn_attrs = &input.attrs;

    // Extract parameter names for the closure
    let param_names: Vec<_> = fn_inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                    return Some(&pat_ident.ident);
                }
            }
            None
        })
        .collect();

    // Validate return type
    let return_type = match fn_output {
        ReturnType::Default => {
            return syn::Error::new_spanned(
                &input.sig,
                "Function must return Result<A, Error>",
            )
            .to_compile_error()
            .into();
        }
        ReturnType::Type(_, ty) => ty,
    };

    // Generate the expanded function
    let expanded = quote! {
        #(#fn_attrs)*
        #fn_vis async fn #fn_name #fn_generics(#fn_inputs) -> #return_type {
            let __mnemosyne_id = #id;
            let __mnemosyne = #mnemosyne;

            __mnemosyne.protect(__mnemosyne_id, || async move {
                #(let #param_names = #param_names;)*
                #fn_block
            }).await
        }
    };

    TokenStream::from(expanded)
}
