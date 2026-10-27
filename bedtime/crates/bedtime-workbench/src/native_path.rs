//! Lossless platform-native path records shared by persistence consumers.
//! Foreign platforms are rejected. Field owners validate absolute/relative scope,
//! normalization and size separately. Never persist display strings.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "platform",
    content = "units",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum NativePath {
    #[cfg(unix)]
    UnixBytes(Vec<u8>),
    #[cfg(windows)]
    WindowsWide(Vec<u16>),
    #[cfg(not(any(unix, windows)))]
    Utf8(String),
}

impl NativePath {
    pub fn encode(path: &Path) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Ok(Self::UnixBytes(path.as_os_str().as_bytes().to_vec()))
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Ok(Self::WindowsWide(path.as_os_str().encode_wide().collect()))
        }
        #[cfg(not(any(unix, windows)))]
        {
            path.to_str()
                .map(|value| Self::Utf8(value.to_owned()))
                .ok_or_else(|| "This platform cannot persist non-UTF-8 paths".into())
        }
    }

    pub fn decode(self) -> Result<PathBuf, String> {
        // These are platform-specific, safe native constructors. In particular, do
        // not use OsString::from_encoded_bytes_unchecked on untrusted JSON bytes.
        // Field codecs/owners check NUL and lexical validity before use.
        match self {
            #[cfg(unix)]
            Self::UnixBytes(bytes) => {
                use std::{ffi::OsString, os::unix::ffi::OsStringExt};
                Ok(PathBuf::from(OsString::from_vec(bytes)))
            }
            #[cfg(windows)]
            Self::WindowsWide(units) => {
                use std::{ffi::OsString, os::windows::ffi::OsStringExt};
                Ok(PathBuf::from(OsString::from_wide(&units)))
            }
            #[cfg(not(any(unix, windows)))]
            Self::Utf8(value) => Ok(PathBuf::from(value)),
        }
    }
}

/// Serde field codec. Reads former UTF-8 strings as well as native records;
/// all new writes use native units. No filesystem access is performed.
pub mod path {
    use super::*;
    pub fn serialize<S: serde::Serializer>(value: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        validate(value).map_err(serde::ser::Error::custom)?;
        NativePath::encode(value)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = PathBuf;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a native path record or legacy UTF-8 path")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<PathBuf, E> {
                Ok(PathBuf::from(value))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<PathBuf, A::Error> {
                NativePath::deserialize(serde::de::value::MapAccessDeserializer::new(map))?
                    .decode()
                    .map_err(serde::de::Error::custom)
            }
        }
        let path = deserializer.deserialize_any(Visitor)?;
        validate(&path).map_err(serde::de::Error::custom)?;
        Ok(path)
    }
}
pub mod option {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(transparent)]
    struct Value(#[serde(with = "path")] PathBuf);
    pub fn serialize<S: serde::Serializer>(
        value: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .as_ref()
            .map(|p| Value(p.clone()))
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Ok(Option::<Value>::deserialize(deserializer)?.map(|value| value.0))
    }
}
/// Native paths cannot contain NUL; other constraints belong to the field.
pub fn validate(path: &Path) -> Result<(), String> {
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err("Native path contains NUL".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Paths {
        #[serde(with = "path")]
        required: PathBuf,
        #[serde(with = "option")]
        optional: Option<PathBuf>,
    }
    #[test]
    fn legacy_strings_remain_readable_and_invalid_records_are_rejected() {
        let legacy = r#"{"required":"/project/file","optional":"/project"}"#;
        let paths: Paths = serde_json::from_str(legacy).unwrap();
        assert_eq!(paths.optional, Some(PathBuf::from("/project")));
        assert_eq!(
            serde_json::from_slice::<Paths>(&serde_json::to_vec(&paths).unwrap()).unwrap(),
            paths
        );
        assert!(
            serde_json::from_str::<Paths>(r#"{"required":"nul\u0000name","optional":null}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<Paths>(
                r#"{"required":{"platform":"foreign","units":[]},"optional":null}"#
            )
            .is_err()
        );
        #[cfg(feature = "persistence")]
        {
            let old: Paths =
                ron::from_str(r#"(required:"/project/file",optional:Some("/project"))"#).unwrap();
            assert_eq!(old, paths);
            assert_eq!(
                ron::from_str::<Paths>(&ron::to_string(&paths).unwrap()).unwrap(),
                paths
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn unix_native_units_roundtrip_in_required_and_optional_fields() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let path = PathBuf::from(OsString::from_vec(b"/project-\xff/file-\xfe".to_vec()));
        let paths = Paths {
            required: path.clone(),
            optional: Some(path),
        };
        assert_eq!(
            serde_json::from_slice::<Paths>(&serde_json::to_vec(&paths).unwrap()).unwrap(),
            paths
        );
        #[cfg(feature = "persistence")]
        assert_eq!(
            ron::from_str::<Paths>(&ron::to_string(&paths).unwrap()).unwrap(),
            paths
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_unpaired_surrogates_roundtrip() {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt};
        let path = PathBuf::from(OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd800,
        ]));
        let paths = Paths {
            required: path.clone(),
            optional: Some(path),
        };
        assert_eq!(
            serde_json::from_slice::<Paths>(&serde_json::to_vec(&paths).unwrap()).unwrap(),
            paths
        );
    }
}
