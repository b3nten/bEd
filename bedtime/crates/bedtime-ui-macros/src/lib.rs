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
    // An app that depends on `bedtime` alone has bedtime-ui as `bedtime::ui`.
    let ui = if names_crate("bedtime-ui") {
        quote!(::bedtime_ui)
    } else {
        quote!(::bedtime::ui)
    };
    quote! {
        #[derive(::bevy::prelude::Component)]
        #[require(#ui::layouts::PanelInstance = #ui::layouts::PanelInstance::of::<Self>())]
        #item
    }
    .into()
}

/// Whether the crate being compiled names `name` in its manifest, as its own
/// name or as a dependency. (When that can't be told, it does.)
fn names_crate(name: &str) -> bool {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok().and_then(|folder| {
        std::fs::read_to_string(std::path::Path::new(&folder).join("Cargo.toml")).ok()
    });
    let Some(manifest) = manifest else {
        return true;
    };
    manifest.lines().any(|line| {
        let line = line.trim();
        line.strip_prefix(name)
            .is_some_and(|rest| rest.starts_with([' ', '=', '.']))
            || line.replace(' ', "") == format!("name=\"{name}\"")
            || (line.starts_with('[') && line.ends_with(&format!(".{name}]")))
    })
}
