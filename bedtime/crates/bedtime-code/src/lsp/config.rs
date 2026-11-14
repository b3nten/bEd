//! Which language servers there are, and how they're configured. Servers
//! are named (`rust-analyzer`); the known ones (Helix's defaults, for the
//! languages there's syntax for) are built in, and the app's settings change
//! them or add more, field by field, in layers (`ServerSettings`: anything
//! left out comes from the layer below, then the built-in). Each language is
//! served by the first enabled server for it whose program is found (on the
//! login shell's PATH): `LanguageServers` is worked out from
//! `ServerCatalog` and what's installed, and servers whose configuration
//! changed restart with the new one.
use super::{LanguageServers, ServerConfig, environment};
use bedtime_workbench::{ManagedTask, ManagedWork};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

/// A built-in server.
#[derive(Debug, PartialEq)]
pub struct KnownServer {
    pub name: &'static str,
    pub program: &'static str,
    pub arguments: &'static [&'static str],
    /// Syntax's language names.
    pub languages: &'static [&'static str],
    /// Where it comes from, for installing it.
    pub home: &'static str,
}

/// The built-in servers, in order of preference for each language.
pub const KNOWN_SERVERS: &[KnownServer] = &[
    KnownServer {
        name: "rust-analyzer",
        program: "rust-analyzer",
        arguments: &[],
        languages: &["rust"],
        home: "https://rust-analyzer.github.io",
    },
    KnownServer {
        name: "basedpyright",
        program: "basedpyright-langserver",
        arguments: &["--stdio"],
        languages: &["python"],
        home: "https://docs.basedpyright.com",
    },
    KnownServer {
        name: "pyright",
        program: "pyright-langserver",
        arguments: &["--stdio"],
        languages: &["python"],
        home: "https://github.com/microsoft/pyright",
    },
    KnownServer {
        name: "ruff",
        program: "ruff",
        arguments: &["server"],
        languages: &["python"],
        home: "https://docs.astral.sh/ruff",
    },
    KnownServer {
        name: "typescript-language-server",
        program: "typescript-language-server",
        arguments: &["--stdio"],
        languages: &["typescript", "tsx", "javascript"],
        home: "https://github.com/typescript-language-server/typescript-language-server",
    },
    KnownServer {
        name: "vscode-css-language-server",
        program: "vscode-css-language-server",
        arguments: &["--stdio"],
        languages: &["css"],
        home: "https://github.com/hrsh7th/vscode-langservers-extracted",
    },
    KnownServer {
        name: "vscode-html-language-server",
        program: "vscode-html-language-server",
        arguments: &["--stdio"],
        languages: &["html"],
        home: "https://github.com/hrsh7th/vscode-langservers-extracted",
    },
    KnownServer {
        name: "vscode-json-language-server",
        program: "vscode-json-language-server",
        arguments: &["--stdio"],
        languages: &["json"],
        home: "https://github.com/hrsh7th/vscode-langservers-extracted",
    },
    KnownServer {
        name: "gopls",
        program: "gopls",
        arguments: &[],
        languages: &["go"],
        home: "https://go.dev/gopls",
    },
    KnownServer {
        name: "clangd",
        program: "clangd",
        arguments: &[],
        languages: &["c", "cpp"],
        home: "https://clangd.llvm.org",
    },
    KnownServer {
        name: "sourcekit-lsp",
        program: "sourcekit-lsp",
        arguments: &[],
        languages: &["swift"],
        home: "https://github.com/swiftlang/sourcekit-lsp",
    },
    KnownServer {
        name: "lua-language-server",
        program: "lua-language-server",
        arguments: &[],
        languages: &["lua"],
        home: "https://luals.github.io",
    },
    KnownServer {
        name: "bash-language-server",
        program: "bash-language-server",
        arguments: &["start"],
        languages: &["bash"],
        home: "https://github.com/bash-lsp/bash-language-server",
    },
    KnownServer {
        name: "yaml-language-server",
        program: "yaml-language-server",
        arguments: &["--stdio"],
        languages: &["yaml"],
        home: "https://github.com/redhat-developer/yaml-language-server",
    },
    KnownServer {
        name: "marksman",
        program: "marksman",
        arguments: &["server"],
        languages: &["markdown"],
        home: "https://github.com/artempyanykh/marksman",
    },
    KnownServer {
        name: "taplo",
        program: "taplo",
        arguments: &["lsp", "stdio"],
        languages: &["toml"],
        home: "https://taplo.tamasfe.dev",
    },
    KnownServer {
        name: "dockerfile-language-server",
        program: "docker-langserver",
        arguments: &["--stdio"],
        languages: &["dockerfile"],
        home: "https://github.com/rcjsuen/dockerfile-language-server",
    },
    KnownServer {
        name: "terraform-ls",
        program: "terraform-ls",
        arguments: &["serve"],
        languages: &["hcl"],
        home: "https://github.com/hashicorp/terraform-ls",
    },
    KnownServer {
        name: "cmake-language-server",
        program: "cmake-language-server",
        arguments: &[],
        languages: &["cmake"],
        home: "https://github.com/regen100/cmake-language-server",
    },
    KnownServer {
        name: "jdtls",
        program: "jdtls",
        arguments: &[],
        languages: &["java"],
        home: "https://github.com/eclipse-jdtls/eclipse.jdt.ls",
    },
    KnownServer {
        name: "kotlin-language-server",
        program: "kotlin-language-server",
        arguments: &[],
        languages: &["kotlin"],
        home: "https://github.com/fwcd/kotlin-language-server",
    },
    KnownServer {
        name: "csharp-ls",
        program: "csharp-ls",
        arguments: &[],
        languages: &["c-sharp"],
        home: "https://github.com/razzmatazz/csharp-language-server",
    },
    KnownServer {
        name: "solargraph",
        program: "solargraph",
        arguments: &["stdio"],
        languages: &["ruby"],
        home: "https://solargraph.org",
    },
    KnownServer {
        name: "intelephense",
        program: "intelephense",
        arguments: &["--stdio"],
        languages: &["php"],
        home: "https://intelephense.com",
    },
    KnownServer {
        name: "dart",
        program: "dart",
        arguments: &["language-server", "--client-id", "bed"],
        languages: &["dart"],
        home: "https://dart.dev",
    },
];

/// One layer's changes to a server: anything `None` comes from below.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSettings {
    pub enabled: Option<bool>,
    pub program: Option<String>,
    pub arguments: Option<Vec<String>>,
    pub environment: Option<BTreeMap<String, String>>,
    /// `workspace/configuration` answers (rust-analyzer's own settings, say).
    pub settings: Option<serde_json::Value>,
    pub initialization_options: Option<serde_json::Value>,
    /// The languages it serves (syntax's names).
    pub languages: Option<Vec<String>>,
}

impl ServerSettings {
    /// `above`'s fields over these.
    fn merge(&mut self, above: &ServerSettings) {
        fn over<T: Clone>(field: &mut Option<T>, above: &Option<T>) {
            if above.is_some() {
                field.clone_from(above);
            }
        }
        over(&mut self.enabled, &above.enabled);
        over(&mut self.program, &above.program);
        over(&mut self.arguments, &above.arguments);
        over(&mut self.environment, &above.environment);
        over(&mut self.settings, &above.settings);
        over(
            &mut self.initialization_options,
            &above.initialization_options,
        );
        over(&mut self.languages, &above.languages);
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A server as configured, after every layer.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerEntry {
    pub name: String,
    /// Built in (else added by the user).
    pub known: Option<&'static KnownServer>,
    pub enabled: bool,
    pub program: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub settings: serde_json::Value,
    pub initialization_options: serde_json::Value,
    pub languages: Vec<String>,
}

impl ServerEntry {
    fn from_settings(
        name: &str,
        known: Option<&'static KnownServer>,
        settings: ServerSettings,
    ) -> Self {
        Self {
            name: name.to_owned(),
            known,
            enabled: settings.enabled.unwrap_or(true),
            program: settings
                .program
                .or_else(|| known.map(|known| known.program.to_owned()))
                .unwrap_or_else(|| name.to_owned()),
            arguments: settings.arguments.unwrap_or_else(|| {
                known.map_or_else(Vec::new, |known| {
                    known.arguments.iter().map(|a| (*a).to_owned()).collect()
                })
            }),
            environment: settings.environment.unwrap_or_default(),
            settings: settings.settings.unwrap_or_else(|| serde_json::json!({})),
            initialization_options: settings
                .initialization_options
                .unwrap_or(serde_json::Value::Null),
            languages: settings.languages.unwrap_or_else(|| {
                known.map_or_else(Vec::new, |known| {
                    known.languages.iter().map(|l| (*l).to_owned()).collect()
                })
            }),
        }
    }
}

/// The built-in servers changed by `layers` (lowest first), then servers
/// the layers add, by name.
pub fn resolve<'a>(
    layers: impl IntoIterator<Item = &'a BTreeMap<String, ServerSettings>>,
) -> Vec<ServerEntry> {
    let mut merged: BTreeMap<String, ServerSettings> = BTreeMap::new();
    for layer in layers {
        for (name, settings) in layer {
            merged.entry(name.clone()).or_default().merge(settings);
        }
    }
    let mut entries: Vec<ServerEntry> = KNOWN_SERVERS
        .iter()
        .map(|known| {
            ServerEntry::from_settings(
                known.name,
                Some(known),
                merged.remove(known.name).unwrap_or_default(),
            )
        })
        .collect();
    entries.extend(
        merged
            .into_iter()
            .map(|(name, settings)| ServerEntry::from_settings(&name, None, settings)),
    );
    entries
}

/// The servers as configured: the app inserts it (`ServerCatalog::default()`
/// is the built-in servers as they are) and keeps it up to date from its
/// settings; `LanguageServers` then follows it.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct ServerCatalog(pub Vec<ServerEntry>);

impl Default for ServerCatalog {
    fn default() -> Self {
        Self(resolve([]))
    }
}

/// Where each program was found (or not), on the login shell's PATH.
#[derive(Resource, Default)]
pub struct InstalledServers {
    found: HashMap<String, Option<PathBuf>>,
    looking: Option<ManagedTask<HashMap<String, Option<PathBuf>>>>,
}

impl InstalledServers {
    /// `None` while it's being looked for.
    pub fn program(&self, program: &str) -> Option<Option<&PathBuf>> {
        self.found.get(program).map(Option::as_ref)
    }

    /// Look again (something was installed).
    pub fn forget(&mut self) {
        self.found.clear();
    }
}

/// Look for programs not yet looked for; work out `LanguageServers` again
/// when what's configured or found changes. Without a `ServerCatalog` (the
/// app sets `LanguageServers` itself) this does nothing.
pub(super) fn update_servers(world: &mut World) {
    if !world.contains_resource::<ServerCatalog>() {
        return;
    }
    // Finished looking.
    let finished = world
        .resource::<InstalledServers>()
        .looking
        .as_ref()
        .and_then(|task| task.try_recv().ok());
    if let Some(found) = finished {
        let mut installed = world.resource_mut::<InstalledServers>();
        installed.looking = None;
        installed.found.extend(found);
    }
    let unknown: Vec<String> = {
        let installed = world.resource::<InstalledServers>();
        let mut programs: Vec<String> = world
            .resource::<ServerCatalog>()
            .0
            .iter()
            .map(|entry| entry.program.clone())
            .filter(|program| !installed.found.contains_key(program))
            .collect();
        programs.sort();
        programs.dedup();
        programs
    };
    if !unknown.is_empty() && world.resource::<InstalledServers>().looking.is_none() {
        let task = world.resource_mut::<ManagedWork>().spawn_task(
            None,
            "Find language servers",
            move |_| {
                let environment = environment::shell_environment();
                unknown
                    .into_iter()
                    .map(|program| {
                        let found = environment::find_program(&program, environment);
                        (program, found)
                    })
                    .collect()
            },
        );
        if let Ok(task) = task {
            world.resource_mut::<InstalledServers>().looking = Some(task);
        }
    }
    if !world.is_resource_changed::<ServerCatalog>()
        && !world.is_resource_changed::<InstalledServers>()
    {
        return;
    }
    let servers = language_servers(
        &world.resource::<ServerCatalog>().0,
        world.resource::<InstalledServers>(),
    );
    if world.resource::<LanguageServers>().0 != servers {
        world.resource_mut::<LanguageServers>().0 = servers;
    }
}

/// Each language's server: the first enabled one for it that's installed.
fn language_servers(
    entries: &[ServerEntry],
    installed: &InstalledServers,
) -> BTreeMap<String, ServerConfig> {
    let mut servers = BTreeMap::new();
    for entry in entries.iter().filter(|entry| entry.enabled) {
        let Some(Some(program)) = installed.program(&entry.program) else {
            continue;
        };
        for language in &entry.languages {
            servers
                .entry(language.clone())
                .or_insert_with(|| config(entry, program));
        }
    }
    servers
}

/// How to launch `entry` (found at `program`): in the login shell's
/// environment (read already: the program was found with it), changed by
/// its own.
fn config(entry: &ServerEntry, program: &std::path::Path) -> ServerConfig {
    let mut environment: BTreeMap<String, String> = ["PATH", "HOME", "LANG"]
        .into_iter()
        .filter_map(|name| {
            environment::shell_environment()
                .get(name)
                .map(|value| (name.to_owned(), value.clone()))
        })
        .collect();
    environment.extend(entry.environment.clone());
    ServerConfig {
        name: entry.name.clone(),
        program: program.to_string_lossy().into_owned(),
        arguments: entry.arguments.clone(),
        environment,
        settings: entry.settings.clone(),
        initialization_options: entry.initialization_options.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_change_built_in_servers_field_by_field_and_add_others() {
        let global: BTreeMap<String, ServerSettings> = [
            (
                "rust-analyzer".to_owned(),
                ServerSettings {
                    arguments: Some(vec!["--log".into()]),
                    ..default()
                },
            ),
            (
                "my-lsp".to_owned(),
                ServerSettings {
                    program: Some("/opt/my-lsp".into()),
                    languages: Some(vec!["lua".into()]),
                    ..default()
                },
            ),
        ]
        .into();
        let workspace: BTreeMap<String, ServerSettings> = [(
            "rust-analyzer".to_owned(),
            ServerSettings {
                enabled: Some(false),
                ..default()
            },
        )]
        .into();
        let entries = resolve([&global, &workspace]);
        let rust = entries.iter().find(|e| e.name == "rust-analyzer").unwrap();
        assert!(!rust.enabled);
        assert_eq!(rust.arguments, ["--log"]);
        assert_eq!(rust.program, "rust-analyzer");
        let mine = entries.last().unwrap();
        assert_eq!(mine.name, "my-lsp");
        assert!(mine.known.is_none() && mine.enabled);
        assert_eq!(mine.languages, ["lua"]);
    }

    #[test]
    fn each_language_gets_its_first_enabled_installed_server() {
        let mut entries = resolve([]);
        let mut installed = InstalledServers::default();
        for entry in &entries {
            installed.found.insert(entry.program.clone(), None);
        }
        installed.found.insert(
            "pyright-langserver".into(),
            Some("/bin/pyright-langserver".into()),
        );
        installed
            .found
            .insert("ruff".into(), Some("/bin/ruff".into()));
        let servers = language_servers(&entries, &installed);
        // basedpyright isn't installed; pyright comes before ruff.
        assert_eq!(servers["python"].name, "pyright");
        assert!(!servers.contains_key("rust"));
        entries
            .iter_mut()
            .find(|e| e.name == "pyright")
            .unwrap()
            .enabled = false;
        assert_eq!(
            language_servers(&entries, &installed)["python"].name,
            "ruff"
        );
    }
}
