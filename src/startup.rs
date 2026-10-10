//! Startup preferences select standalone panels or resume the last project.
//! Explicit launch paths take precedence and only exact saved roots attach.
use bed_settings::{Settings, StartupMode};
use bed_workbench::{WindowCommand, Workbench};
use bed_workbench_api::workspace::{WorkspaceSpec, WorkspaceTarget};
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
        let resume_last = !self.new_window && self.cwd.is_none() && paths.is_empty();
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
        Ok(Startup {
            directory,
            workspace,
            files,
            new_window: self.new_window,
            resume_last,
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
    new_window: bool,
    resume_last: bool,
    requested_workspace: Option<WorkspaceSpec>,
}

impl Startup {
    pub fn into_workbench(self, settings: Settings) -> io::Result<Workbench> {
        let mode = if self.new_window {
            StartupMode::Startup
        } else {
            settings.startup_mode()
        };
        let mut workbench = Workbench::with_settings(settings, crate::builtins::modules);
        workbench.set_directory(&self.directory)?;
        let attached = if let Some(spec) = self.requested_workspace {
            if !matches!(spec.target, WorkspaceTarget::Local)
                || !workbench.open_startup_workspace(Path::new(&spec.root))?
            {
                workbench.set_workspace(spec)?;
            }
            true
        } else {
            match self.workspace {
                Some(root) => workbench.open_startup_workspace(&root)?,
                None => false,
            }
        };
        if !attached {
            match mode {
                StartupMode::Terminal => {
                    workbench.dispatch(WindowCommand::NewTerminal)?;
                    // A standalone terminal launch contains exactly one panel.
                    workbench.close_tab(0)?;
                }
                StartupMode::LastProject if self.resume_last => {
                    if let Err(error) = workbench.restore_last_workspace() {
                        workbench.error = Some(format!("Could not restore last project: {error}"));
                    }
                }
                _ => {}
            }
        }
        for path in self.files {
            workbench.open_or_focus(&path)?;
        }
        Ok(workbench)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench::WindowCommand;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "bed-startup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn workbench(&self, paths: Vec<PathBuf>, cwd: &Path) -> Workbench {
            let settings =
                Settings::with_paths(self.0.join("config"), Settings::get_app_resources_path())
                    .unwrap();
            StartupOptions::default()
                .resolve(paths, Some(cwd))
                .unwrap()
                .into_workbench(settings)
                .unwrap()
        }
        fn save_workspace(&self, root: &Path) {
            let file = root.join("saved.txt");
            std::fs::write(&file, b"saved document").unwrap();
            let mut previous = self.workbench(vec![root.to_owned(), file], root);
            previous.set_project(root).unwrap();
            previous.cleanup().unwrap();
        }
        fn startup_mode(&self, mode: &str) {
            let mut settings =
                Settings::with_paths(self.0.join("config"), Settings::get_app_resources_path())
                    .unwrap();
            settings.settings["startup_mode"] = serde_json::json!(mode);
            settings.save_settings().unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn bare_launch_uses_projects_and_ignores_saved_cwd_workspace() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        let mut workbench = fixture.workbench(vec![], &fixture.0);
        workbench.tick().unwrap();
        assert!(workbench.workspace_spec.is_none());
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 1);
        assert_eq!(workbench.terminal.session_count(), 0);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn explicit_saved_folder_restores_directly_without_default_terminals() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        let mut workbench = fixture.workbench(vec![fixture.0.clone()], &fixture.0);
        assert!(workbench.workspace_spec.is_some());
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 0);
        assert_eq!(workbench.panel_count(bed_module_editor::TEXT_PANEL_TYPE), 1);
        assert_eq!(workbench.terminal.session_count(), 1);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn file_only_launch_does_not_attach_its_saved_workspace() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        let file = fixture.0.join("standalone.txt");
        std::fs::write(&file, b"standalone").unwrap();
        let mut workbench = fixture.workbench(vec![file], &fixture.0);
        assert!(workbench.workspace_spec.is_none());
        assert_eq!(workbench.active_snapshot().unwrap().bytes, b"standalone");
        assert_eq!(workbench.terminal.session_count(), 0);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn last_folder_sets_standalone_base_then_all_files_open_in_order() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        let first = fixture.0.join("first");
        let last = fixture.0.join("last");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&last).unwrap();
        let a = first.join("a.txt");
        let b = last.join("b.txt");
        std::fs::write(&a, b"first file").unwrap();
        std::fs::write(&b, b"last file").unwrap();
        let paths = vec![a, first, b, last.clone()];
        let mut workbench = fixture.workbench(paths, &fixture.0);
        assert!(workbench.workspace_spec.is_none());
        assert_eq!(workbench.working_directory(), last.canonicalize().unwrap());
        assert_eq!(workbench.panel_count(bed_module_editor::TEXT_PANEL_TYPE), 2);
        assert_eq!(workbench.active_snapshot().unwrap().bytes, b"last file");
        let store =
            bed_workbench::workspace::store::WorkspaceStore::load(&fixture.0.join("config"))
                .unwrap();
        assert!(
            store
                .recent_workspaces()
                .iter()
                .all(|spec| spec.root != fixture.0.join("first").to_str().unwrap())
        );
        workbench.cleanup().unwrap();
    }

    #[test]
    fn cwd_sets_working_directory_without_attaching_a_workspace() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        let settings =
            Settings::with_paths(fixture.0.join("config"), Settings::get_app_resources_path())
                .unwrap();
        let mut workbench = StartupOptions {
            cwd: Some(fixture.0.clone()),
            ..Default::default()
        }
        .resolve(vec![], Some(Path::new("/")))
        .unwrap()
        .into_workbench(settings)
        .unwrap();
        assert!(workbench.workspace_spec.is_none());
        assert_eq!(
            workbench.working_directory(),
            fixture.0.canonicalize().unwrap()
        );
        assert_eq!(workbench.terminal.session_count(), 0);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn terminal_startup_opens_only_one_fresh_shell() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        fixture.startup_mode("terminal");
        let mut workbench = fixture.workbench(vec![], &fixture.0);
        workbench.tick().unwrap();
        assert!(workbench.workspace_spec.is_none());
        assert_eq!(workbench.tab_count(), 1);
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 0);
        assert_eq!(workbench.terminal.session_count(), 1);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn saved_default_only_initializes_new_workspaces() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        let mut previous = fixture.workbench(vec![], &fixture.0);
        previous.close_tab(0).unwrap();
        previous.dispatch(WindowCommand::NewTerminal).unwrap();
        previous.dispatch(WindowCommand::NewDocument).unwrap();
        assert_eq!(previous.panel_count(bed_module_editor::TEXT_PANEL_TYPE), 1);
        previous.dispatch(WindowCommand::SaveDefaultLayout).unwrap();
        previous.cleanup().unwrap();
        drop(previous);

        let file = fixture.0.join("file.txt");
        std::fs::write(&file, b"standalone document").unwrap();
        let mut standalone = fixture.workbench(vec![file], &fixture.0);
        assert!(standalone.workspace_spec.is_none());
        assert_eq!(
            standalone.panel_count(bed_module_editor::TEXT_PANEL_TYPE),
            1,
        );
        assert_eq!(standalone.panel_count(bed_module_projects::PANEL_ID), 1);
        assert_eq!(standalone.terminal.session_count(), 0);
        assert_eq!(
            standalone.active_snapshot().unwrap().bytes,
            b"standalone document"
        );
        standalone.cleanup().unwrap();
        drop(standalone);

        let folder = fixture.0.join("new-workspace");
        std::fs::create_dir(&folder).unwrap();
        let mut fresh_workspace = fixture.workbench(vec![folder.clone()], &fixture.0);
        assert!(fresh_workspace.workspace_spec.is_none());
        assert_eq!(
            fresh_workspace.panel_count(bed_module_projects::PANEL_ID),
            1
        );
        assert_eq!(fresh_workspace.terminal.session_count(), 0);
        fresh_workspace.set_project(&folder).unwrap();
        assert!(fresh_workspace.workspace_spec.is_some());
        assert_eq!(
            fresh_workspace.panel_count(bed_module_editor::TEXT_PANEL_TYPE),
            1
        );
        assert_eq!(
            fresh_workspace.panel_count(bed_module_projects::PANEL_ID),
            0
        );
        assert_eq!(fresh_workspace.terminal.session_count(), 1);
        assert!(fresh_workspace.active_snapshot().unwrap().bytes.is_empty());
        let terminal = fresh_workspace.terminal.active_session_id().unwrap();
        assert_eq!(
            fresh_workspace.terminal.working_directory(terminal),
            Some(folder.canonicalize().unwrap().as_path())
        );
        fresh_workspace.cleanup().unwrap();
    }

    #[test]
    fn last_project_survives_standalone_windows_and_restores_saved_layout() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        let mut standalone = fixture.workbench(vec![], &fixture.0);
        standalone.cleanup().unwrap();
        fixture.startup_mode("last_project");
        let mut resumed = fixture.workbench(vec![], &fixture.0);
        assert_eq!(
            resumed.workspace_spec.as_ref().unwrap().root,
            fixture.0.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(resumed.panel_count(bed_module_projects::PANEL_ID), 0);
        assert_eq!(resumed.panel_count(bed_module_editor::TEXT_PANEL_TYPE), 1);
        assert_eq!(resumed.terminal.session_count(), 1);
        resumed.cleanup().unwrap();
    }

    #[test]
    fn last_project_without_a_project_or_with_a_missing_root_keeps_startup() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.startup_mode("last_project");
        let mut empty = fixture.workbench(vec![], &fixture.0);
        assert!(empty.workspace_spec.is_none());
        assert_eq!(empty.panel_count(bed_module_projects::PANEL_ID), 1);
        assert!(empty.error.is_none());
        empty.cleanup().unwrap();
        let root = fixture.0.join("deleted-project");
        std::fs::create_dir(&root).unwrap();
        fixture.save_workspace(&root);
        std::fs::remove_dir_all(&root).unwrap();
        let mut missing = fixture.workbench(vec![], &fixture.0);
        assert!(missing.workspace_spec.is_none());
        assert_eq!(missing.panel_count(bed_module_projects::PANEL_ID), 1);
        assert!(
            missing
                .error
                .as_ref()
                .unwrap()
                .contains("Could not restore last project")
        );
        missing.cleanup().unwrap();
    }

    #[test]
    fn explicit_paths_and_cwd_suppress_last_project_and_new_window_always_uses_startup() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        fixture.startup_mode("last_project");
        let file = fixture.0.join("saved.txt");
        let mut standalone = fixture.workbench(vec![file], &fixture.0);
        assert!(standalone.workspace_spec.is_none());
        standalone.cleanup().unwrap();
        for mode in ["terminal", "last_project"] {
            fixture.startup_mode(mode);
            let settings =
                Settings::with_paths(fixture.0.join("config"), Settings::get_app_resources_path())
                    .unwrap();
            let mut window = StartupOptions {
                new_window: true,
                ..Default::default()
            }
            .resolve(vec![], Some(&fixture.0))
            .unwrap()
            .into_workbench(settings)
            .unwrap();
            assert!(window.workspace_spec.is_none());
            assert_eq!(window.tab_count(), 1);
            assert_eq!(window.panel_count(bed_module_projects::PANEL_ID), 1);
            assert_eq!(window.terminal.session_count(), 0);
            window.cleanup().unwrap();
        }
        let settings =
            Settings::with_paths(fixture.0.join("config"), Settings::get_app_resources_path())
                .unwrap();
        let mut cwd = StartupOptions {
            cwd: Some(fixture.0.clone()),
            ..Default::default()
        }
        .resolve(vec![], None)
        .unwrap()
        .into_workbench(settings)
        .unwrap();
        assert!(cwd.workspace_spec.is_none());
        assert_eq!(cwd.panel_count(bed_module_projects::PANEL_ID), 1);
        cwd.cleanup().unwrap();
    }

    #[test]
    fn explicit_workspace_launch_does_not_compose_startup_terminals() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        fixture.startup_mode("terminal");
        let settings =
            Settings::with_paths(fixture.0.join("config"), Settings::get_app_resources_path())
                .unwrap();
        let mut workbench = StartupOptions {
            new_window: true,
            requested_workspace: Some(WorkspaceSpec::local(fixture.0.to_str().unwrap())),
            ..Default::default()
        }
        .resolve(vec![], None)
        .unwrap()
        .into_workbench(settings)
        .unwrap();
        assert!(workbench.workspace_spec.is_some());
        assert_eq!(workbench.terminal.session_count(), 1);
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 0);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn every_registered_bundled_panel_can_restore_with_no_input_and_null_state() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        let composition = crate::builtins::modules();
        let mut registry = bed_workbench_api::Registry::default();
        for module in &composition.instances {
            registry.register(module.as_ref()).unwrap();
        }
        let ids: Vec<u64> = (1..=registry.panels.len() as u64).collect();
        let panels: Vec<_> = registry
            .panels
            .iter()
            .zip(&ids)
            .map(
                |(panel, id)| serde_json::json!({"id": id, "panel_type": panel.id, "viewer": null}),
            )
            .collect();
        let mut store =
            bed_workbench::workspace::store::WorkspaceStore::load(&fixture.0.join("config"))
                .unwrap();
        store
            .set_default_layout(serde_json::json!({
                "version": 1,
                "panels": panels,
                "focused": ids.first(),
                "tiling": {
                    "version": 1,
                    "layout": { "version": 1, "next_id": 2, "areas": [
                        {"id": 1, "min": [0, 0], "max": [10000, 10000]}
                    ] },
                    "areas": [{"area": 1, "tabs": ids, "selected": ids.first()}]
                }
            }))
            .unwrap();
        drop(store);
        let mut workbench = fixture.workbench(vec![], &fixture.0);
        workbench.apply_default_layout().unwrap();
        assert!(workbench.error.is_none(), "{:?}", workbench.error);
        assert_eq!(workbench.tab_count(), registry.panels.len());
        for panel in &registry.panels {
            assert_eq!(workbench.panel_count(panel.id), 1, "{}", panel.id);
        }
        workbench.cleanup().unwrap();
    }

    #[test]
    fn launch_directory_defaults_to_home_and_validates_explicit_directories() {
        let fixture = Fixture::new();
        let home = fixture.0.join("home");
        std::fs::create_dir(&home).unwrap();
        assert_eq!(
            launch_directory(None, Some(&home)).unwrap(),
            home.canonicalize().unwrap()
        );
        assert_eq!(
            launch_directory(Some(Path::new("/")), Some(&home)).unwrap(),
            Path::new("/")
        );
        assert!(launch_directory(Some(&fixture.0.join("deleted")), Some(&home)).is_err());
        let file = fixture.0.join("file");
        std::fs::write(&file, b"file").unwrap();
        assert!(launch_directory(Some(&file), Some(&home)).is_err());
        assert!(launch_directory(None, None).is_err());
    }

    #[test]
    fn unknown_folder_and_workspace_subdirectory_stay_standalone() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let mut standalone = fixture.workbench(vec![root.clone()], &fixture.0);
        assert!(standalone.workspace_spec.is_none());
        standalone.cleanup().unwrap();
        assert!(!fixture.0.join("config/workspaces.json").exists());
        fixture.save_workspace(&root);
        let mut nested = fixture.workbench(vec![child.clone()], &fixture.0);
        assert!(nested.workspace_spec.is_none());
        assert_eq!(nested.working_directory(), child.canonicalize().unwrap());
        nested.cleanup().unwrap();
    }

    #[test]
    fn bare_and_relative_file_launches_use_home_without_rebasing_file_paths() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        fixture.save_workspace(&fixture.0);
        let mut bare = fixture.workbench(vec![], &fixture.0);
        assert_eq!(
            bare.working_directory(),
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .canonicalize()
                .unwrap()
        );
        assert!(bare.workspace_spec.is_none());
        bare.cleanup().unwrap();
        let mut file = fixture.workbench(vec![PathBuf::from("saved.txt")], &fixture.0);
        assert!(file.workspace_spec.is_none());
        assert_eq!(file.active_snapshot().unwrap().bytes, b"saved document");
        file.cleanup().unwrap();
    }
}
