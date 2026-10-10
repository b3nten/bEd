//! Shell commands belong to ordinary terminals in every layout.
use super::*;
use bed_editing::identity::WorkspaceId;
use bed_terminal::shell_bridge::{self, Bridge, Request};

struct PendingDock {
    source: u64,
    target: u64,
    split_down: Option<bool>,
}

#[derive(Default)]
pub(super) struct ShellIntegration {
    bridge: Option<Bridge>,
    polling: bool,
    session: Option<WorkspaceId>,
    groups: HashMap<u64, u64>,
    views: HashMap<(u64, PathBuf), u64>,
    pending: Vec<PendingDock>,
}

impl Workbench {
    pub(super) fn open_terminal_link(&mut self, session: u64, target: &str) -> io::Result<()> {
        use bed_terminal::terminal_links::{LinkTarget, classify_link};
        match classify_link(target)
            .ok_or_else(|| io::Error::other("This link has no supported opening action"))?
        {
            LinkTarget::External(url) => {
                #[cfg(target_os = "macos")]
                let program = "open";
                #[cfg(not(target_os = "macos"))]
                let program = "xdg-open";
                let mut child = std::process::Command::new(program).arg(&url).spawn()?;
                std::thread::Builder::new()
                    .name("bed-open-link".into())
                    .spawn(move || {
                        let _ = child.wait();
                    })?;
            }
            LinkTarget::File { host, path } => {
                if let Some(ssh) = self.terminal.ssh_target(session) {
                    let remote_host = ssh.host.split('@').next_back().unwrap_or(&ssh.host);
                    if host.as_deref().is_some_and(|host| host != remote_host) {
                        return Err(io::Error::other(
                            "The file link belongs to a different SSH host",
                        ));
                    }
                    let same_workspace = self.workspace_spec.as_ref().is_some_and(|spec| matches!(&spec.target, WorkspaceTarget::Ssh { host } if host == &ssh.host));
                    if !same_workspace {
                        return Err(io::Error::other(
                            "Open this terminal's SSH workspace before opening its file links",
                        ));
                    }
                    self.open_or_focus(&path)?;
                } else {
                    self.open_terminal_companion(&path, Some(session))?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn ensure_shell_integration(&mut self) -> io::Result<()> {
        if self.shell_integration.bridge.is_some() || self.shell_integration.polling {
            return Ok(());
        }
        let bridge = Bridge::new()?;
        let executable = std::env::current_exe()?;
        let mut paths = vec![bridge.helper_directory().to_owned()];
        if let Some(parent) = executable.parent() {
            paths.push(parent.to_owned());
        }
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = std::env::join_paths(paths).map_err(io::Error::other)?;
        self.terminal.configure_shell_environment(HashMap::from([
            (
                shell_bridge::SOCKET_ENV.into(),
                bridge.socket_path().to_string_lossy().into_owned(),
            ),
            (
                "BEDTERM_EXE".into(),
                executable.to_string_lossy().into_owned(),
            ),
            ("PATH".into(), path.to_string_lossy().into_owned()),
        ]));
        self.shell_integration.bridge = Some(bridge);
        Ok(())
    }

    fn sync_shell_integration(&mut self) {
        let session = self.session.workspace_id();
        if self.shell_integration.session != Some(session) {
            self.shell_integration.session = Some(session);
            self.shell_integration.groups.clear();
            self.shell_integration.views.clear();
            self.shell_integration.pending.clear();
        }
    }

    pub(super) fn poll_shell_integration(&mut self) -> io::Result<()> {
        self.sync_shell_integration();
        let Some(mut bridge) = self.shell_integration.bridge.take() else {
            return Ok(());
        };
        // Restoring a workspace may open terminal panels while the bridge is
        // borrowed here. They must keep using its configured shell environment.
        self.shell_integration.polling = true;
        let result = bridge.poll(|request| {
            let pane = match &request {
                Request::Open { pane, .. }
                | Request::Workspace { pane, .. }
                | Request::Ducky { pane } => pane,
            };
            let terminal = match pane {
                Some(pane) => Some(
                    self.terminal
                        .session_for_pane(pane)
                        .filter(|id| self.terminal_panel_id(*id).is_some())
                        .ok_or_else(|| {
                            "The shell that requested this command is no longer available"
                                .to_owned()
                        })?,
                ),
                None => self.terminal.active_session_id(),
            };
            match request {
                Request::Open { paths, .. } => {
                    for path in paths {
                        self.open_terminal_companion(&path, terminal)
                            .map_err(|error| error.to_string())?;
                    }
                }
                Request::Workspace { root, .. } => {
                    if self.session.is_remote() {
                        return Err("Workspace upgrades require a local terminal".into());
                    }
                    if self.workspace_spec.is_none() {
                        self.attach_workspace_with_terminal(&root, terminal)
                    } else {
                        self.set_project(&root)
                    }
                    .map_err(|error| error.to_string())?;
                }
                Request::Ducky { .. } => {
                    self.open_plugin_panel_in_area(
                        "bed.duck.panel",
                        None,
                        &Value::Null,
                        None,
                        self.largest_area(),
                    )
                    .map_err(|error| error.to_string())?;
                }
            }
            Ok(())
        });
        self.shell_integration.bridge = Some(bridge);
        self.shell_integration.polling = false;
        result
    }

    /// Open a viewer beside the requesting shell. A document is shared, while
    /// each shell gets its own view; subsequent files reuse its companion group.
    pub fn open_terminal_companion(
        &mut self,
        path: &Path,
        terminal: Option<u64>,
    ) -> io::Result<()> {
        self.sync_shell_integration();
        if !path.is_file() {
            return Err(io::Error::other(format!(
                "Expected a file: {}",
                path.display()
            )));
        }
        let path = std::fs::canonicalize(path)?;
        let terminal = terminal.or_else(|| self.terminal.active_session_id());
        let source = terminal.and_then(|id| self.terminal_panel_id(id));
        let Some(source) = source else {
            self.open_or_focus(&path)?;
            return Ok(());
        };
        if let Some(index) = self
            .shell_integration
            .views
            .get(&(source, path.clone()))
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == *id))
        {
            self.switch_to_tab(index);
            return Ok(());
        }
        let anchor = self
            .shell_integration
            .groups
            .get(&source)
            .copied()
            .filter(|id| self.tabs.iter().any(|tab| tab.id == *id))
            .or_else(|| {
                self.shell_integration
                    .views
                    .iter()
                    .filter(|((shell, _), id)| {
                        *shell == source && self.tabs.iter().any(|tab| tab.id == **id)
                    })
                    .map(|(_, id)| *id)
                    .min()
            });
        let already_open = self.session.document_for_path(&path).is_some();
        self.open_or_focus(&path)?;
        if already_open {
            self.dispatch(WindowCommand::DuplicateView)?;
        }
        if let Some(target) = self.active_panel_id() {
            self.shell_integration.pending.push(PendingDock {
                source: anchor.unwrap_or(source),
                target,
                split_down: anchor.is_none().then_some(false),
            });
            self.shell_integration.groups.insert(source, target);
            self.shell_integration.views.insert((source, path), target);
        }
        Ok(())
    }

    /// Split from the last focused ordinary terminal, preserving its process.
    pub fn split_last_terminal(&mut self, down: bool) -> io::Result<u64> {
        self.sync_shell_integration();
        let source = self
            .terminal
            .active_session_id()
            .and_then(|id| self.terminal_panel_id(id))
            .ok_or_else(|| io::Error::other("Select a terminal before splitting"))?;
        let area = self
            .area_for_panel(source)
            .expect("terminal belongs to an area");
        let axis = usize::from(down);
        let rect = self.current_tiling().layout.area(area).rect;
        if !self.can_split_area(area, axis, (rect.min[axis] + rect.max[axis]) / 2) {
            return Err(io::Error::other("This terminal area is too small to split"));
        }
        let target = self.open_plugin_panel_in_area(
            bed_module_terminal::PANEL_ID,
            None,
            &Value::Null,
            None,
            area,
        )?;
        assert!(
            self.place_panel_beside(source, target, Some(down)),
            "opening a terminal preserves its validated split"
        );
        Ok(target)
    }

    pub(super) fn finish_shell_integration(&mut self) {
        for pending in std::mem::take(&mut self.shell_integration.pending) {
            if !self.tabs.iter().any(|tab| tab.id == pending.source)
                || !self.tabs.iter().any(|tab| tab.id == pending.target)
            {
                continue;
            }
            if !self.place_panel_beside(pending.source, pending.target, pending.split_down) {
                // A companion still opens when its shell is too small to split.
                // Consume the request so resizing cannot trigger a later split.
                assert!(
                    self.place_panel_beside(pending.source, pending.target, None),
                    "live shell panel belongs to an area"
                );
            }
            if let Some(index) = self.tabs.iter().position(|tab| tab.id == pending.target) {
                self.switch_to_tab(index);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use bed_terminal::terminal_pty::{PtyOptions, TerminalShell};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::{ffi::OsStrExt, net::UnixStream};

    fn shell_request(workbench: &mut Workbench, request: Value) -> Value {
        let bridge = workbench.shell_integration.bridge.as_ref().unwrap();
        let mut stream = UnixStream::connect(bridge.socket_path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        serde_json::to_writer(&mut stream, &request).unwrap();
        stream.write_all(b"\n").unwrap();
        workbench.poll_shell_integration().unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    #[test]
    fn terminal_file_links_open_beside_the_originating_shell() {
        let dir = TempDir::new();
        let file = dir.write("linked file.txt", b"linked contents");
        let mut workbench = super::super::tests::workspace(&dir);
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let session = workbench.terminal.active_session_id().unwrap();
        let terminal_panel = workbench.active_panel_id().unwrap();
        let target = format!("file://{}", file.to_string_lossy().replace(' ', "%20"));
        workbench.open_terminal_link(session, &target).unwrap();
        let companion = workbench.active_panel_id().unwrap();
        assert_ne!(companion, terminal_panel);
        assert_eq!(
            workbench
                .session
                .document_for_path(&file.canonicalize().unwrap()),
            workbench.active_document()
        );
        assert_eq!(workbench.terminal.session_ids(), vec![session]);
        let tabs = workbench.tabs.len();
        assert!(
            workbench
                .open_terminal_link(session, "javascript:alert(1)")
                .is_err()
        );
        assert_eq!(workbench.tabs.len(), tabs);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn shell_ducky_opens_and_moves_the_singleton_to_the_largest_area() {
        let dir = TempDir::new();
        let mut workbench = super::super::tests::workspace(&dir);
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let terminal = workbench.active_panel_id().unwrap();
        let mut layout = crate::workspace::tiling::Layout::default();
        let largest = layout.split(1, 0, 2500, 1).unwrap();
        let expected_layout = layout.clone();
        workbench.tiling = TilingState::new(layout);
        for panel in workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>() {
            workbench.place_panel_in_area(panel, 1);
        }
        assert_eq!(workbench.active_panel_id(), Some(terminal));
        assert!(
            shell_request(
                &mut workbench,
                json!({"command":"ducky","pane":"closed-pane"})
            )["error"]
                .is_string()
        );
        assert!(
            !workbench
                .tabs
                .iter()
                .any(|tab| tab.panel.kind == "bed.duck.panel")
        );
        assert!(shell_request(&mut workbench, json!({"command":"ducky"}))["error"].is_null());
        let duck = workbench.active_panel_id().unwrap();
        assert_eq!(
            workbench
                .tabs
                .iter()
                .find(|tab| tab.id == duck)
                .unwrap()
                .panel
                .kind,
            "bed.duck.panel"
        );
        assert_eq!(workbench.area_for_panel(duck), Some(largest));
        assert_eq!(workbench.tiling.layout, expected_layout);
        workbench.place_panel_in_area(duck, 1);
        assert!(shell_request(&mut workbench, json!({"command":"ducky"}))["error"].is_null());
        assert_eq!(workbench.active_panel_id(), Some(duck));
        assert_eq!(workbench.area_for_panel(duck), Some(largest));
        assert_eq!(
            workbench
                .tabs
                .iter()
                .filter(|tab| tab.panel.kind == "bed.duck.panel")
                .count(),
            1
        );
        workbench.cleanup().unwrap();
    }

    #[test]
    fn shell_workspace_attaches_the_requested_directory_and_preserves_live_panels() {
        let dir = TempDir::new();
        let file = dir.write("project/file.txt", b"original");
        let root = dir.path("project").canonicalize().unwrap();
        let mut workbench = super::super::tests::workspace(&dir);
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let first = workbench.terminal.active_session_id().unwrap();
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        workbench
            .open_terminal_companion(&file, Some(first))
            .unwrap();
        let document = workbench.active_document().unwrap();
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"dirty "))
            .unwrap();
        let sessions = workbench.terminal.session_ids();
        let panels = workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.kind != bed_module_projects::PANEL_ID)
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        let mut request = json!({"command":"workspace", "root":root.as_os_str().as_bytes()});
        request["pane"] = json!("closed-pane");
        assert!(shell_request(&mut workbench, request.clone())["error"].is_string());
        assert!(workbench.workspace_spec.is_none());
        request.as_object_mut().unwrap().remove("pane");
        assert!(shell_request(&mut workbench, request)["error"].is_null());
        assert_eq!(
            workbench.session.options().project_root.as_deref(),
            Some(root.as_path())
        );
        assert_eq!(workbench.terminal.session_ids(), sessions);
        assert_eq!(workbench.active_view(), Some(view));
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        assert!(
            panels
                .iter()
                .all(|id| workbench.tabs.iter().any(|tab| tab.id == *id))
        );
        assert_eq!(workbench.tiling.layout.areas.len(), 4);
        assert_eq!(workbench.panel_count(bed_module_explorer::PANEL_ID), 1);
        assert!(
            shell_request(
                &mut workbench,
                json!({"command":"workspace", "root":dir.root().as_os_str().as_bytes()})
            )["error"]
                .is_null()
        );
        assert_eq!(
            workbench.session.options().project_root.as_deref(),
            Some(root.as_path())
        );
        assert_eq!(
            workbench
                .take_workspace_windows()
                .into_iter()
                .map(|spec| PathBuf::from(spec.root))
                .collect::<Vec<_>>(),
            vec![dir.root().canonicalize().unwrap()]
        );
        let same = json!({"command":"workspace", "root":root.as_os_str().as_bytes()});
        assert!(shell_request(&mut workbench, same)["error"].is_null());
        assert!(workbench.take_workspace_windows().is_empty());
        assert_eq!(workbench.working_directory(), root);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn shell_workspace_keeps_the_command_bridge_when_restoring_saved_terminals() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let dir = TempDir::new();
        std::fs::create_dir(dir.path("project")).unwrap();
        let root = dir.path("project").canonicalize().unwrap();
        let mut workbench = super::super::tests::workspace(&dir);
        workbench.terminal = BedTerminal::with_pty_options(PtyOptions {
            working_directory: Some(root.clone()),
            shell: Some(TerminalShell::new("/bin/sh", vec!["-i".into()])),
            ..Default::default()
        });
        for session in workbench.terminal.session_ids() {
            assert!(workbench.terminal.close_session_id(session));
        }
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        workbench
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        super::super::tests::frame(&mut context, &mut workbench);
        let first = workbench.terminal.active_session_id().unwrap();
        let first_pid = workbench.terminal.process_id(first).unwrap();
        let spec = WorkspaceSpec::local(root.to_str().unwrap());
        workbench
            .store
            .as_mut()
            .unwrap()
            .set_layout(
                &spec,
                json!({"version":1,"panels":[{"id":1,"kind":"terminal"}]}),
            )
            .unwrap();
        let socket = workbench
            .shell_integration
            .bridge
            .as_ref()
            .unwrap()
            .socket_path()
            .to_owned();
        assert!(
            shell_request(
                &mut workbench,
                json!({"command":"workspace", "root":root.as_os_str().as_bytes()})
            )["error"]
                .is_null()
        );
        assert_eq!(workbench.terminal.session_count(), 2);
        assert_eq!(workbench.terminal.process_id(first), Some(first_pid));
        assert_eq!(
            workbench
                .shell_integration
                .bridge
                .as_ref()
                .unwrap()
                .socket_path(),
            socket
        );
        let restored = *workbench.terminal.session_ids().last().unwrap();
        let tab = workbench.terminal_panel_id(restored).unwrap();
        let index = workbench
            .tabs
            .iter()
            .position(|panel| panel.id == tab)
            .unwrap();
        workbench.switch_to_tab(index);
        super::super::tests::frame(&mut context, &mut workbench);
        let output = dir.path("restored-socket");
        let quoted = output.to_string_lossy().replace('\'', "'\\''");
        workbench
            .terminal
            .write_active(format!("printf '%s' \"$BEDTERM_SOCKET\" > '{quoted}'\n").as_bytes())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while std::fs::read(&output).unwrap_or_default().is_empty() {
            workbench.terminal.poll().unwrap();
            assert!(
                Instant::now() < deadline,
                "restored shell did not report its socket"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read(output).unwrap(),
            socket.as_os_str().as_bytes()
        );
        workbench.cleanup().unwrap();
    }

    fn wait_for_directory(workbench: &mut Workbench, session: u64, directory: &Path) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            workbench.terminal.poll().unwrap();
            if workbench
                .terminal
                .live_working_directory(session)
                .ok()
                .as_deref()
                == Some(directory)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "terminal {session} did not reach {}",
                directory.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn change_directory(workbench: &mut Workbench, session: u64, directory: &Path) {
        assert!(workbench.terminal.focus_session(session));
        let quoted = directory.to_string_lossy().replace('\'', "'\\''");
        workbench
            .terminal
            .write_active(format!("cd '{quoted}'\n").as_bytes())
            .unwrap();
        wait_for_directory(workbench, session, directory);
    }

    #[test]
    fn new_terminals_and_splits_use_the_window_base_despite_shell_cd() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for attached in [false, true] {
            let dir = TempDir::new();
            for directory in ["launch", "project", "first cwd", "second cwd"] {
                std::fs::create_dir_all(dir.path(directory)).unwrap();
            }
            let launch = dir.path("launch").canonicalize().unwrap();
            let first_cwd = dir.path("first cwd").canonicalize().unwrap();
            let second_cwd = dir.path("second cwd").canonicalize().unwrap();
            let viewer = dir.write("viewer.txt", b"viewer focus preserves the last shell");
            let mut workbench = super::super::tests::workspace(&dir);
            workbench.terminal = BedTerminal::with_pty_options(PtyOptions {
                working_directory: Some(launch.clone()),
                shell: Some(TerminalShell::new("/bin/sh", vec!["-i".into()])),
                ..Default::default()
            });
            for session in workbench.terminal.session_ids() {
                assert!(workbench.terminal.close_session_id(session));
            }
            workbench.set_directory(&launch).unwrap();
            if attached {
                workbench.set_project(&dir.path("project")).unwrap();
            }
            let initial_sessions = workbench.terminal.session_count();
            let base = workbench.working_directory();
            let mut context = Context::create();
            context.set_ini_filename(None::<PathBuf>).unwrap();
            workbench
                .initialize(&mut context, WorkbenchHostMode::Fullscreen)
                .unwrap();
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            workbench.dispatch(WindowCommand::NewTerminal).unwrap();
            let first = workbench.terminal.active_session_id().unwrap();
            super::super::tests::frame(&mut context, &mut workbench);
            change_directory(&mut workbench, first, &first_cwd);
            if !attached {
                assert_eq!(workbench.working_directory(), base);
                assert_eq!(
                    workbench.window_title(),
                    format!("bEd • {}", base.display())
                );
            }
            let first_pid = workbench.terminal.process_id(first).unwrap();

            workbench.dispatch(WindowCommand::NewTerminal).unwrap();
            let second = workbench.terminal.active_session_id().unwrap();
            super::super::tests::frame(&mut context, &mut workbench);
            wait_for_directory(&mut workbench, second, &base);
            change_directory(&mut workbench, second, &second_cwd);
            if !attached {
                assert_eq!(workbench.working_directory(), base);
            }

            // Focusing an older shell changes the split source, not the base.
            let first_tab = workbench.terminal_panel_id(first).unwrap();
            let index = workbench
                .tabs
                .iter()
                .position(|tab| tab.id == first_tab)
                .unwrap();
            assert!(workbench.switch_to_tab(index));
            workbench.open_or_focus(&viewer).unwrap();
            assert!(!workbench.focused_terminal());
            assert_eq!(workbench.terminal.active_session_id(), Some(first));
            if !attached {
                assert_eq!(workbench.working_directory(), base);
            } else {
                assert_eq!(
                    workbench.working_directory(),
                    dir.path("project").canonicalize().unwrap()
                );
            }
            workbench.dispatch(WindowCommand::NewTerminal).unwrap();
            let third = workbench.terminal.active_session_id().unwrap();
            super::super::tests::frame(&mut context, &mut workbench);
            wait_for_directory(&mut workbench, third, &base);

            let second_tab = workbench.terminal_panel_id(second).unwrap();
            let index = workbench
                .tabs
                .iter()
                .position(|tab| tab.id == second_tab)
                .unwrap();
            assert!(workbench.switch_to_tab(index));
            workbench.open_or_focus(&viewer).unwrap();
            assert_eq!(workbench.terminal.active_session_id(), Some(second));
            let split = workbench.split_last_terminal(false).unwrap();
            let split_session = workbench
                .tabs
                .iter()
                .find(|tab| tab.id == split)
                .unwrap()
                .panel
                .terminal_id()
                .unwrap();
            super::super::tests::frame(&mut context, &mut workbench);
            wait_for_directory(&mut workbench, split_session, &base);
            let area = workbench.area_for_panel(second_tab).unwrap();
            let duplicate = workbench
                .duplicate_panel(second_tab, area)
                .unwrap()
                .unwrap();
            let duplicated_session = workbench
                .tabs
                .iter()
                .find(|tab| tab.id == duplicate)
                .unwrap()
                .panel
                .terminal_id()
                .unwrap();
            super::super::tests::frame(&mut context, &mut workbench);
            wait_for_directory(&mut workbench, duplicated_session, &base);
            assert_eq!(workbench.terminal.process_id(first), Some(first_pid));
            assert_eq!(workbench.terminal.session_count(), initial_sessions + 5);
            assert_eq!(workbench.workspace_spec.is_some(), attached);
            workbench.cleanup().unwrap();
        }
    }

    #[test]
    fn ordinary_terminals_get_distinct_companion_views_and_reuse_their_own_view() {
        let dir = TempDir::new();
        dir.write("file.txt", b"shared text");
        let mut workbench = super::super::tests::workspace(&dir);
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let first = workbench.terminal.active_session_id().unwrap();
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let second = workbench.terminal.active_session_id().unwrap();
        workbench
            .open_terminal_companion(&dir.path("file.txt"), Some(first))
            .unwrap();
        let first_view = workbench.active_panel_id();
        workbench
            .open_terminal_companion(&dir.path("file.txt"), Some(second))
            .unwrap();
        let second_view = workbench.active_panel_id();
        assert_ne!(first_view, second_view);
        assert_eq!(workbench.session.document_ids().len(), 1);
        workbench
            .open_terminal_companion(&dir.path("file.txt"), Some(first))
            .unwrap();
        assert_eq!(workbench.active_panel_id(), first_view);
        assert_eq!(workbench.panel_count("document"), 2);
        assert!(workbench.workspace_spec.is_none());
        assert!(workbench.shell_integration.bridge.is_some());
        workbench.cleanup().unwrap();
    }

    #[test]
    fn companion_files_reuse_a_tiled_area_and_terminal_splits_use_the_same_layout() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let dir = TempDir::new();
        let first_file = dir.write("first.txt", b"first companion");
        let second_file = dir.write("second.txt", b"second companion");
        let mut workbench = super::super::tests::workspace(&dir);
        while !workbench.tabs.is_empty() {
            assert!(workbench.close_tab(0).unwrap());
        }
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let terminal = workbench.terminal.active_session_id().unwrap();
        let source = workbench.terminal_panel_id(terminal).unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        workbench
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for _ in 0..3 {
            super::super::tests::frame(&mut context, &mut workbench);
        }
        let source_area = workbench.area_for_panel(source).unwrap();

        workbench
            .open_terminal_companion(&first_file, Some(terminal))
            .unwrap();
        let first = workbench.active_panel_id().unwrap();
        for _ in 0..3 {
            super::super::tests::frame(&mut context, &mut workbench);
        }
        let companion_area = workbench.area_for_panel(first).unwrap();
        assert_ne!(companion_area, source_area);
        let source_rect = workbench.tiling.layout.area(source_area).rect;
        let companion_rect = workbench.tiling.layout.area(companion_area).rect;
        assert_eq!(source_rect.max[0], companion_rect.min[0]);
        assert_eq!(source_rect.min[1], companion_rect.min[1]);
        assert_eq!(source_rect.max[1], companion_rect.max[1]);
        let count = workbench.tiling.layout.areas.len();

        workbench
            .open_terminal_companion(&second_file, Some(terminal))
            .unwrap();
        let second = workbench.active_panel_id().unwrap();
        for _ in 0..3 {
            super::super::tests::frame(&mut context, &mut workbench);
        }
        assert_eq!(workbench.area_for_panel(second), Some(companion_area));
        assert_eq!(workbench.tiling.layout.areas.len(), count);

        let split = workbench.split_last_terminal(true).unwrap();
        for _ in 0..3 {
            super::super::tests::frame(&mut context, &mut workbench);
        }
        let split_area = workbench.area_for_panel(split).unwrap();
        let source_rect = workbench.tiling.layout.area(source_area).rect;
        let split_rect = workbench.tiling.layout.area(split_area).rect;
        assert_eq!(source_rect.max[1], split_rect.min[1]);
        assert_eq!(source_rect.min[0], split_rect.min[0]);
        assert_eq!(source_rect.max[0], split_rect.max[0]);
        assert_eq!(workbench.tiling.layout.areas.len(), count + 1);
        assert!(workbench.shell_integration.pending.is_empty());
        workbench.cleanup().unwrap();
    }

    #[test]
    fn terminal_splits_refuse_small_areas_without_spawning_or_retrying() {
        use crate::workspace::tiling::{EXTENT, Layout};

        for down in [false, true] {
            let dir = TempDir::new();
            let mut workbench = super::super::tests::workspace(&dir);
            while !workbench.tabs.is_empty() {
                assert!(workbench.close_tab(0).unwrap());
            }
            workbench.dispatch(WindowCommand::NewTerminal).unwrap();
            let source = workbench.active_panel_id().unwrap();
            let axis = usize::from(down);
            let mut layout = Layout::default();
            let area = layout.split(1, axis, 8_500, 1_000).unwrap();
            workbench.tiling = TilingState::new(layout);
            workbench.tiling.assign(source, area);
            workbench.tiling_ui.minimum = [1_000; 2];
            workbench.dock_built = true;

            let state = workbench.current_tiling().clone();
            let panels = workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
            let sessions = workbench.terminal.session_ids();
            let next = workbench.next_tab;
            let focused = workbench.active_panel_id();
            let command = if down {
                WindowCommand::SplitDown
            } else {
                WindowCommand::SplitRight
            };
            let error = workbench.dispatch(command).unwrap_err();
            assert!(error.to_string().contains("too small to split"));
            assert_eq!(workbench.current_tiling(), &state);
            assert_eq!(
                workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
                panels
            );
            assert_eq!(workbench.terminal.session_ids(), sessions);
            assert_eq!(workbench.next_tab, next);
            assert_eq!(workbench.active_panel_id(), focused);
            assert!(workbench.shell_integration.pending.is_empty());

            workbench
                .edit_tiling()
                .layout
                .move_border(axis, 8_500, [0, EXTENT], EXTENT / 2, 1_000);
            let expanded = workbench.current_tiling().clone();
            workbench.finish_shell_integration();
            assert_eq!(workbench.current_tiling(), &expanded);
            assert_eq!(workbench.terminal.session_ids(), sessions);
            assert_eq!(workbench.next_tab, next);
            workbench.cleanup().unwrap();
        }
    }

    #[test]
    fn companion_in_a_small_terminal_area_opens_as_a_tab_without_a_delayed_split() {
        use crate::workspace::tiling::{EXTENT, Layout};

        let dir = TempDir::new();
        let file = dir.write("companion.txt", b"companion");
        let mut workbench = super::super::tests::workspace(&dir);
        while !workbench.tabs.is_empty() {
            assert!(workbench.close_tab(0).unwrap());
        }
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let terminal = workbench.terminal.active_session_id().unwrap();
        let source = workbench.terminal_panel_id(terminal).unwrap();
        let mut layout = Layout::default();
        let area = layout.split(1, 0, 8_500, 1_000).unwrap();
        workbench.tiling = TilingState::new(layout);
        workbench.tiling.assign(source, area);
        workbench.tiling_ui.minimum = [1_000; 2];
        workbench.dock_built = true;
        let original = workbench.current_tiling().layout.clone();

        workbench
            .open_terminal_companion(&file, Some(terminal))
            .unwrap();
        let companion = workbench.active_panel_id().unwrap();
        workbench.finish_shell_integration();
        assert_eq!(workbench.area_for_panel(companion), Some(area));
        assert_eq!(workbench.current_tiling().layout, original);
        assert!(workbench.shell_integration.pending.is_empty());

        workbench
            .edit_tiling()
            .layout
            .move_border(0, 8_500, [0, EXTENT], EXTENT / 2, 1_000);
        let expanded = workbench.current_tiling().clone();
        workbench.finish_shell_integration();
        assert_eq!(workbench.current_tiling(), &expanded);
        assert_eq!(workbench.area_for_panel(companion), Some(area));
        workbench.cleanup().unwrap();
    }
}
