//! Layered language/server configuration and executable discovery.
use globset::{Glob, GlobMatcher};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io,
    path::{Component, Path, PathBuf},
};

const BUNDLED: &str = include_str!("../../../resources/config/lsp.json");
const LANGUAGE_FIELDS: &[&str] = &[
    "name",
    "language_id",
    "file_types",
    "shebangs",
    "roots",
    "workspace_lsp_roots",
    "language_server",
    "enabled",
];
const SERVER_FIELDS: &[&str] = &[
    "command",
    "args",
    "environment",
    "settings",
    "initialization_options",
    "timeout_secs",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileType {
    Extension(String),
    Glob(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct LanguageConfiguration {
    pub name: String,
    pub language_id: String,
    pub file_types: Vec<FileType>,
    pub shebangs: Vec<String>,
    pub roots: Vec<String>,
    pub workspace_lsp_roots: Vec<PathBuf>,
    pub language_server: Option<String>,
    pub enabled: bool,
}
impl Default for LanguageConfiguration {
    fn default() -> Self {
        Self {
            name: String::new(),
            language_id: String::new(),
            file_types: Vec::new(),
            shebangs: Vec::new(),
            roots: Vec::new(),
            workspace_lsp_roots: Vec::new(),
            language_server: None,
            enabled: true,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ServerConfiguration {
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub settings: Value,
    pub initialization_options: Option<Value>,
    pub timeout_secs: u64,
}
impl Default for ServerConfiguration {
    fn default() -> Self {
        Self {
            command: Vec::new(),
            args: Vec::new(),
            environment: BTreeMap::new(),
            settings: json!({}),
            initialization_options: None,
            timeout_secs: 20,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct LspConfig {
    pub languages: Vec<LanguageConfiguration>,
    pub language_servers: BTreeMap<String, ServerConfiguration>,
    compiled_globs: Vec<(usize, GlobMatcher)>,
}
impl PartialEq for LspConfig {
    fn eq(&self, other: &Self) -> bool {
        self.languages == other.languages && self.language_servers == other.language_servers
    }
}
impl LspConfig {
    /// Load a complete configuration without applying bundled defaults.
    pub fn load(path: &Path) -> io::Result<Self> {
        read_json(path)
            .and_then(|value| Self::from_json(&value))
            .map_err(|error| source_error(path, error))
    }
    pub fn from_json(value: &Value) -> io::Result<Self> {
        Self::parse(value)
    }
    /// Embedded defaults, optional user overrides, then optional project overrides.
    pub fn load_layered(user_path: &Path, project_path: Option<&Path>) -> io::Result<Self> {
        let read_optional = |path: &Path| match read_json(path) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(source_error(path, error)),
        };
        let user = read_optional(user_path)?;
        let project = project_path.map(read_optional).transpose()?.flatten();
        Self::from_layers(user.as_ref(), project.as_ref())
    }
    pub fn from_layers(user: Option<&Value>, project: Option<&Value>) -> io::Result<Self> {
        let mut effective: Value = serde_json::from_str(BUNDLED).expect("valid bundled LSP JSON");
        for (source, layer) in [("user", user), ("project", project)] {
            if let Some(layer) = layer {
                merge_configuration(&mut effective, layer)
                    .map_err(|error| invalid(format!("{source} overrides: {error}")))?;
                Self::parse(&effective)
                    .map_err(|error| invalid(format!("{source} overrides: {error}")))?;
            }
        }
        Self::parse(&effective)
    }
    fn parse(value: &Value) -> io::Result<Self> {
        let object = configuration_object(value)?;
        let rows = value
            .get("languages")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("expected languages array"))?;
        let mut config = Self::default();
        let mut names = BTreeSet::new();
        for entry in rows {
            let object = entry
                .as_object()
                .ok_or_else(|| invalid("language must be an object"))?;
            known_fields(object, LANGUAGE_FIELDS, "language")?;
            let name = required_string(object, "name")?;
            if !names.insert(name.clone()) {
                return Err(invalid(format!("duplicate language {name}")));
            }
            let language_id =
                optional_string(object, "language_id")?.unwrap_or_else(|| name.clone());
            let file_types = match object.get("file_types") {
                None => Vec::new(),
                Some(value) => value
                    .as_array()
                    .ok_or_else(|| invalid("file_types must be an array"))?
                    .iter()
                    .map(|item| {
                        if let Some(extension) = item.as_str() {
                            let extension = extension.trim_start_matches('.');
                            if extension.is_empty() || extension.contains(['/', '\\']) {
                                return Err(invalid(
                                    "file extension must be nonempty without separators",
                                ));
                            }
                            return Ok(FileType::Extension(extension.to_owned()));
                        }
                        let glob = item
                            .as_object()
                            .filter(|object| object.len() == 1)
                            .and_then(|object| object.get("glob"))
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                invalid("file type must be an extension string or {glob: string}")
                            })?;
                        Ok(FileType::Glob(glob.to_owned()))
                    })
                    .collect::<io::Result<Vec<_>>>()?,
            };
            let shebangs = optional_strings(object, "shebangs")?;
            let roots = optional_strings(object, "roots")?;
            for marker in &roots {
                Glob::new(marker).map_err(|error| invalid(format!("{name} root glob: {error}")))?;
            }
            let workspace_lsp_roots = optional_strings(object, "workspace_lsp_roots")?
                .into_iter()
                .map(|root| {
                    let normalized = root.replace('\\', "/");
                    let path = PathBuf::from(&normalized);
                    if path.is_absolute()
                        || normalized.as_bytes().get(1) == Some(&b':')
                        || path
                            .components()
                            .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
                    {
                        return Err(invalid(
                            "workspace_lsp_roots must remain inside the project",
                        ));
                    }
                    Ok(path)
                })
                .collect::<io::Result<Vec<_>>>()?;
            let language_server = optional_string(object, "language_server")?;
            let enabled = match object.get("enabled") {
                None => true,
                Some(value) => value
                    .as_bool()
                    .ok_or_else(|| invalid("enabled must be boolean"))?,
            };
            let index = config.languages.len();
            for file_type in &file_types {
                if let FileType::Glob(pattern) = file_type {
                    // Helix file globs match suffixes of full document paths.
                    let pattern = if pattern.starts_with('/') || pattern.starts_with("**/") {
                        pattern.clone()
                    } else {
                        format!("**/{pattern}")
                    };
                    let glob = Glob::new(&pattern)
                        .map_err(|error| invalid(format!("{name} file glob: {error}")))?;
                    config.compiled_globs.push((index, glob.compile_matcher()));
                }
            }
            config.languages.push(LanguageConfiguration {
                name,
                language_id,
                file_types,
                shebangs,
                roots,
                workspace_lsp_roots,
                language_server,
                enabled,
            });
        }
        if let Some(value) = object.get("language_servers") {
            let servers = value
                .as_object()
                .ok_or_else(|| invalid("language_servers must be an object"))?;
            for (name, value) in servers {
                let object = value
                    .as_object()
                    .ok_or_else(|| invalid(format!("server {name} must be an object")))?;
                known_fields(object, SERVER_FIELDS, &format!("server {name}"))?;
                let command = match object.get("command") {
                    Some(Value::String(command)) => vec![command.clone()],
                    Some(value) => strings(value, "command")?,
                    None => return Err(invalid(format!("server {name} requires command"))),
                };
                if command.is_empty()
                    || command
                        .iter()
                        .any(|command| command.is_empty() || command.contains('\0'))
                {
                    return Err(invalid(format!(
                        "server {name} command must contain nonempty executables without NUL"
                    )));
                }
                let args = optional_strings(object, "args")?;
                if args.iter().any(|arg| arg.contains('\0')) {
                    return Err(invalid("server arguments cannot contain NUL"));
                }
                let mut environment = BTreeMap::new();
                if let Some(value) = object.get("environment") {
                    for (key, value) in value
                        .as_object()
                        .ok_or_else(|| invalid("environment must be an object"))?
                    {
                        let value = value
                            .as_str()
                            .ok_or_else(|| invalid("environment values must be strings"))?;
                        if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                            return Err(invalid("invalid environment variable name/value"));
                        }
                        environment.insert(key.clone(), value.to_owned());
                    }
                }
                let settings = object.get("settings").cloned().unwrap_or_else(|| json!({}));
                if !settings.is_object() {
                    return Err(invalid("settings must be an object"));
                }
                let initialization_options = object
                    .get("initialization_options")
                    .filter(|value| !value.is_null())
                    .cloned();
                let timeout_secs = match object.get("timeout_secs") {
                    None => 20,
                    Some(value) => value
                        .as_u64()
                        .filter(|value| (1..=3600).contains(value))
                        .ok_or_else(|| invalid("timeout_secs must be an integer from 1 to 3600"))?,
                };
                config.language_servers.insert(
                    name.clone(),
                    ServerConfiguration {
                        command,
                        args,
                        environment,
                        settings,
                        initialization_options,
                        timeout_secs,
                    },
                );
            }
        }
        for language in &config.languages {
            if let Some(server) = &language.language_server
                && !config.language_servers.contains_key(server)
            {
                return Err(invalid(format!(
                    "language {} refers to missing server {server}",
                    language.name
                )));
            }
        }
        Ok(config)
    }
    pub fn detect_language(&self, file_path: &str) -> String {
        self.detect_language_with_content(Path::new(file_path), &[])
            .map(|language| language.name.clone())
            .unwrap_or_default()
    }
    pub fn detect_language_with_content(
        &self,
        path: impl AsRef<Path>,
        content: &[u8],
    ) -> Option<&LanguageConfiguration> {
        let normalized = path.as_ref().to_string_lossy().replace('\\', "/");
        for (index, matcher) in &self.compiled_globs {
            let language = &self.languages[*index];
            if language.enabled && matcher.is_match(&normalized) {
                return Some(language);
            }
        }
        if let Some(extension) = Path::new(&normalized)
            .extension()
            .and_then(|extension| extension.to_str())
        {
            let find = |extension: &str| {
                self.languages.iter().find(|language| {
                    language.enabled
                        && language.file_types.iter().any(
                            |kind| matches!(kind, FileType::Extension(value) if value == extension),
                        )
                })
            };
            if let Some(language) =
                find(extension).or_else(|| find(&extension.to_ascii_lowercase()))
            {
                return Some(language);
            }
        }
        let marker = shebang_marker(content)?;
        self.languages.iter().find(|language| {
            language.enabled && language.shebangs.iter().any(|value| value == &marker)
        })
    }
    pub fn language(&self, name: &str) -> Option<&LanguageConfiguration> {
        self.languages.iter().find(|language| language.name == name)
    }
    pub fn server_id(&self, language: &str) -> Option<&str> {
        self.language(language)
            .filter(|language| language.enabled)?
            .language_server
            .as_deref()
    }
    pub fn server(&self, name_or_language: &str) -> Option<&ServerConfiguration> {
        self.language_servers
            .get(name_or_language)
            .or_else(|| self.language_servers.get(self.server_id(name_or_language)?))
    }
    pub fn find_server_path(&self, name_or_language: &str) -> Option<PathBuf> {
        let server = self.server(name_or_language)?;
        let mut environment = ServerDiscoveryEnvironment::current();
        if let Some(path) = server.environment.get("PATH") {
            environment.path = Some(OsString::from(path));
        }
        resolve_server_paths_with_environment(&server.command, &environment)
    }
    pub fn supported_languages(&self) -> Vec<String> {
        self.languages
            .iter()
            .filter(|language| language.enabled && language.language_server.is_some())
            .map(|language| language.name.clone())
            .collect()
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Invalid lsp.json: {}", message.into()),
    )
}
fn source_error(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}
fn known_fields(object: &Map<String, Value>, fields: &[&str], kind: &str) -> io::Result<()> {
    for field in object.keys() {
        if !fields.contains(&field.as_str()) {
            return Err(invalid(format!("unknown {kind} field {field}")));
        }
    }
    Ok(())
}
fn configuration_object(value: &Value) -> io::Result<&Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("configuration must be an object"))?;
    known_fields(
        object,
        &["languages", "language_servers", "_provenance"],
        "configuration",
    )?;
    if object
        .get("_provenance")
        .is_some_and(|value| !value.is_object())
    {
        return Err(invalid("_provenance must be an object"));
    }
    Ok(object)
}
fn strings(value: &Value, field: &str) -> io::Result<Vec<String>> {
    value
        .as_array()
        .ok_or_else(|| invalid(format!("{field} must be an array")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(format!("{field} values must be strings")))
        })
        .collect()
}
fn optional_strings(object: &Map<String, Value>, field: &str) -> io::Result<Vec<String>> {
    object
        .get(field)
        .map(|value| strings(value, field))
        .transpose()
        .map(Option::unwrap_or_default)
}
fn required_string(object: &Map<String, Value>, field: &str) -> io::Result<String> {
    optional_string(object, field)?.ok_or_else(|| invalid(format!("{field} is required")))
}
fn optional_string(object: &Map<String, Value>, field: &str) -> io::Result<Option<String>> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.clone())),
        _ => Err(invalid(format!(
            "{field} must be a nonempty string or null"
        ))),
    }
}
fn shebang_marker(content: &[u8]) -> Option<String> {
    let first_line = content.split(|byte| *byte == b'\n').next()?;
    let line = std::str::from_utf8(first_line)
        .ok()?
        .strip_prefix("#!")?
        .trim();
    let mut words = line.split_whitespace();
    let first = words.next()?;
    let executable = first.rsplit(['/', '\\']).next()?;
    let executable = if executable == "env" {
        let mut command = None;
        while let Some(word) = words.next() {
            if matches!(word, "-u" | "--unset" | "-C" | "--chdir") {
                words.next();
                continue;
            }
            if word.starts_with('-') || word.contains('=') {
                continue;
            }
            command = Some(word);
            break;
        }
        command?.rsplit(['/', '\\']).next()?
    } else {
        executable
    };
    let marker = executable
        .trim_end_matches(|character: char| character.is_ascii_digit() || character == '.');
    (!marker.is_empty()).then(|| marker.to_owned())
}
fn merge_object(base: &mut Value, overrides: &Value) {
    if let (Some(base), Some(overrides)) = (base.as_object_mut(), overrides.as_object()) {
        for (key, value) in overrides {
            match base.get_mut(key) {
                Some(base) => merge_object(base, value),
                None => {
                    base.insert(key.clone(), value.clone());
                }
            }
        }
    } else {
        *base = overrides.clone();
    }
}
fn merge_configuration(base: &mut Value, overrides: &Value) -> io::Result<()> {
    let object = configuration_object(overrides)?;
    if let Some(rows) = object.get("languages") {
        let rows = rows
            .as_array()
            .ok_or_else(|| invalid("languages must be an array"))?;
        let base_rows = base["languages"]
            .as_array_mut()
            .expect("validated bundled languages array");
        let mut ordered = Vec::new();
        let mut seen = BTreeSet::new();
        for row in rows {
            let object = row
                .as_object()
                .ok_or_else(|| invalid("language must be an object"))?;
            known_fields(object, LANGUAGE_FIELDS, "language")?;
            let name = row
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("language name is required"))?;
            if !seen.insert(name) {
                return Err(invalid(format!("duplicate language {name}")));
            }
            let mut value = base_rows
                .iter()
                .find(|value| value.get("name").and_then(Value::as_str) == Some(name))
                .cloned()
                .unwrap_or_else(|| json!({}));
            merge_object(&mut value, row);
            ordered.push(value);
        }
        ordered.extend(
            base_rows
                .iter()
                .filter(|row| !seen.contains(row["name"].as_str().expect("validated name")))
                .cloned(),
        );
        *base_rows = ordered;
    }
    if let Some(servers) = object.get("language_servers") {
        if !servers.is_object() {
            return Err(invalid("language_servers must be an object"));
        }
        if base.get("language_servers").is_none() {
            base["language_servers"] = json!({});
        }
        merge_object(&mut base["language_servers"], servers);
    }
    Ok(())
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
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn expanded_catalog_uses_protocol_ids_and_has_no_zig() {
        let config = LspConfig::from_layers(None, None).unwrap();
        assert_eq!(config.languages.len(), 35);
        assert_eq!(config.supported_languages().len(), 35);
        for (path, name, id) in [
            ("main.c", "c", "c"),
            ("main.C", "cpp", "cpp"),
            ("main.h", "cpp", "cpp"),
            ("main.js", "javascript", "javascript"),
            ("main.jsx", "jsx", "javascriptreact"),
            ("main.tsx", "tsx", "typescriptreact"),
            ("go.mod", "gomod", "gomod"),
        ] {
            let language = config.detect_language_with_content(path, &[]).unwrap();
            assert_eq!((&*language.name, &*language.language_id), (name, id));
        }
        assert!(config.language("zig").is_none());
        assert!(config.server("zls").is_none());
        assert_eq!(config.detect_language("elsewhere.mod"), "");
        assert_eq!(config.server("bash").unwrap().args, ["start"]);
        assert_eq!(config.server("toml").unwrap().args, ["lsp", "stdio"]);
        assert_eq!(config.server("csharp").unwrap().args, ["--languageserver"]);
        assert_eq!(config.server("java").unwrap().timeout_secs, 60);
        assert_eq!(config.server_id("json"), config.server_id("jsonc"));
        assert_eq!(config.server_id("css"), config.server_id("less"));
    }
    #[test]
    fn layered_objects_merge_arrays_replace_and_defaults_stay_available() {
        let user = json!({"languages":[{"name":"rust","roots":["user.toml"]}], "language_servers":{"rust-analyzer":{"environment":{"A":"user","B":"user"},"args":["user"],"settings":{"check":{"command":"clippy","targets":["user"]}}}}});
        let project = json!({"languages":[{"name":"rust","roots":[],"workspace_lsp_roots":["crates/a"]}],"language_servers":{"rust-analyzer":{"environment":{"A":"project"},"args":[],"settings":{"check":{"targets":[]}}}}});
        let config = LspConfig::from_layers(Some(&user), Some(&project)).unwrap();
        let rust = config.language("rust").unwrap();
        assert!(rust.roots.is_empty());
        assert_eq!(rust.workspace_lsp_roots, [PathBuf::from("crates/a")]);
        let server = config.server("rust").unwrap();
        assert!(server.args.is_empty());
        assert_eq!(
            server.environment,
            BTreeMap::from([("A".into(), "project".into()), ("B".into(), "user".into())])
        );
        assert_eq!(
            server.settings["check"],
            json!({"command":"clippy","targets":[]})
        );
        assert_eq!(
            server.settings["rust-analyzer"]["files"]["watcher"],
            "server"
        );
        assert!(config.language("lua").is_some());
    }
    #[test]
    fn language_override_layers_control_detection_conflicts() {
        let user = json!({"languages":[{"name":"first","file_types":["rs",{"glob":"*.custom"}]},{"name":"second","file_types":["rs",{"glob":"*.custom"}]}]});
        let project = json!({"languages":[{"name":"second"}]});
        let config = LspConfig::from_layers(Some(&user), Some(&project)).unwrap();
        assert_eq!(config.detect_language("test.rs"), "second");
        assert_eq!(config.detect_language("test.custom"), "second");
        let user_only = LspConfig::from_layers(Some(&user), None).unwrap();
        assert_eq!(user_only.detect_language("test.rs"), "first");
    }
    #[test]
    fn disables_and_null_associations_remain_distinct() {
        let config = LspConfig::from_layers(Some(&json!({"languages":[{"name":"rust","enabled":false},{"name":"python","language_server":null}]})), None).unwrap();
        assert_eq!(config.detect_language("main.rs"), "");
        assert_eq!(config.detect_language("main.py"), "python");
        assert!(config.server_id("rust").is_none());
        assert!(config.server_id("python").is_none());
    }
    #[test]
    fn filename_globs_precede_extensions_and_support_braces_and_separators() {
        let config = LspConfig::from_layers(None, None).unwrap();
        for (path, expected) in [
            ("/project/tsconfig.json", "jsonc"),
            ("/project/jsconfig.json", "jsonc"),
            ("C:\\project\\.vscode\\settings.json", "jsonc"),
            ("/project/CMakeLists.txt", "cmake"),
            ("/project/.bashrc", "bash"),
            ("/project/Dockerfile.release", "dockerfile"),
            ("/project/go.mod", "gomod"),
            ("/project/main.js.map", "json"),
            ("/project/src/header.hpp.in", "cpp"),
            ("/project/src/header.h.in", "cpp"),
        ] {
            assert_eq!(config.detect_language(path), expected, "{path}");
        }
    }
    #[test]
    fn shebangs_use_document_content_and_env_arguments() {
        let config = LspConfig::from_layers(None, None).unwrap();
        for bytes in [
            b"#!/usr/bin/python3.12\nprint(1)".as_slice(),
            b"#!/usr/bin/env -S python3 -u\nprint(1)",
            b"#!/usr/bin/env -u PYTHONPATH LANG=C python3\n",
        ] {
            assert_eq!(
                config
                    .detect_language_with_content("script", bytes)
                    .unwrap()
                    .name,
                "python"
            );
        }
        assert_eq!(
            config
                .detect_language_with_content("script", b"#!/usr/bin/env bash\n")
                .unwrap()
                .name,
            "bash"
        );
        assert_eq!(
            config
                .detect_language_with_content("script.rs", b"#!/usr/bin/python3\n")
                .unwrap()
                .name,
            "rust"
        );
        assert!(
            config
                .detect_language_with_content("script", b"#!\xff")
                .is_none()
        );
    }
    #[test]
    fn snapshots_are_exact_and_do_not_infer_server_arguments_or_settings() {
        let empty = LspConfig::from_json(&json!({"languages":[]})).unwrap();
        assert_eq!(empty, LspConfig::default());
        assert!(LspConfig::from_json(&json!({})).is_err());
        let config = LspConfig::from_json(&json!({
            "languages":[{"name":"python","file_types":["py"],"language_server":"custom"}],
            "language_servers":{"custom":{"command":"pyright"}}
        }))
        .unwrap();
        assert_eq!(config.languages.len(), 1);
        assert_eq!(config.detect_language("script.py"), "python");
        assert_eq!(config.detect_language("main.rs"), "");
        assert_eq!(config.server_id("python"), Some("custom"));
        let server = config.server("python").unwrap();
        assert!(server.args.is_empty());
        assert_eq!(server.settings, json!({}));
        assert_eq!(server.initialization_options, None);
    }
    #[test]
    fn empty_layered_overrides_preserve_the_bundled_catalog() {
        let defaults = LspConfig::from_layers(None, None).unwrap();
        for layer in [json!({}), json!({"languages":[],"language_servers":{}})] {
            assert_eq!(
                LspConfig::from_layers(Some(&layer), Some(&layer)).unwrap(),
                defaults
            );
        }
    }
    #[test]
    fn unknown_fields_fail_at_the_configuration_language_and_server_boundaries() {
        for (value, field) in [
            (
                json!({"languages":[],"languageServers":{}}),
                "languageServers",
            ),
            (
                json!({"languages":[{"name":"rust","extensions":["rs"]}]}),
                "extensions",
            ),
            (
                json!({"languages":[{"language_name":"rust"}]}),
                "language_name",
            ),
            (
                json!({"languages":[],"language_servers":{"fixture":{"command":"server","initializationOptions":{}}}}),
                "initializationOptions",
            ),
        ] {
            for result in [
                LspConfig::from_json(&value),
                LspConfig::from_layers(Some(&value), None),
            ] {
                let error = result.unwrap_err();
                assert!(error.to_string().contains(field), "{error}");
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            }
        }
        let settings = json!({"languages":[],"language_servers":{"fixture":{"command":"server",
            "settings":{"arbitraryServerSection":{"anything":true}},
            "initialization_options":{"anything":[1,2]}}}});
        assert!(LspConfig::from_json(&settings).is_ok());
    }
    #[test]
    fn malformed_boundaries_reject_bad_paths_types_and_launch_parameters() {
        for root in ["../outside", "/outside", "C:\\outside", "..\\outside"] {
            assert!(
                LspConfig::from_layers(
                    Some(&json!({"languages":[{"name":"rust","workspace_lsp_roots":[root]}]})),
                    None
                )
                .is_err(),
                "{root}"
            );
        }
        for user in [
            json!({"languages":null}),
            json!({"languages":[{"name":"rust","file_types":[{"glob":"["}]}]}),
            json!({"languages":[{"name":"rust","language_server":"missing"}]}),
            json!({"language_servers":{"bad":{"command":[]}}}),
            json!({"language_servers":{"bad":{"command":"server","environment":{"A=B":"value"}}}}),
            json!({"language_servers":{"bad":{"command":"server","args":["\u{0000}"]}}}),
            json!({"language_servers":{"bad":{"command":"server","timeout_secs":0}}}),
            json!({"language_servers":{"bad":{"command":"server","timeout_secs":u64::MAX}}}),
            json!({"language_servers":{"bad":{"command":"server","settings":null}}}),
        ] {
            assert!(LspConfig::from_layers(Some(&user), None).is_err(), "{user}");
        }
    }
    #[test]
    fn invalid_user_configuration_cannot_be_hidden_by_project_overrides() {
        let user = json!({"language_servers":{"rust-analyzer":{"timeout_secs":0}}});
        let project = json!({"language_servers":{"rust-analyzer":{"timeout_secs":30}}});
        let error = LspConfig::from_layers(Some(&user), Some(&project)).unwrap_err();
        assert!(error.to_string().contains("user overrides"));
    }
    #[test]
    fn optional_files_missing_are_defaults_and_parse_errors_name_the_source() {
        let temp = TempDir::new();
        let absent = temp.path("absent.json");
        assert_eq!(
            LspConfig::load_layered(&absent, None)
                .unwrap()
                .languages
                .len(),
            35
        );
        let malformed = temp.write("malformed.json", b"{} {}");
        let error = LspConfig::load_layered(&malformed, None).unwrap_err();
        assert!(error.to_string().contains("malformed.json"));
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
    #[test]
    fn configured_environment_path_is_used_for_executable_discovery() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new();
        let executable = temp.write("tools/custom-server", b"fixture");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = LspConfig::from_json(&json!({"languages":[{"name":"custom","language_server":"fixture"}],"language_servers":{"fixture":{"command":"custom-server","environment":{"PATH":executable.parent().unwrap()}}}})).unwrap();
        assert_eq!(config.find_server_path("custom"), Some(executable));
    }
    #[test]
    fn explicit_discovery_keeps_configured_regular_file_order() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let config = LspConfig::from_json(&json!({"languages":[{"name":"rust","language_server":"fixture"}],"language_servers":{"fixture":{"command":[root,root.join("Cargo.toml")]}}})).unwrap();
        assert_eq!(
            config.find_server_path("rust"),
            Some(root.join("Cargo.toml"))
        );
        assert!(config.find_server_path("missing").is_none());
    }
    #[test]
    fn bare_servers_search_path_then_cargo_without_global_environment_changes() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new();
        let write_executable = |name: &str| {
            let path = temp.write(name, b"fixture");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
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
    #[test]
    fn rustup_proxy_keeps_its_dispatch_filename_and_gui_home_fallback() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = TempDir::new();
        let target = temp.write("home/.cargo/bin/rustup", b"fixture");
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
