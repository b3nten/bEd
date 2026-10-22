//! Root-scoped visibility data and its application-persisted wire format.
//!
//! No filesystem access is performed here: roots may be temporarily unavailable.
//! Native path units are persisted, not display strings, and records from another
//! platform are rejected rather than converted lossily.
use bevy::prelude::Component;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component as PathComponent, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTreeVisibility {
    /// Hide dotfiles (and other platform-hidden entries identified by the panel).
    pub hide_hidden: bool,
    /// Explicitly hidden paths, relative to the owning root.
    pub hidden_paths: BTreeSet<PathBuf>,
}

impl Default for FileTreeVisibility {
    fn default() -> Self {
        Self {
            hide_hidden: true,
            hidden_paths: BTreeSet::new(),
        }
    }
}

impl FileTreeVisibility {
    /// Explicitly hide an entry and its descendants, never the root itself.
    /// Paths must already be normalized, nonempty, root-relative native paths.
    pub fn hide(&mut self, relative: PathBuf) -> Result<(), String> {
        validate_relative(&relative)?;
        self.hidden_paths.insert(relative);
        Ok(())
    }

    /// Remove only this explicit entry; an explicitly hidden ancestor remains.
    pub fn unhide(&mut self, relative: &Path) -> bool {
        validate_relative(relative).is_ok() && self.hidden_paths.remove(relative)
    }

    /// Test explicit hiding, including any hidden ancestor, component by component.
    /// `hide_hidden` is a separate policy applied to entry metadata by the panel.
    pub fn is_hidden(&self, relative: &Path) -> bool {
        validate_relative(relative).is_ok()
            && relative
                .ancestors()
                .any(|ancestor| self.hidden_paths.contains(ancestor))
    }
}

/// Application-owned preferences shared by all panels using the same root.
/// The application owns storage I/O and decides when to encode or decode them.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct FileTreePreferences {
    pub roots: BTreeMap<PathBuf, FileTreeVisibility>,
}

impl FileTreePreferences {
    /// Encode versioned JSON without losing native filename units.
    /// Publicly mutable fields are validated here as well as on decode.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&self.record()?).map_err(|error| error.to_string())
    }

    fn record(&self) -> Result<PreferencesRecord, String> {
        let mut roots = Vec::with_capacity(self.roots.len());
        for (root, visibility) in &self.roots {
            validate_root(root)?;
            let mut hidden_paths = Vec::with_capacity(visibility.hidden_paths.len());
            for path in &visibility.hidden_paths {
                validate_relative(path)?;
                hidden_paths.push(NativePath::encode(path)?);
            }
            roots.push(RootRecord {
                root: NativePath::encode(root)?,
                hide_hidden: visibility.hide_hidden,
                hidden_paths,
            });
        }
        Ok(PreferencesRecord { version: 1, roots })
    }

    /// Validate the entire document before returning any preferences.
    /// Roots must be absolute and lexically normalized, not necessarily existent.
    /// Unknown fields/platforms/versions, duplicates and escaping paths are errors;
    /// no record is silently skipped, normalized or repaired.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let record: PreferencesRecord = serde_json::from_slice(bytes)
            .map_err(|error| format!("Invalid file tree preferences: {error}"))?;
        Self::from_record(record)
    }

    fn from_record(record: PreferencesRecord) -> Result<Self, String> {
        if record.version != 1 {
            return Err(format!(
                "Unsupported file tree preferences version: {}",
                record.version
            ));
        }
        let mut roots = BTreeMap::new();
        for record in record.roots {
            let root = record.root.decode()?;
            validate_root(&root)?;
            let mut hidden_paths = BTreeSet::new();
            for encoded in record.hidden_paths {
                let relative = encoded.decode()?;
                validate_relative(&relative)?;
                if !hidden_paths.insert(relative) {
                    return Err("Duplicate hidden path in file tree preferences".into());
                }
            }
            let visibility = FileTreeVisibility {
                hide_hidden: record.hide_hidden,
                hidden_paths,
            };
            if roots.insert(root, visibility).is_some() {
                return Err("Duplicate root in file tree preferences".into());
            }
        }
        Ok(Self { roots })
    }
}

// The embedding application can store the same validated native-path records in
// its own format (for example RON), without embedding a JSON string.
impl Serialize for FileTreePreferences {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.record()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for FileTreePreferences {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_record(PreferencesRecord::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

fn validate_relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || !path
            .components()
            .all(|component| matches!(component, PathComponent::Normal(_)))
    {
        return Err("Hidden paths must be nonempty root-relative paths without '.' or '..'".into());
    }
    validate_normalized(path)
}

fn validate_root(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, PathComponent::CurDir | PathComponent::ParentDir))
    {
        return Err("File tree preference roots must be absolute normalized paths".into());
    }
    validate_normalized(path)
}

fn validate_normalized(path: &Path) -> Result<(), String> {
    // Components elides interior '.', repeated separators and trailing separators.
    // Compare native strings, not Path equality (which also elides those spellings).
    let normalized: PathBuf = path.components().collect();
    if path.as_os_str() != normalized.as_os_str()
        || path.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err("Preference paths must be normalized and contain no NUL units".into());
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferencesRecord {
    version: u32,
    // A sequence, rather than a JSON map, preserves duplicate keys for validation.
    roots: Vec<RootRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootRecord {
    root: NativePath,
    hide_hidden: bool,
    hidden_paths: Vec<NativePath>,
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "platform",
    content = "units",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum NativePath {
    #[cfg(unix)]
    UnixBytes(Vec<u8>),
    #[cfg(windows)]
    WindowsWide(Vec<u16>),
    #[cfg(not(any(unix, windows)))]
    Utf8(String),
}

impl NativePath {
    fn encode(path: &Path) -> Result<Self, String> {
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

    fn decode(self) -> Result<PathBuf, String> {
        // These are platform-specific, safe native constructors. In particular, do
        // not use OsString::from_encoded_bytes_unchecked on untrusted JSON bytes.
        // NUL and lexical validity are checked before the result enters preferences.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\unavailable-filetree-preferences-test")
        } else {
            PathBuf::from("/unavailable-filetree-preferences-test")
        }
    }

    fn preferences() -> FileTreePreferences {
        let mut visibility = FileTreeVisibility::default();
        visibility.hide(PathBuf::from("private")).unwrap();
        visibility.hide(PathBuf::from("a").join("b")).unwrap();
        FileTreePreferences {
            roots: BTreeMap::from([(root(), visibility)]),
        }
    }

    #[test]
    fn explicit_visibility_is_root_relative_and_includes_ancestors_not_name_prefixes() {
        let mut visibility = FileTreeVisibility::default();
        assert!(visibility.hide_hidden);
        visibility.hide(PathBuf::from("private")).unwrap();
        assert!(visibility.is_hidden(Path::new("private")));
        assert!(visibility.is_hidden(&PathBuf::from("private").join("nested").join("file")));
        assert!(!visibility.is_hidden(Path::new("private-copy")));
        assert!(!visibility.is_hidden(Path::new(".dotfile")));
        assert!(!visibility.unhide(&PathBuf::from("private").join("nested")));
        assert!(visibility.unhide(Path::new("private")));
        assert!(!visibility.is_hidden(Path::new("private")));
    }

    #[test]
    fn hide_rejects_empty_absolute_escaping_and_non_normalized_paths_without_mutation() {
        let mut visibility = FileTreeVisibility::default();
        for path in [
            "",
            ".",
            "..",
            "../escape",
            "a/../escape",
            "./a",
            "a/./b",
            "a//b",
            "a/",
        ] {
            assert!(visibility.hide(PathBuf::from(path)).is_err(), "{path:?}");
        }
        assert!(visibility.hide(root()).is_err());
        assert!(visibility.hidden_paths.is_empty());
    }

    #[test]
    fn preferences_roundtrip_without_filesystem_access() {
        let mut expected = preferences();
        expected.roots.get_mut(&root()).unwrap().hide_hidden = false;
        assert_eq!(
            FileTreePreferences::decode(&expected.encode().unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            FileTreePreferences::decode(&FileTreePreferences::default().encode().unwrap()).unwrap(),
            FileTreePreferences::default()
        );
    }

    #[test]
    fn decode_rejects_version_duplicate_root_and_duplicate_hidden_path() {
        let good: serde_json::Value =
            serde_json::from_slice(&preferences().encode().unwrap()).unwrap();
        let mut invalid = good.clone();
        invalid["version"] = 2.into();
        assert!(FileTreePreferences::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut invalid = good.clone();
        let record = invalid["roots"][0].clone();
        invalid["roots"].as_array_mut().unwrap().push(record);
        assert!(FileTreePreferences::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut invalid = good;
        let hidden = invalid["roots"][0]["hidden_paths"][0].clone();
        invalid["roots"][0]["hidden_paths"]
            .as_array_mut()
            .unwrap()
            .push(hidden);
        assert!(FileTreePreferences::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }

    #[test]
    fn decode_rejects_the_entire_document_for_invalid_paths_or_records() {
        let good: serde_json::Value =
            serde_json::from_slice(&preferences().encode().unwrap()).unwrap();
        for relative in [
            "",
            ".",
            "..",
            "../escape",
            "a/../escape",
            "a/./b",
            "a//b",
            "a/",
            "nul\0name",
        ] {
            let mut invalid = good.clone();
            invalid["roots"][0]["hidden_paths"][0] =
                serde_json::to_value(NativePath::encode(Path::new(relative)).unwrap()).unwrap();
            assert!(
                FileTreePreferences::decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
                "{relative:?}"
            );
        }
        for invalid_root in [
            PathBuf::from("relative"),
            root().join("..").join("escape"),
            root().join("a\0b"),
        ] {
            let mut invalid = good.clone();
            invalid["roots"][0]["root"] =
                serde_json::to_value(NativePath::encode(&invalid_root).unwrap()).unwrap();
            assert!(FileTreePreferences::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let mut invalid = good.clone();
        invalid["roots"][0]["hidden_paths"][0] =
            serde_json::to_value(NativePath::encode(&root()).unwrap()).unwrap();
        assert!(FileTreePreferences::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut invalid = good.clone();
        invalid["roots"][0]["root"]["platform"] = "foreign_platform".into();
        assert!(FileTreePreferences::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut invalid = good;
        invalid["roots"][0]["unexpected"] = true.into();
        assert!(FileTreePreferences::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        assert!(FileTreePreferences::decode(br#"{"version":1,"version":1,"roots":[]}"#).is_err());
        assert!(FileTreePreferences::decode(b"not json").is_err());
    }

    #[test]
    fn encode_rejects_invalid_public_fields_instead_of_repairing_them() {
        let mut invalid = preferences();
        invalid
            .roots
            .insert(PathBuf::from("relative"), FileTreeVisibility::default());
        assert!(invalid.encode().is_err());
        let mut invalid = preferences();
        invalid
            .roots
            .get_mut(&root())
            .unwrap()
            .hidden_paths
            .insert(PathBuf::from("../escape"));
        assert!(invalid.encode().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_unix_roots_and_hidden_paths_roundtrip_losslessly() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let root = PathBuf::from(OsString::from_vec(b"/project-\xff".to_vec()));
        let relative = PathBuf::from(OsString::from_vec(b"dir-\xfe/name-\x80".to_vec()));
        let mut visibility = FileTreeVisibility::default();
        visibility.hide(relative).unwrap();
        let expected = FileTreePreferences {
            roots: BTreeMap::from([(root, visibility)]),
        };
        assert_eq!(
            FileTreePreferences::decode(&expected.encode().unwrap()).unwrap(),
            expected
        );
    }

    #[cfg(windows)]
    #[test]
    fn unpaired_windows_surrogates_roundtrip_losslessly() {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt};
        let mut root_units: Vec<u16> = r"C:\project-".encode_utf16().collect();
        root_units.push(0xd800);
        let root = PathBuf::from(OsString::from_wide(&root_units));
        let relative = PathBuf::from(OsString::from_wide(&[b'f' as u16, 0xdc00]));
        let mut visibility = FileTreeVisibility::default();
        visibility.hide(relative).unwrap();
        let expected = FileTreePreferences {
            roots: BTreeMap::from([(root, visibility)]),
        };
        assert_eq!(
            FileTreePreferences::decode(&expected.encode().unwrap()).unwrap(),
            expected
        );
    }
}
