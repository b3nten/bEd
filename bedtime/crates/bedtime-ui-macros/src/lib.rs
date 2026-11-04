//! `#[panel]`; see `bedtime_ui::layouts::panel`.
use proc_macro::TokenStream;
use quote::quote;
use syn::{Item, parse_macro_input};

/// Make a type a panel component: derives `Component` and requires the
/// panel's `PanelInstance`. Put it above the type's other derives, and
/// implement `Panel` (label and drawing) as usual. More `#[require(...)]`
/// attributes on the type are kept.
#[proc_macro_attribute]
pub fn panel(arguments: TokenStream, item: TokenStream) -> TokenStream {
    if !arguments.is_empty() {
        let arguments = proc_macro2::TokenStream::from(arguments);
        return syn::Error::new_spanned(arguments, "#[panel] takes no arguments")
            .to_compile_error()
            .into();
    }
    let item = parse_macro_input!(item as Item);
    if !matches!(item, Item::Struct(_) | Item::Enum(_)) {
        return syn::Error::new_spanned(item, "#[panel] goes on a struct or enum")
            .to_compile_error()
            .into();
    }
    quote! {
        #[derive(::bevy::prelude::Component)]
        #[require(::bedtime_ui::layouts::PanelInstance = ::bedtime_ui::layouts::PanelInstance::of::<Self>())]
        #item
    }
    .into()
}
