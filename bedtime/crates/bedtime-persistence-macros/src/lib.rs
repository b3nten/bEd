//! `#[derive(Persisted)]`; see `bedtime_persistence::Persisted`.
use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, LitStr, parse_macro_input};

/// `#[derive(Persisted)] #[persisted(key = "layout_defaults")]`: the resource
/// is saved as the entry `layout_defaults` of the store it's persisted in.
#[proc_macro_derive(Persisted, attributes(persisted))]
pub fn derive_persisted(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let mut key = None;
    for attribute in input
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("persisted"))
    {
        let result = attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("key") {
                key = Some(meta.value()?.parse::<LitStr>()?);
                Ok(())
            } else {
                Err(meta.error("expected `key = \"...\"`"))
            }
        });
        if let Err(error) = result {
            return error.to_compile_error().into();
        }
    }
    let Some(key) = key else {
        return syn::Error::new_spanned(
            &input.ident,
            "Persisted needs a key: #[persisted(key = \"...\")]",
        )
        .to_compile_error()
        .into();
    };
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    quote! {
        impl #impl_generics ::bedtime_persistence::Persisted for #name #type_generics #where_clause {
            const KEY: &'static str = #key;
        }
    }
    .into()
}
