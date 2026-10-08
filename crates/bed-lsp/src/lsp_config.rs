//! Config and discovery helpers translated from ned lsp_client.cpp.
use serde_json::Value;
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanguageServerInfo {
    pub language: String,
    pub file_extensions: Vec<String>,
    pub server_paths: Vec<String>,
    pub server_args: Vec<String>,
}
#[derive(Clone, Debug, Default)]
pub struct LspConfig {
    pub language_servers: Vec<LanguageServerInfo>,
}
impl LspConfig {
    pub fn load(path: &Path) -> io::Result<Self> {
        Self::from_json(&read_json(path)?)
    }
    pub fn from_json(value: &Value) -> io::Result<Self> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid lsp.json language configuration",
            )
        };
        let languages = value
            .get("languages")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        let mut language_servers = Vec::new();
        for entry in languages {
            let (Some(language), Some(extensions), Some(paths)) = (
                entry.get("language_name"),
                entry.get("language_file_extensions"),
                entry.get("language_server_paths"),
            ) else {
                continue;
            };
            let language = language.as_str().ok_or_else(invalid)?.to_owned();
            let strings = |value: &Value| -> io::Result<Vec<String>> {
                value
                    .as_array()
                    .ok_or_else(invalid)?
                    .iter()
                    .map(|value| value.as_str().map(str::to_owned).ok_or_else(invalid))
                    .collect()
            };
            let server_args = if language == "typescript" || language == "python" {
                vec!["--stdio".into()]
            } else {
                Vec::new()
            };
            language_servers.push(LanguageServerInfo {
                language,
                file_extensions: strings(extensions)?,
                server_paths: strings(paths)?,
                server_args,
            });
        }
        Ok(Self { language_servers })
    }
    pub fn detect_language(&self, file_path: &str) -> String {
        let extension = Path::new(file_path)
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy().to_ascii_lowercase()))
            .unwrap_or_default();
        self.language_servers
            .iter()
            .find(|server| server.file_extensions.contains(&extension))
            .map(|server| server.language.clone())
            .unwrap_or_default()
    }
    pub fn find_server_path(&self, language: &str) -> Option<PathBuf> {
        resolve_server_paths(
            &self
                .language_servers
                .iter()
                .find(|server| server.language == language)?
                .server_paths,
        )
    }
    pub fn supported_languages(&self) -> Vec<String> {
        self.language_servers
            .iter()
            .map(|server| server.language.clone())
            .collect()
    }
}

/// Captured search inputs make discovery deterministic without changing the
/// process environment. Never canonicalize a rustup proxy into `rustup`.
#[derive(Clone, Debug, Default)]
pub struct ServerDiscoveryEnvironment {
    pub path: Option<OsString>,
    pub cargo_home: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub current_directory: PathBuf,
}
impl ServerDiscoveryEnvironment {
    pub fn current() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            cargo_home: std::env::var_os("CARGO_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            home: std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            current_directory: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
    pub fn cargo_bin(&self) -> Option<PathBuf> {
        self.cargo_home
            .clone()
            .or_else(|| self.home.as_ref().map(|home| home.join(".cargo")))
            .map(|home| self.absolute(&home).join("bin"))
    }
    fn absolute(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_owned()
        } else {
            self.current_directory.join(path)
        }
    }
}

pub fn resolve_server_paths(paths: &[String]) -> Option<PathBuf> {
    resolve_server_paths_with_environment(paths, &ServerDiscoveryEnvironment::current())
}
pub fn resolve_server_paths_with_environment(
    paths: &[String],
    environment: &ServerDiscoveryEnvironment,
) -> Option<PathBuf> {
    for configured in paths {
        let path = Path::new(configured);
        let literal = environment.absolute(path);
        // Keep upstream's explicit configured-file precedence. Spawn reports a
        // permission error for a configured non-executable, rather than hiding it.
        if literal.is_file() {
            return Some(literal);
        }
        if path.components().count() != 1 {
            continue;
        }
        let mut directories: Vec<PathBuf> = environment
            .path
            .as_deref()
            .map(std::env::split_paths)
            .map(Iterator::collect)
            .unwrap_or_default();
        if configured == "rust-analyzer"
            && let Some(bin) = environment.cargo_bin()
        {
            directories.push(bin);
        }
        for directory in directories {
            let candidate = environment.absolute(&directory).join(path);
            if executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}
fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

/// Ensure Cargo/rustc remain discoverable for a GUI-launched rust-analyzer.
/// Only the child receives this PATH; host environment and cwd are untouched.
pub fn server_child_path(program: &Path) -> io::Result<OsString> {
    let environment = ServerDiscoveryEnvironment::current();
    let mut directories = Vec::new();
    if let Some(parent) = program.parent() {
        directories.push(parent.to_owned());
    }
    if let Some(bin) = environment.cargo_bin() {
        directories.push(bin);
    }
    directories.extend(
        environment
            .path
            .as_deref()
            .map(std::env::split_paths)
            .into_iter()
            .flatten(),
    );
    let mut unique = Vec::new();
    for directory in directories {
        if !unique.contains(&directory) {
            unique.push(directory);
        }
    }
    std::env::join_paths(unique).map_err(io::Error::other)
}
fn read_json(path: &std::path::Path) -> std::io::Result<serde_json::Value> {
    let bytes = std::fs::read(path)?;
    serde_json::Deserializer::from_slice(&bytes)
        .into_iter::<serde_json::Value>()
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "Empty JSON file"))?
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn original_seven_language_config_and_extension_priority() {
        let value: Value =
            serde_json::from_str(include_str!("../../../resources/config/lsp.json")).unwrap();
        let config = LspConfig::from_json(&value).unwrap();
        assert_eq!(config.language_servers.len(), 7);
        assert_eq!(config.detect_language("A.CPP"), "cpp");
        assert_eq!(config.detect_language("module.js"), "typescript");
        assert_eq!(config.language_servers[1].server_args, vec!["--stdio"]);
        assert_eq!(config.language_servers[2].server_args, vec!["--stdio"]);
        assert!(config.language_servers[4].server_args.is_empty());
        assert_eq!(config.detect_language("unknown.txt"), "");
    }
    #[test]
    fn missing_keys_skip_and_type_errors_return_invalid_data() {
        assert_eq!(
            LspConfig::from_json(&json!({"languages":[{"language_name":"missing"}]}))
                .unwrap()
                .language_servers
                .len(),
            0
        );
        assert!(LspConfig::from_json(&json!({"languages":[{"language_name":42,"language_file_extensions":[],"language_server_paths":[]}]})).is_err());
    }
    #[test]
    fn explicit_discovery_keeps_configured_regular_file_order() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let config = LspConfig {
            language_servers: vec![LanguageServerInfo {
                language: "rust".into(),
                server_paths: vec![
                    root.to_string_lossy().into_owned(),
                    root.join("Cargo.toml").to_string_lossy().into_owned(),
                ],
                ..Default::default()
            }],
        };
        assert_eq!(
            config.find_server_path("rust"),
            Some(root.join("Cargo.toml"))
        );
        assert!(config.find_server_path("missing").is_none());
    }
    #[test]
    fn bare_servers_search_path_then_cargo_without_global_environment_changes() {
        use crate::test_support::TempDir;
        let temp = TempDir::new();
        let write_executable = |name: &str| {
            let path = temp.write(name, b"executable fixture");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            path
        };
        let first = write_executable("first/rust-analyzer");
        let second = write_executable("second/rust-analyzer");
        let cargo = write_executable("custom-cargo/bin/rust-analyzer");
        let environment = ServerDiscoveryEnvironment {
            path: Some(
                std::env::join_paths([first.parent().unwrap(), second.parent().unwrap()]).unwrap(),
            ),
            cargo_home: Some(temp.path("custom-cargo")),
            home: Some(temp.path("home")),
            current_directory: temp.root().to_owned(),
        };
        let paths = vec!["rust-analyzer".into()];
        assert_eq!(
            resolve_server_paths_with_environment(&paths, &environment),
            Some(first.clone())
        );
        std::fs::remove_file(first).unwrap();
        assert_eq!(
            resolve_server_paths_with_environment(&paths, &environment),
            Some(second.clone())
        );
        std::fs::remove_file(second).unwrap();
        assert_eq!(
            resolve_server_paths_with_environment(&paths, &environment),
            Some(cargo)
        );
        assert!(
            resolve_server_paths_with_environment(&["missing/server".into()], &environment)
                .is_none()
        );
        assert!(resolve_server_paths_with_environment(&["clangd".into()], &environment).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn rustup_proxy_keeps_its_dispatch_filename_and_gui_home_fallback() {
        use crate::test_support::TempDir;
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = TempDir::new();
        let target = temp.write("home/.cargo/bin/rustup", b"proxy fixture");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let proxy = temp.path("home/.cargo/bin/rust-analyzer");
        symlink("rustup", &proxy).unwrap();
        let environment = ServerDiscoveryEnvironment {
            home: Some(temp.path("home")),
            current_directory: temp.root().to_owned(),
            ..Default::default()
        };
        assert_eq!(
            resolve_server_paths_with_environment(&["rust-analyzer".into()], &environment),
            Some(proxy)
        );
        assert_eq!(
            resolve_server_paths_with_environment(
                &[
                    target.to_string_lossy().into_owned(),
                    "rust-analyzer".into()
                ],
                &environment
            ),
            Some(target)
        );
    }
}
