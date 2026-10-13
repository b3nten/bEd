//! Windows without explicit launch paths use the Startup layout.
//! Explicit launch paths take precedence and only exact saved roots attach.
use crate::application::WorkspaceSpec;
use crate::config::LayoutSlot;
use bed_api::launch::ApplicationPaths;
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(crate) struct StartupOptions {
    pub cwd: Option<PathBuf>,
    pub new_window: bool,
    pub requested_workspace: Option<WorkspaceSpec>,
}

impl StartupOptions {
    pub fn resolve(self, paths: Vec<PathBuf>, process_cwd: Option<&Path>) -> io::Result<Startup> {
        let mut workspace = None;
        let mut files = Vec::new();
        for path in paths {
            let path = if path.is_absolute() {
                path
            } else {
                process_cwd
                    .ok_or_else(|| io::Error::other("Cannot resolve a relative launch path"))?
                    .join(path)
            };
            if path.is_dir() {
                workspace = Some(path.canonicalize()?);
            } else {
                files.push(path);
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let directory = launch_directory(
            workspace.as_deref().or(self.cwd.as_deref()),
            home.as_deref(),
        )?;
        let explicit_directory = workspace.is_some() || self.cwd.is_some();
        Ok(Startup {
            workspace: explicit_directory.then(|| directory.clone()),
            directory,
            files,
            requested_workspace: self.requested_workspace,
        })
    }
}

fn launch_directory(explicit: Option<&Path>, home: Option<&Path>) -> io::Result<PathBuf> {
    if let Some(path) = explicit {
        let directory = std::fs::canonicalize(path)?;
        if !directory.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Launch directory must name a directory",
            ));
        }
        return Ok(directory);
    }
    home.and_then(|path| path.canonicalize().ok())
        .filter(|path| path.is_dir())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "No usable launch directory; set HOME or pass --cwd",
            )
        })
}

pub(crate) struct Startup {
    directory: PathBuf,
    workspace: Option<PathBuf>,
    files: Vec<PathBuf>,
    requested_workspace: Option<WorkspaceSpec>,
}

impl Startup {
    pub fn into_options(
        self,
        paths: &ApplicationPaths,
    ) -> io::Result<crate::application::AppOptions> {
        use crate::application::{AppOptions, OpenFileOptions};
        let store = if cfg!(feature = "workspaces") {
            crate::application::workspace::store::WorkspaceStore::load(paths)?
        } else {
            crate::application::workspace::store::WorkspaceStore::empty(paths)
        };
        let workspace = if !cfg!(feature = "workspaces") {
            if self.requested_workspace.is_some() {
                return Err(io::Error::other("Workspace support is disabled"));
            }
            None
        } else if let Some(spec) = self.requested_workspace {
            Some(spec)
        } else if let Some(root) = self.workspace.as_ref() {
            store.stored_spec(&WorkspaceSpec::local(root.to_string_lossy()))?
        } else {
            None
        };
        let initial_layout =
            workspace
                .is_none()
                .then_some(if self.workspace.is_some() || !self.files.is_empty() {
                    LayoutSlot::Layout1
                } else {
                    LayoutSlot::Startup
                });
        let options = AppOptions {
            working_directory: Some(self.directory),
            workspace,
            initial_layout,
            files: self
                .files
                .into_iter()
                .map(|path| OpenFileOptions {
                    path,
                    viewer: None,
                    additional: false,
                })
                .collect(),
            ..Default::default()
        };
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::WorkspaceTarget;
    use crate::application::workspace::store::WorkspaceStore;
    use crate::config::Settings;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Fixture {
        root: PathBuf,
        settings: Settings,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-startup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let settings =
                Settings::with_paths(root.join("config"), Settings::get_app_resources_path())
                    .unwrap();
            Self { root, settings }
        }
        fn project(&self, name: &str) -> WorkspaceSpec {
            let root = self.root.join(name);
            std::fs::create_dir_all(&root).unwrap();
            let mut store = WorkspaceStore::load(
                &ApplicationPaths::new("bed", Some(self.settings.config_dir.clone()), None)
                    .unwrap(),
            )
            .unwrap();
            let spec = store
                .record_workspace(WorkspaceSpec::local(root.to_str().unwrap()))
                .unwrap();
            store
                .set_layout(&spec, serde_json::json!({"version":2,"panels":[]}))
                .unwrap();
            spec
        }
        fn options(
            &self,
            options: StartupOptions,
            paths: Vec<PathBuf>,
        ) -> crate::application::AppOptions {
            options
                .resolve(paths, Some(&self.root))
                .unwrap()
                .into_options(
                    &ApplicationPaths::new("bed", Some(self.settings.config_dir.clone()), None)
                        .unwrap(),
                )
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    #[cfg(feature = "workspaces")]
    fn only_exact_saved_folder_roots_attach() {
        let fixture = Fixture::new();
        let saved = fixture.project("saved");
        let root = PathBuf::from(&saved.root);
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let options = fixture.options(StartupOptions::default(), vec![root.clone()]);
        assert_eq!(options.workspace, Some(saved.clone()));
        for paths in [vec![nested], vec![root.join("file.txt")], vec![]] {
            let options = fixture.options(StartupOptions::default(), paths);
            assert!(options.workspace.is_none());
        }
        let options = fixture.options(
            StartupOptions {
                cwd: Some(root),
                ..Default::default()
            },
            vec![],
        );
        assert_eq!(options.workspace, Some(saved));
    }
    #[test]
    fn last_folder_selects_directory_and_preserves_file_order() {
        let fixture = Fixture::new();
        let first = fixture.root.join("first");
        let last = fixture.root.join("last");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&last).unwrap();
        let a = fixture.root.join("a.txt");
        let b = fixture.root.join("b.txt");
        let options = fixture.options(
            StartupOptions::default(),
            vec![first, a.clone(), last.clone(), b.clone()],
        );
        assert_eq!(
            options.working_directory,
            Some(last.canonicalize().unwrap())
        );
        assert_eq!(
            options
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>(),
            vec![a, b]
        );
        assert!(options.workspace.is_none());
        assert_eq!(options.initial_layout, Some(LayoutSlot::Layout1));
    }
    #[test]
    fn launches_without_paths_select_startup_and_ignore_legacy_preferences() {
        let mut fixture = Fixture::new();
        fixture.project("previous");
        for mode in ["startup", "terminal", "last_project", "unknown"] {
            fixture.settings.settings["startup_mode"] = serde_json::json!(mode);
            for new_window in [false, true] {
                let options = fixture.options(
                    StartupOptions {
                        new_window,
                        ..Default::default()
                    },
                    vec![],
                );
                assert!(options.workspace.is_none());
                assert_eq!(options.initial_layout, Some(LayoutSlot::Startup));
                assert!(options.startup_panel.is_none());
            }
        }
    }
    #[test]
    fn explicit_launch_paths_select_layout_one_unless_a_workspace_is_attached() {
        let fixture = Fixture::new();
        for (startup, paths) in [
            (
                StartupOptions::default(),
                vec![fixture.root.join("file.txt")],
            ),
            (StartupOptions::default(), vec![fixture.root.clone()]),
            (
                StartupOptions {
                    cwd: Some(fixture.root.clone()),
                    ..Default::default()
                },
                vec![],
            ),
        ] {
            let options = fixture.options(startup, paths);
            assert!(options.workspace.is_none());
            assert_eq!(options.initial_layout, Some(LayoutSlot::Layout1));
        }
        let saved = fixture.project("explicit");
        let options = fixture.options(StartupOptions::default(), vec![PathBuf::from(&saved.root)]);
        if cfg!(feature = "workspaces") {
            assert_eq!(options.workspace, Some(saved.clone()));
            assert!(options.initial_layout.is_none());
        } else {
            assert!(options.workspace.is_none());
            assert_eq!(options.initial_layout, Some(LayoutSlot::Layout1));
        }
    }
    #[test]
    #[cfg(feature = "workspaces")]
    fn an_implicit_launch_directory_does_not_attach_a_saved_workspace() {
        let fixture = Fixture::new();
        let saved = fixture.project("implicit-home");
        let options = Startup {
            directory: PathBuf::from(&saved.root),
            workspace: None,
            files: Vec::new(),
            requested_workspace: None,
        }
        .into_options(
            &ApplicationPaths::new("bed", Some(fixture.settings.config_dir.clone()), None).unwrap(),
        )
        .unwrap();
        assert!(options.workspace.is_none());
        assert_eq!(options.initial_layout, Some(LayoutSlot::Startup));
    }
    #[test]
    #[cfg(feature = "workspaces")]
    fn explicit_remote_workspace_wins_over_preferences_without_local_canonicalization() {
        let fixture = Fixture::new();
        let spec = WorkspaceSpec {
            name: "remote".into(),
            target: WorkspaceTarget::Ssh {
                host: "example.invalid".into(),
            },
            root: "/srv/project".into(),
        };
        let options = fixture.options(
            StartupOptions {
                requested_workspace: Some(spec.clone()),
                ..Default::default()
            },
            vec![],
        );
        assert_eq!(options.workspace, Some(spec));
        assert!(options.initial_layout.is_none());
    }
    #[test]
    #[cfg(not(feature = "workspaces"))]
    fn disabled_workspace_integration_ignores_unusable_saved_workspace_state() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.settings.config_dir.join("registry.json"),
            b"not valid JSON",
        )
        .unwrap();
        let options = fixture.options(StartupOptions::default(), vec![fixture.root.clone()]);
        assert!(options.workspace.is_none());
        assert_eq!(options.initial_layout, Some(LayoutSlot::Layout1));
    }

    #[test]
    fn launch_directory_validates_and_defaults_to_home() {
        let home = std::env::temp_dir();
        assert_eq!(
            launch_directory(None, Some(&home)).unwrap(),
            home.canonicalize().unwrap()
        );
        assert!(launch_directory(None, None).is_err());
        assert!(launch_directory(Some(&home.join("bed-missing-launch-dir")), Some(&home)).is_err());
    }
    #[test]
    fn relative_files_keep_process_directory_while_standalone_defaults_to_home() {
        let cwd = std::env::temp_dir();
        let startup = StartupOptions::default()
            .resolve(vec![PathBuf::from("a.txt")], Some(&cwd))
            .unwrap();
        assert_eq!(startup.files, vec![cwd.join("a.txt")]);
        assert_eq!(
            startup.directory,
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .canonicalize()
                .unwrap()
        );
    }
}

#[derive(Default, Clone)]
pub(crate) struct SmokeOptions {
    pub scenarios: Vec<String>,
    pub capture: Option<PathBuf>,
    pub after_frames: u32,
}
impl SmokeOptions {
    pub fn requested(&self) -> bool {
        !self.scenarios.is_empty() || self.capture.is_some()
    }
}
pub(crate) struct Launch {
    pub startup: StartupOptions,
    pub paths: Vec<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub smoke: SmokeOptions,
}
impl Launch {
    pub fn parse(
        arguments: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let mut launch = Self {
            startup: StartupOptions::default(),
            paths: Vec::new(),
            config_dir: None,
            smoke: SmokeOptions {
                after_frames: 2,
                ..Default::default()
            },
        };
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--help" | "-h") => {
                    println!(
                        "Usage: bed [FILE_OR_FOLDER ...] [--cwd DIRECTORY] [--config-dir DIRECTORY]\n  --new-window     Open a standalone instance\n  --workspace JSON\n  --capture-frame OUTPUT.png|OUTPUT.ppm [--capture-after-frames N]\n  --smoke-test | --main-only-smoke | --appearance-smoke\n  --platform-smoke | --menu-smoke | --term-promotion-smoke\n\nSmoke and screenshot commands require --features automation.\nWindows without launch paths open the configurable Startup layout.\nExplicit files and folders use layout 1; saved workspace roots restore their selected layout.\nOnly exact saved folder roots attach at launch; workspaces must be requested explicitly."
                    );
                    return Ok(None);
                }
                Some("--cwd") => {
                    launch.startup.cwd =
                        Some(PathBuf::from(arguments.next().ok_or_else(|| {
                            io::Error::other("--cwd requires a directory")
                        })?))
                }
                Some("--config-dir") => {
                    launch.config_dir =
                        Some(PathBuf::from(arguments.next().ok_or_else(|| {
                            io::Error::other("--config-dir requires a directory")
                        })?))
                }
                Some("--new-window") => launch.startup.new_window = true,
                Some("--workspace") => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| io::Error::other("--workspace requires JSON"))?;
                    let value = serde_json::from_str(&value.to_string_lossy())?;
                    launch.startup.requested_workspace = Some(
                        WorkspaceSpec::from_value(&value)
                            .ok_or_else(|| io::Error::other("Invalid workspace specification"))?,
                    );
                }
                Some("--capture-frame") => {
                    launch.smoke.capture =
                        Some(PathBuf::from(arguments.next().ok_or_else(|| {
                            io::Error::other("--capture-frame requires an output path")
                        })?))
                }
                Some("--capture-after-frames") => {
                    launch.smoke.after_frames = arguments
                        .next()
                        .ok_or_else(|| {
                            io::Error::other("--capture-after-frames requires a positive count")
                        })?
                        .to_string_lossy()
                        .parse()?;
                    if launch.smoke.after_frames == 0 {
                        return Err(
                            io::Error::other("--capture-after-frames must be positive").into()
                        );
                    }
                }
                Some(
                    "--smoke-test"
                    | "--main-only-smoke"
                    | "--appearance-smoke"
                    | "--platform-smoke"
                    | "--menu-smoke"
                    | "--term-promotion-smoke",
                ) => launch
                    .smoke
                    .scenarios
                    .push(argument.to_string_lossy().into_owned()),
                Some("--viewports-smoke") => {
                    return Err(
                        io::Error::other("detached native windows are not supported").into(),
                    );
                }
                Some("--term" | "--edit") => {
                    return Err(io::Error::other(
                        "--term and --edit have been removed; configure the Startup layout in the Window menu",
                    )
                    .into());
                }
                Some(value) if value.starts_with("--") => {
                    return Err(io::Error::other(format!("Unknown option {value}")).into());
                }
                _ => launch.paths.push(PathBuf::from(argument)),
            }
        }
        Ok(Some(launch))
    }
}
