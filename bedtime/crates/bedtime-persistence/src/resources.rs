//! Resources saved as their own entry.
use crate::StoreBackend;
use bevy::prelude::*;
use serde::{Serialize, de::DeserializeOwned};

/// A resource saved as its own entry with `app.persist::<T, S>()`, named by
/// its key so renaming the type doesn't lose what was saved:
///
/// ```ignore
/// #[derive(Resource, Serialize, Deserialize, Persisted)]
/// #[persisted(key = "layout_defaults")]
/// struct LayoutDefaults(Vec<String>);
/// ```
pub trait Persisted: Resource + Serialize + DeserializeOwned {
    /// The entry's name: `<key>.ron` in a `DirectoryStore`.
    const KEY: &'static str;
}

pub(crate) fn to_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let config = ron::ser::PrettyConfig::new().indentor("    ".to_owned());
    ron::ser::to_string_pretty(value, config)
        .map(|text| (text + "\n").into_bytes())
        .map_err(|error| error.to_string())
}

pub(crate) fn from_bytes<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    ron::de::from_bytes(bytes).map_err(|error| error.to_string())
}

/// Read `T` straight from a backend, before there's an App (to decide how to
/// build it, say). `None` if nothing is saved.
pub fn read_persisted<T: Persisted>(backend: &dyn StoreBackend) -> Result<Option<T>, String> {
    backend
        .read(T::KEY)?
        .map(|bytes| from_bytes(&bytes))
        .transpose()
}

/// Write `T` straight to a backend, in the format `persist` uses.
pub fn write_persisted<T: Persisted>(backend: &dyn StoreBackend, value: &T) -> Result<(), String> {
    backend.write(T::KEY, &to_bytes(value)?)
}
