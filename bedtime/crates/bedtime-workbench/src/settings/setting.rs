//! The `Setting` trait and the `settings!` macro.
use bedtime_persistence::StoreId;
use bevy::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use std::borrow::Cow;

/// A typed, named value resolved through the settings layers. Usually
/// declared with [`settings!`](crate::settings!).
pub trait Setting: Send + Sync + 'static {
    type Value: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static;
    /// "filetree.indent". A function so generic settings can derive it.
    fn key() -> Cow<'static, str>;
    fn default() -> Self::Value;
    /// Which layers may hold this setting. All of them by default.
    fn allowed_in(_store: StoreId) -> bool {
        true
    }
    /// Runs whenever the resolved value changes, including once at startup.
    fn apply(_world: &mut World, _value: &Self::Value) {}

    /// The section of the settings panel it's shown in; `None` isn't shown.
    #[cfg(feature = "ui")]
    const SECTION: Option<&'static str> = None;
    /// Its label in the settings panel: the type's name in words by default
    /// (`FontSize` is "Font size").
    #[cfg(feature = "ui")]
    fn label() -> Cow<'static, str> {
        let path = std::any::type_name::<Self>();
        let name = path.split('<').next().unwrap_or(path);
        crate::settings::ui::words(name.rsplit("::").next().unwrap_or(name)).into()
    }
    /// Shown when hovering its label.
    #[cfg(feature = "ui")]
    fn description() -> Option<&'static str> {
        None
    }
    /// Whether its row is shown while editing `layer` (only while another
    /// setting is on, say). Rows for layers a setting isn't allowed in are
    /// never shown.
    #[cfg(feature = "ui")]
    fn visible(_world: &World, _layer: StoreId) -> bool {
        true
    }
    /// How it's edited. `None` uses the editor for its value's type
    /// ([`inferred_ui`](Setting::inferred_ui)).
    #[cfg(feature = "ui")]
    fn ui() -> Option<crate::settings::ui::SettingUi<Self::Value>> {
        None
    }
    /// The editor for its value's type, if there's an obvious one. `settings!`
    /// provides it.
    #[cfg(feature = "ui")]
    fn inferred_ui() -> Option<crate::settings::ui::SettingUi<Self::Value>> {
        None
    }
}

/// Declare settings. Each is a block of entries; `key` and `default` are
/// required. Settings inside a quoted section are shown in the settings panel
/// (with the `ui` feature); others aren't.
///
/// ```ignore
/// settings! {
///     "Appearance" {
///         /// The editor's font size, in points.
///         pub FontSize: f32 {
///             default: 15.5,
///             key: "appearance.font_size",
///             ui: slider(8.0..=72.0).suffix(" pt"),
///             apply: |world, size| world.resource_mut::<Appearance>().font_size = *size,
///         }
///     }
///     pub ReopenLast: bool { default: false, key: "startup.reopen_last", only_in: [GlobalStore] }
/// }
/// ```
///
/// Entries:
/// - `default`, `key`: the value without any layer, and its name in a layer's
///   file.
/// - `apply: |world, value| …`: runs when the resolved value changes.
/// - `only_in: [StoreA, StoreB]`: the layers that may hold it.
/// - `label`, `description`: text in the settings panel.
/// - `ui`: how it's edited: `toggle()`, `text()`, `drag()`, `slider(range)`,
///   `choice()`, or `custom(row)` for a row of its own. Without it, the editor
///   comes from the value's type.
/// - `visible: |world, layer| …`: whether its row is shown.
#[macro_export]
macro_rules! settings {
    () => {};
    ($section:literal { $($body:tt)* } $($rest:tt)*) => {
        $crate::__settings_in! { ($section) $($body)* }
        $crate::settings! { $($rest)* }
    };
    ($(#[$meta:meta])* $vis:vis $name:ident : $ty:ty { $($entries:tt)* } $($rest:tt)*) => {
        $crate::__setting! { () $(#[$meta])* $vis $name : $ty { $($entries)* } }
        $crate::settings! { $($rest)* }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __settings_in {
    (($section:literal)) => {};
    (($section:literal) $(#[$meta:meta])* $vis:vis $name:ident : $ty:ty { $($entries:tt)* } $($rest:tt)*) => {
        $crate::__setting! { ($section) $(#[$meta])* $vis $name : $ty { $($entries)* } }
        $crate::__settings_in! { ($section) $($rest)* }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __setting {
    (($($section:literal)?) $(#[$meta:meta])* $vis:vis $name:ident : $ty:ty { $($entries:tt)* }) => {
        $(#[$meta])*
        $vis struct $name;
        impl $crate::settings::Setting for $name {
            type Value = $ty;
            $crate::__setting_entries! { $ty ; $($entries)* }
            $( $crate::__setting_section! { $section, $ty } )?
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __setting_entries {
    ($ty:ty ;) => {};
    ($ty:ty ; only_in : [$($store:ty),* $(,)?] $(, $($rest:tt)*)?) => {
        fn allowed_in(store: $crate::settings::__private::StoreId) -> bool {
            [$($crate::settings::__private::StoreId::of::<$store>()),*].contains(&store)
        }
        $crate::__setting_entries! { $ty ; $($($rest)*)? }
    };
    ($ty:ty ; $entry:ident : $value:expr $(, $($rest:tt)*)?) => {
        $crate::__setting_entry! { $entry, $ty, $value }
        $crate::__setting_entries! { $ty ; $($($rest)*)? }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __setting_entry {
    (key, $ty:ty, $value:expr) => {
        fn key() -> ::std::borrow::Cow<'static, str> {
            ::std::borrow::Cow::Borrowed($value)
        }
    };
    (default, $ty:ty, $value:expr) => {
        fn default() -> $ty {
            $value
        }
    };
    (apply, $ty:ty, $value:expr) => {
        fn apply(world: &mut $crate::settings::__private::World, value: &$ty) {
            let apply: fn(&mut $crate::settings::__private::World, &$ty) = $value;
            apply(world, value)
        }
    };
    (label, $ty:ty, $value:expr) => {
        $crate::__setting_ui_entry! { label, $ty, $value }
    };
    (description, $ty:ty, $value:expr) => {
        $crate::__setting_ui_entry! { description, $ty, $value }
    };
    (visible, $ty:ty, $value:expr) => {
        $crate::__setting_ui_entry! { visible, $ty, $value }
    };
    (ui, $ty:ty, $value:expr) => {
        $crate::__setting_ui_entry! { ui, $ty, $value }
    };
}

#[cfg(feature = "ui")]
#[doc(hidden)]
#[macro_export]
macro_rules! __setting_ui_entry {
    (label, $ty:ty, $value:expr) => {
        fn label() -> ::std::borrow::Cow<'static, str> {
            ::std::borrow::Cow::Borrowed($value)
        }
    };
    (description, $ty:ty, $value:expr) => {
        fn description() -> ::std::option::Option<&'static str> {
            ::std::option::Option::Some($value)
        }
    };
    (visible, $ty:ty, $value:expr) => {
        fn visible(
            world: &$crate::settings::__private::World,
            layer: $crate::settings::__private::StoreId,
        ) -> bool {
            let visible: fn(
                &$crate::settings::__private::World,
                $crate::settings::__private::StoreId,
            ) -> bool = $value;
            visible(world, layer)
        }
    };
    (ui, $ty:ty, $value:expr) => {
        fn ui() -> ::std::option::Option<$crate::settings::ui::SettingUi<$ty>> {
            #[allow(unused_imports)]
            use $crate::settings::ui::editors::*;
            ::std::option::Option::Some(($value).into())
        }
    };
}

#[cfg(not(feature = "ui"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __setting_ui_entry {
    ($entry:ident, $ty:ty, $value:expr) => {};
}

#[cfg(feature = "ui")]
#[doc(hidden)]
#[macro_export]
macro_rules! __setting_section {
    ($section:literal, $ty:ty) => {
        const SECTION: ::std::option::Option<&'static str> = ::std::option::Option::Some($section);
        fn inferred_ui() -> ::std::option::Option<$crate::settings::ui::SettingUi<$ty>> {
            #[allow(unused_imports)]
            use $crate::settings::ui::infer::{InferEnum, InferNone, InferType};
            (&&&$crate::settings::ui::infer::Infer::<$ty>::new()).infer()
        }
    };
}

#[cfg(not(feature = "ui"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __setting_section {
    ($section:literal, $ty:ty) => {};
}
