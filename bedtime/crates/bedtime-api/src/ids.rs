//! Distinct names for extensible registries. Serialization preserves their wire names.
//!
//! Registry lookups reject IDs from another category:
//! ```compile_fail
//! use bedtime_api::{PanelTypeId, workbench::WorkbenchRegistry};
//! WorkbenchRegistry::default().command(&PanelTypeId::new("example.panel"));
//! ```
//! ```compile_fail
//! use bedtime_api::{ViewerId, workbench::WorkbenchRegistry};
//! WorkbenchRegistry::default().panel(&ViewerId::new("example.viewer"));
//! ```
use serde::{Deserialize, Serialize};
use std::{
    borrow::{Borrow, Cow},
    fmt,
    ops::Deref,
};

macro_rules! registry_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Cow<'static, str>);

        impl $name {
            pub const fn new(name: &'static str) -> Self {
                Self(Cow::Borrowed(name))
            }

            pub fn as_str(&self) -> &str {
                self.0.as_ref()
            }
        }

        impl From<String> for $name {
            fn from(name: String) -> Self {
                Self(Cow::Owned(name))
            }
        }

        impl From<&str> for $name {
            fn from(name: &str) -> Self {
                Self::from(name.to_owned())
            }
        }

        impl From<&String> for $name {
            fn from(name: &String) -> Self {
                Self::from(name.as_str())
            }
        }

        impl From<&$name> for $name {
            fn from(name: &$name) -> Self {
                name.clone()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }
    };
}

registry_id!(CommandId);
registry_id!(PanelTypeId);
registry_id!(ViewerId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_preserve_serialized_names() {
        let wire = r#""example.open""#;
        let command: CommandId = serde_json::from_str(wire).unwrap();
        assert_eq!(command, CommandId::new("example.open"));
        assert_eq!(serde_json::to_string(&command).unwrap(), wire);
        let panel: PanelTypeId = serde_json::from_str(wire).unwrap();
        assert_eq!(panel.as_str(), "example.open");
        assert_eq!(serde_json::to_string(&panel).unwrap(), wire);
        let viewer: ViewerId = serde_json::from_str(wire).unwrap();
        assert_eq!(viewer.as_str(), "example.open");
        assert_eq!(serde_json::to_string(&viewer).unwrap(), wire);
        assert!(serde_json::from_str::<CommandId>("42").is_err());
    }
}
