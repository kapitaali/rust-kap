//! `kap-derive` — proc-macro for `#[kap_fn]`
//!
//! Current scaffold: attribute is parsed and the function is passed through
//! unchanged. The full codegen (generating `struct Foo; impl NativeFn for Foo`
//! + `inventory::submit!`) lands in the next cut; this stub lets `kap-ext-*`
//! crates already depend on `kap-derive` and compile.
//!
//! Usage (as documented in `docs/native-api-design.html` §05):
//! ```rust,ignore
//! use kap_core::native::prelude::*;
//! #[kap_fn(name = "stats:mean", arity = "monadic")]
//! fn stats_mean(_ctx: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
//!     let v = args.mono_named("stats:mean")?;
//!     // ...
//!     Ok(Rc::new(APLValue::Null))
//! }
//! ```

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, ItemFn, LitStr, Token};

struct KapFnArgs {
    _raw: String,
}

impl Parse for KapFnArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Accept any `name = "...", arity = "..."` etc. without validating yet.
        // We just slurp the token stream so the attribute parses.
        let mut raw = String::new();
        while !input.is_empty() {
            if input.peek(LitStr) {
                let lit: LitStr = input.parse()?;
                raw.push_str(&lit.value());
            } else if input.peek(syn::Ident) {
                let ident: syn::Ident = input.parse()?;
                raw.push_str(&ident.to_string());
            } else if input.peek(Token![=]) {
                let _: Token![=] = input.parse()?;
                raw.push('=');
            } else if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
                raw.push(',');
            } else {
                let _: proc_macro2::TokenTree = input.parse()?;
            }
        }
        Ok(Self { _raw: raw })
    }
}

/// Kap native function attribute.
///
/// ```rust,ignore
/// #[kap_fn(name = "stats:mean")]
/// fn stats_mean(ctx: &NativeContext, args: Args) -> Result<…, AplError> { … }
/// ```
/// or with options:
/// ```rust,ignore
/// #[kap_fn(name = "stats:stdev", arity = "monadic", rank = "scalar", doc = "…")]
/// fn stdev(...) { … }
/// ```
///
/// Scaffold: currently a pass-through. The next iteration will generate:
/// * a `struct` named from the function (e.g. `StatsMean`)
/// * `impl NativeFn for StatsMean` forwarding to the function
/// * `inventory::submit! { NativeReg::of::<StatsMean>() }`
#[proc_macro_attribute]
pub fn kap_fn(attr: TokenStream, item: TokenStream) -> TokenStream {
    let _args = parse_macro_input!(attr with KapFnArgs::parse);
    let input_fn = parse_macro_input!(item as ItemFn);

    // For now, emit the function unchanged plus a compile-time note that the
    // attribute was recognized. This lets `kap-ext-*` crates use `#[kap_fn]`
    // today and get the full codegen later without changing call sites.
    let expanded = quote! {
        #input_fn
    };
    TokenStream::from(expanded)
}
