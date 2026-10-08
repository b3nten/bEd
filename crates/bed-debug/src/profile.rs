use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A local workspace's remembered launch configuration. Paths may be relative
/// to that workspace; runtime objects never write this configuration themselves.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct DebugProfile {
    pub name: String,
    pub program: String,
    pub build_command: String,
    pub cargo: Option<CargoLaunch>,
    pub args: Vec<String>,
    pub test_filter: String,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub source_map: Vec<[String; 2]>,
    pub stop_on_entry: bool,
}
impl Default for DebugProfile {
    fn default() -> Self {
        Self {
            name: "Debug".into(),
            program: String::new(),
            build_command: String::new(),
            cargo: None,
            args: Vec::new(),
            test_filter: String::new(),
            cwd: String::new(),
            env: BTreeMap::new(),
            source_map: Vec::new(),
            stop_on_entry: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct CargoLaunch {
    pub manifest_path: String,
    pub package: String,
    pub target: CargoTarget,
    pub features: Vec<String>,
    pub default_features: bool,
}
impl Default for CargoLaunch {
    fn default() -> Self {
        Self {
            manifest_path: "Cargo.toml".into(),
            package: String::new(),
            target: CargoTarget::default(),
            features: Vec::new(),
            default_features: true,
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CargoTarget {
    pub name: String,
    pub kind: CargoTargetKind,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CargoTargetKind {
    #[default]
    Binary,
    Example,
    LibraryTests,
    BinaryTests,
    IntegrationTest,
}
impl CargoTargetKind {
    pub fn is_test(self) -> bool {
        matches!(
            self,
            Self::LibraryTests | Self::BinaryTests | Self::IntegrationTest
        )
    }
    pub fn cargo_kind(self) -> &'static str {
        match self {
            Self::Binary | Self::BinaryTests => "bin",
            Self::Example => "example",
            Self::LibraryTests => "lib",
            Self::IntegrationTest => "test",
        }
    }
    pub fn matches_cargo_kind(self, kind: &str) -> bool {
        if self == Self::LibraryTests {
            matches!(
                kind,
                "lib" | "rlib" | "dylib" | "staticlib" | "cdylib" | "proc-macro"
            )
        } else {
            kind == self.cargo_kind()
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Binary => "Binary",
            Self::Example => "Example",
            Self::LibraryTests => "Library tests",
            Self::BinaryTests => "Binary tests",
            Self::IntegrationTest => "Integration test",
        }
    }
}
