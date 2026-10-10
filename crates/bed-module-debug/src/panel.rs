//! Immediate-mode debugger presentation. Actions are applied after drawing.
use super::*;
use bed_ui::{
    presentation::{fit_text, readable_color, same_line_if_fits},
    util::popup_style::{controls_style, tooltip_text},
};
use dear_imgui_rs::{
    Condition, InputTextFlags, StyleColor, StyleVar, TabBarFittingPolicy, TabBarOptions,
    WindowFlags,
};

impl Debugger {
    pub(super) fn draw(&mut self, ui: &Ui, root: &Path) -> Vec<Action> {
        let mut actions = Vec::new();
        let _controls = controls_style(ui);
        let fs = ui.current_font_size();
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.5, fs * 0.3]));
        let _padding = ui.push_style_var(StyleVar::FramePadding([fs * 0.45, fs * 0.2]));
        let active = self.active();
        let paused = self
            .session
            .as_ref()
            .is_some_and(|s| s.state == SessionState::Stopped);
        let running = self
            .session
            .as_ref()
            .is_some_and(|s| s.state == SessionState::Running);
        let style = ui.clone_style();
        let spacing = style.item_spacing()[0];
        let button_width =
            |label: &str| ui.calc_text_size(label)[0] + style.frame_padding()[0] * 2.0;
        let configure_label = if self.configure { "Done" } else { "Setup" };
        let names: Vec<_> = self.profiles.iter().map(|p| p.name.clone()).collect();
        let available = ui.content_region_avail()[0];
        ui.set_next_item_width(
            (available - button_width(configure_label) - spacing)
                .max(60.0)
                .min(available),
        );
        if ui.combo_simple_string("##debug_profile", &mut self.selected, &names) {
            self.discovery = None;
            self.cargo_workspace = None;
            if self.profiles[self.selected].cargo.is_some() {
                actions.push(Action::Discover);
            }
        }
        same_line_if_fits(ui, button_width(configure_label));
        if ui.button(configure_label) {
            self.configure = !self.configure;
        }
        let state = if self.build.is_some() {
            "Building…".to_owned()
        } else if let Some(s) = &self.session {
            match s.state {
                SessionState::Initializing | SessionState::Launching => "Starting debugger…".into(),
                SessionState::Running => "Running".into(),
                SessionState::Stopped => format!("Paused: {}", s.stop_reason),
                SessionState::Stopping => "Stopping…".into(),
                SessionState::Terminated => format!(
                    "Exited{}",
                    s.exit_code.map(|c| format!(" ({c})")).unwrap_or_default()
                ),
                SessionState::Failed => "Debugger failed".into(),
            }
        } else {
            "Ready".into()
        };
        let state_color = readable_color(
            ui,
            if paused {
                [0.95, 0.7, 0.3, 1.0]
            } else if running {
                [0.4, 0.75, 0.5, 1.0]
            } else {
                ui.style_color(StyleColor::TextDisabled)
            },
        );
        let terminal_width = if self.terminal.is_some() {
            button_width("Terminal") + spacing
        } else {
            0.0
        };
        ui.align_text_to_frame_padding();
        ui.text_colored(
            state_color,
            fit_text(
                ui,
                &state,
                (ui.content_region_avail()[0] - terminal_width).max(1.0),
            ),
        );
        if ui.is_item_hovered() {
            tooltip_text(ui, state);
        }
        if self.terminal.is_some() {
            same_line_if_fits(ui, button_width("Terminal"));
            if ui.small_button("Terminal") {
                actions.push(Action::ShowTerminal);
            }
            if ui.is_item_hovered() {
                tooltip_text(
                    ui,
                    "Program input and output. Debugger expressions and LLDB messages appear in the Console tab.",
                );
            }
        }
        for (message, color) in [
            (
                self.error.as_deref(),
                readable_color(ui, [0.95, 0.35, 0.3, 1.0]),
            ),
            (
                self.source_notice.as_deref(),
                ui.style_color(StyleColor::TextDisabled),
            ),
        ] {
            if let Some(message) = message {
                ui.text_colored(color, fit_text(ui, message, ui.content_region_avail()[0]));
                if ui.is_item_hovered() {
                    bed_ui::util::popup_style::tooltip(ui, || {
                        let _wrap = ui.push_text_wrap_pos(fs * 36.0);
                        ui.text(message);
                    });
                }
            }
        }
        for (index, (label, hint, enabled, action)) in [
            (
                if paused { "Continue" } else { "Start" },
                if paused {
                    "Continue (F5)"
                } else {
                    "Start debugging (F5)"
                },
                !active || paused,
                if paused {
                    Action::Continue
                } else {
                    Action::Start
                },
            ),
            ("Pause", "Pause program", running, Action::Pause),
            ("Stop", "Stop debugging (Shift+F5)", active, Action::Stop),
            ("Restart", "Restart debugging", active, Action::Restart),
            ("Over", "Step over (F10)", paused, Action::Over),
            ("Into", "Step into (F11)", paused, Action::Into),
            ("Out", "Step out (Shift+F11)", paused, Action::Out),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                same_line_if_fits(ui, button_width(label));
            }
            {
                let _disabled = ui.begin_disabled_with_cond(!enabled);
                let _primary = (index == 0).then(|| {
                    ui.push_style_color(StyleColor::Button, ui.style_color(StyleColor::Header))
                });
                if ui.button(label) {
                    actions.push(action);
                }
            }
            if ui.is_item_hovered_with_flags(dear_imgui_rs::ItemHoveredFlags::ALLOW_WHEN_DISABLED) {
                tooltip_text(ui, hint);
            }
        }
        if self.configure {
            ui.separator();
            ui.child_window("debug_setup")
                .size([0.0, 0.0])
                .build(ui, || {
                    self.draw_configuration(ui, root, &mut actions);
                });
            return actions;
        }
        if let Some(_tabs) = ui.tab_bar_with_flags(
            "debug_tabs",
            TabBarOptions::new().fitting_policy(TabBarFittingPolicy::Shrink),
        ) {
            if let Some(_tab) = ui.tab_item("Inspect") {
                self.draw_inspection(ui, paused, &mut actions);
            }
            if let Some(_tab) = ui.tab_item("Breakpoints") {
                ui.child_window("debug_breakpoints").build(ui, || {
                    self.draw_breakpoints(ui, root, &mut actions);
                });
            }
            if let Some(_tab) = ui.tab_item("Build") {
                ui.child_window("debug_build_log")
                    .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
                    .build(ui, || {
                        ui.text(&self.build_log);
                        if self.build.is_some() && ui.scroll_y() >= ui.scroll_max_y() - 24.0 {
                            ui.set_scroll_here_y(1.0);
                        }
                    });
            }
            if let Some(_tab) = ui.tab_item("Console") {
                ui.checkbox("LLDB commands", &mut self.console_commands);
                let height =
                    (ui.content_region_avail()[1] - ui.frame_height_with_spacing()).max(1.0);
                ui.child_window("debug_console_log")
                    .size([0.0, height])
                    .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
                    .build(ui, || {
                        ui.text(&self.console_log);
                    });
                ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
                let submitted = ui
                    .input_text(
                        if self.console_commands {
                            "##lldb_command"
                        } else {
                            "##debug_expression"
                        },
                        &mut self.console_input,
                    )
                    .hint(if self.console_commands {
                        "LLDB command (Enter to run)"
                    } else {
                        "Expression in selected frame (Enter to evaluate)"
                    })
                    .flags(InputTextFlags::ENTER_RETURNS_TRUE)
                    .build();
                if submitted && !self.console_input.trim().is_empty() {
                    let input = std::mem::take(&mut self.console_input);
                    let expression = if self.console_commands {
                        format!("`{input}")
                    } else {
                        input.clone()
                    };
                    append_log(&mut self.console_log, &format!("> {input}\n"));
                    actions.push(Action::Evaluate(
                        expression,
                        if self.console_commands {
                            EvaluateContext::Repl
                        } else {
                            EvaluateContext::Watch
                        },
                    ));
                }
            }
        }
        actions
    }

    fn draw_configuration(&mut self, ui: &Ui, _root: &Path, actions: &mut Vec<Action>) {
        let fs = ui.current_font_size();
        let button_width =
            |label: &str| ui.calc_text_size(label)[0] + ui.clone_style().frame_padding()[0] * 2.0;
        ui.text("Launch profile");
        if ui.small_button("New Profile") {
            self.profiles.push(DebugProfile {
                name: format!("Debug {}", self.profiles.len() + 1),
                ..Default::default()
            });
            self.selected = self.profiles.len() - 1;
            self.discovery = None;
            self.cargo_workspace = None;
        }
        same_line_if_fits(ui, button_width("Duplicate"));
        if ui.small_button("Duplicate") {
            let mut duplicate = self.profiles[self.selected].clone();
            duplicate.name.push_str(" copy");
            self.profiles.push(duplicate);
            self.selected = self.profiles.len() - 1;
            self.discovery = None;
            self.cargo_workspace = None;
            if self.profiles[self.selected].cargo.is_some() {
                actions.push(Action::Discover);
            }
        }
        same_line_if_fits(ui, button_width("Delete Profile"));
        {
            let _disabled = ui.begin_disabled_with_cond(self.profiles.len() <= 1);
            if ui.small_button("Delete Profile") {
                self.profiles.remove(self.selected);
                self.selected = self.selected.min(self.profiles.len() - 1);
                self.discovery = None;
                self.cargo_workspace = None;
                if self.profiles[self.selected].cargo.is_some() {
                    actions.push(Action::Discover);
                }
            }
        }
        let profile = &mut self.profiles[self.selected];
        form_input(ui, "Name", &mut profile.name, "Profile name");
        let mut cargo = profile.cargo.is_some();
        if ui.checkbox("Cargo target", &mut cargo) {
            profile.cargo = cargo.then(CargoLaunch::default);
            self.cargo_workspace = None;
            self.discovery = None;
            if cargo {
                actions.push(Action::Discover);
            }
        }
        if let Some(cargo) = &mut profile.cargo {
            ui.text_disabled("Cargo manifest");
            ui.set_next_item_width(
                (ui.content_region_avail()[0] - button_width("Browse") - fs * 0.5).max(1.0),
            );
            if ui
                .input_text("##cargo_manifest", &mut cargo.manifest_path)
                .build()
            {
                self.cargo_workspace = None;
                self.discovery = None;
            }
            same_line_if_fits(ui, button_width("Browse"));
            if ui.small_button("Browse##cargo") {
                actions.push(Action::BrowseManifest(self.selected));
            }
            if ui.small_button("Refresh Targets") {
                actions.push(Action::Discover);
            }
            if self.discovery.is_some() {
                same_line_if_fits(ui, ui.calc_text_size("Discovering…")[0]);
                ui.text_disabled("Discovering…");
            }
            if let Some(workspace) = &self.cargo_workspace {
                let names: Vec<_> = workspace
                    .targets
                    .iter()
                    .map(|t| {
                        format!(
                            "{} / {} ({})",
                            t.package,
                            t.target.name,
                            t.target.kind.label()
                        )
                    })
                    .collect();
                let mut selected = workspace
                    .targets
                    .iter()
                    .position(|t| t.package == cargo.package && t.target == cargo.target)
                    .unwrap_or(0);
                ui.text_disabled("Target");
                ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
                if !names.is_empty()
                    && ui.combo_simple_string("##cargo_target", &mut selected, &names)
                {
                    let target = &workspace.targets[selected];
                    cargo.package = target.package.clone();
                    cargo.target = target.target.clone();
                }
                if let Some(target) = workspace.targets.get(selected)
                    && !target.required_features.is_empty()
                {
                    let _wrap = ui.push_text_wrap_pos(0.0);
                    ui.text_disabled(format!(
                        "Required features: {}",
                        target.required_features.join(", ")
                    ));
                }
                if names.is_empty() {
                    ui.text_disabled("No runnable targets found.");
                }
            } else {
                ui.text_disabled("Refresh to choose a Cargo target.");
            }
            let mut features = cargo.features.join(", ");
            if form_input(ui, "Features", &mut features, "Comma-separated features") {
                cargo.features = features
                    .split([',', ' '])
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            ui.checkbox("Default features", &mut cargo.default_features);
            if cargo.target.kind.is_test() {
                form_input(
                    ui,
                    "Test filter",
                    &mut profile.test_filter,
                    "Optional test name",
                );
            }
        } else {
            ui.text_disabled("Executable");
            ui.set_next_item_width(
                (ui.content_region_avail()[0] - button_width("Browse") - fs * 0.5).max(1.0),
            );
            ui.input_text("##executable", &mut profile.program)
                .hint("Path relative to project, or absolute path")
                .build();
            same_line_if_fits(ui, button_width("Browse"));
            if ui.small_button("Browse##binary") {
                actions.push(Action::BrowseProgram(self.selected));
            }
            form_input(
                ui,
                "Build command",
                &mut profile.build_command,
                "Optional; runs in the project directory",
            );
        }
        form_input(
            ui,
            "Working directory",
            &mut profile.cwd,
            "Project directory when empty",
        );
        ui.checkbox("Stop on Entry", &mut profile.stop_on_entry);
        if ui.is_item_hovered() {
            tooltip_text(
                ui,
                "Pause before main, which can stop in loader or startup code without source. Continue (F5) to reach your breakpoints.",
            );
        }
        ui.separator();
        ui.text("Arguments");
        if ui.is_item_hovered() {
            tooltip_text(ui, "One literal program argument per row");
        }
        let mut remove = None;
        for (i, arg) in profile.args.iter_mut().enumerate() {
            let _id = ui.push_id(&format!("argument_{i}"));
            ui.set_next_item_width(
                (ui.content_region_avail()[0] - button_width("Remove") - fs * 0.5).max(1.0),
            );
            ui.input_text("##argument", arg).hint("Argument").build();
            ui.same_line();
            if ui.small_button("Remove") {
                remove = Some(i);
            }
        }
        if let Some(i) = remove {
            profile.args.remove(i);
        }
        if ui.small_button("Add Argument") {
            profile.args.push(String::new());
        }
        if ui.collapsing_header("Advanced", TreeNodeFlags::empty()) {
            form_input(
                ui,
                "Adapter executable",
                &mut self.adapter_path,
                "Automatic lldb-dap discovery when empty",
            );
            use bed_debug::profile::RustFormatterMode;
            let mut rust_formatters = match profile.rust_formatters {
                RustFormatterMode::Auto => 0,
                RustFormatterMode::Enabled => 1,
                RustFormatterMode::Disabled => 2,
            };
            ui.text_disabled("Rust pretty printers");
            ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
            if ui.combo_simple_string(
                "##rust_pretty_printers",
                &mut rust_formatters,
                &["Automatic", "Enabled", "Disabled"],
            ) {
                profile.rust_formatters = match rust_formatters {
                    1 => RustFormatterMode::Enabled,
                    2 => RustFormatterMode::Disabled,
                    _ => RustFormatterMode::Auto,
                };
            }
            if ui.is_item_hovered() {
                tooltip_text(
                    ui,
                    "Automatic enables readable Rust values for Cargo profiles and Rust workspaces. Choose Enabled for a standalone Rust executable. Pretty printers follow the project's Rust toolchain.",
                );
            }
            ui.text_wrapped("Environment overrides (build and program)");
            let mut environment: Vec<_> = profile
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut removed = None;
            for (i, (name, value)) in environment.iter_mut().enumerate() {
                let _id = ui.push_id(&format!("env_{i}"));
                form_input(ui, "Name", name, "VARIABLE");
                ui.set_next_item_width(
                    (ui.content_region_avail()[0] - button_width("Remove") - fs * 0.5).max(1.0),
                );
                ui.input_text("##value", value).hint("Value").build();
                same_line_if_fits(ui, button_width("Remove"));
                if ui.small_button("Remove") {
                    removed = Some(i);
                }
            }
            if let Some(i) = removed {
                environment.remove(i);
            }
            if ui.small_button("Add Environment Variable") {
                environment.push((format!("VARIABLE_{}", environment.len() + 1), String::new()));
            }
            profile.env = environment.into_iter().collect();
            ui.text_wrapped("Source mappings connect original build directories to your local source directories. Leave empty for ordinary local builds.");
            let mut removed = None;
            for (i, mapping) in profile.source_map.iter_mut().enumerate() {
                let _id = ui.push_id(&format!("source_map_{i}"));
                form_input(
                    ui,
                    "Build directory",
                    &mut mapping[0],
                    "Original source directory",
                );
                form_input(
                    ui,
                    "Local directory",
                    &mut mapping[1],
                    "Local source directory",
                );
                if ui.small_button("Remove") {
                    removed = Some(i);
                }
            }
            if let Some(i) = removed {
                profile.source_map.remove(i);
            }
            if ui.small_button("Add Source Mapping") {
                profile.source_map.push(Default::default());
            }
        }
        ui.separator();
    }

    fn draw_inspection(&mut self, ui: &Ui, paused: bool, actions: &mut Vec<Action>) {
        let fs = ui.current_font_size();
        let width = ui.content_region_avail()[0];
        let height = ui.content_region_avail()[1].max(1.0);
        let style = ui.clone_style();
        let side_by_side = width >= fs * 32.0;
        let stack_height = if side_by_side {
            height
        } else {
            ((height - style.item_spacing()[1]) * 0.35).max(1.0)
        };
        let _padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.5, fs * 0.4]));
        let _rounding = ui.push_style_var(StyleVar::ChildRounding(fs * 0.4));
        ui.child_window("debug_stack")
            .size([if side_by_side { width * 0.35 } else { 0.0 }, stack_height])
            .border(true)
            .build(ui, || {
                ui.text_disabled("CALL STACK");
                if let Some(session) = &self.session {
                    let names: Vec<_> = session
                        .threads
                        .iter()
                        .map(|t| format!("{}: {}", t.id, t.name))
                        .collect();
                    let mut selected = session
                        .threads
                        .iter()
                        .position(|t| Some(t.id) == session.selected_thread)
                        .unwrap_or(0);
                    ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
                    if !names.is_empty()
                        && ui.combo_simple_string("##debug_thread", &mut selected, &names)
                    {
                        actions.push(Action::Thread(session.threads[selected].id));
                    }
                    for frame in &session.frames {
                        let _id = ui.push_id(&format!("frame_{}", frame.id));
                        let label = fit_text(ui, &frame.name, ui.content_region_avail()[0]);
                        if ui
                            .selectable_config(format!("{label}###frame"))
                            .selected(Some(frame.id) == session.selected_frame)
                            .build()
                        {
                            actions.push(Action::Frame(frame.id));
                        }
                        if ui.is_item_hovered() {
                            tooltip_text(ui, &frame.name);
                        }
                        if let Some(path) = &frame.source {
                            let location = format!(
                                "{}:{}",
                                Path::new(path)
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy(),
                                frame.line
                            );
                            ui.text_disabled(fit_text(ui, &location, ui.content_region_avail()[0]));
                            if ui.is_item_hovered() {
                                tooltip_text(ui, format!("{path}:{}", frame.line));
                            }
                            if self.source_paths.get(&frame.id)
                                .and_then(|path| path.to_str())
                                .is_some_and(|path| self.source.changed(path)) {
                                let _wrap = ui.push_text_wrap_pos(0.0);
                                ui.text_colored(
                                    [1.0, 0.7, 0.2, 1.0],
                                    "Source changed; restart to update",
                                );
                            }
                        } else {
                            let _wrap = ui.push_text_wrap_pos(0.0);
                            ui.text_disabled("No source for this runtime or assembly frame.");
                            if paused && Some(frame.id) == session.selected_frame {
                                ui.text_disabled(
                                    "Continue (F5) to reach a breakpoint, or select a frame with source.",
                                );
                            }
                        }
                    }
                    if session.frames.is_empty() {
                        ui.text_wrapped("Pause to inspect stack frames.");
                    }
                } else {
                    ui.text_wrapped("Start debugging to see stack frames.");
                }
            });
        if side_by_side {
            ui.same_line();
        }
        ui.child_window("debug_variables")
            .size([
                0.0,
                if side_by_side {
                    height
                } else {
                    (height - stack_height - style.item_spacing()[1]).max(1.0)
                },
            ])
            .border(true)
            .build(ui, || {
                ui.text_disabled("VARIABLES");
                if !paused {
                    let _muted = ui.push_style_color(
                        StyleColor::Text,
                        ui.style_color(StyleColor::TextDisabled),
                    );
                    ui.text_wrapped("Pause the program to inspect variables.");
                }
                if let Some(session) = &self.session {
                    for scope in &session.scopes {
                        if let Some(_tree) = ui
                            .tree_node_config(format!("scope_name_{}", scope.name))
                            .label(&scope.name)
                            .opened(scope.is_locals(), Condition::FirstUseEver)
                            .push()
                        {
                            draw_variables(ui, session, scope.variables_reference, actions, 0);
                        }
                    }
                }
                ui.separator();
                ui.text("Watches");
                for (i, expression) in self.watches.iter().enumerate() {
                    let _id = ui.push_id(&format!("watch_{i}"));
                    let remove_width =
                        ui.calc_text_size("Remove")[0] + style.frame_padding()[0] * 2.0;
                    ui.text(fit_text(
                        ui,
                        expression,
                        (ui.content_region_avail()[0] - remove_width - style.item_spacing()[0])
                            .max(1.0),
                    ));
                    if ui.is_item_hovered() {
                        tooltip_text(ui, expression);
                    }
                    same_line_if_fits(ui, remove_width);
                    if ui.small_button("Remove") {
                        actions.push(Action::RemoveWatch(i));
                    }
                    let _wrap = ui.push_text_wrap_pos(0.0);
                    ui.text_disabled(
                        self.watch_values
                            .get(expression)
                            .map(String::as_str)
                            .unwrap_or(if paused {
                                "Evaluating…"
                            } else {
                                "Unavailable while running"
                            }),
                    );
                }
                let add_width = ui.calc_text_size("Add")[0] + style.frame_padding()[0] * 2.0;
                let available = ui.content_region_avail()[0];
                ui.set_next_item_width(
                    (available - add_width - style.item_spacing()[0])
                        .max(40.0)
                        .min(available),
                );
                if ui
                    .input_text("##new_watch", &mut self.watch_input)
                    .hint("Add watch expression")
                    .flags(InputTextFlags::ENTER_RETURNS_TRUE)
                    .build()
                    && !self.watch_input.trim().is_empty()
                {
                    actions.push(Action::Watch(std::mem::take(&mut self.watch_input)));
                }
                same_line_if_fits(ui, add_width);
                if ui.small_button("Add") && !self.watch_input.trim().is_empty() {
                    actions.push(Action::Watch(std::mem::take(&mut self.watch_input)));
                }
            });
    }

    fn draw_breakpoints(&self, ui: &Ui, root: &Path, actions: &mut Vec<Action>) {
        let fs = ui.current_font_size();
        let button_width =
            |label: &str| ui.calc_text_size(label)[0] + ui.clone_style().frame_padding()[0] * 2.0;
        ui.text_disabled(format!(
            "{} breakpoints",
            self.source
                .breakpoints
                .values()
                .map(Vec::len)
                .sum::<usize>()
        ));
        if ui.is_item_hovered() {
            tooltip_text(ui, "Breakpoints last until you switch projects or quit.");
        }
        same_line_if_fits(ui, button_width("Clear All"));
        let _disabled = ui.begin_disabled_with_cond(self.source.breakpoints.is_empty());
        if ui.small_button("Clear All") {
            actions.push(Action::ClearBreakpoints);
        }
        drop(_disabled);
        if self.source.breakpoints.is_empty() {
            let _wrap = ui.push_text_wrap_pos(0.0);
            ui.text_disabled("Click the source gutter or press F9 to add a breakpoint.");
        }
        for (path, breakpoints) in &self.source.breakpoints {
            let _id = ui.push_id(path);
            let relative = Path::new(path)
                .strip_prefix(root)
                .unwrap_or(Path::new(path));
            let heading = fit_text(
                ui,
                &relative.to_string_lossy(),
                (ui.content_region_avail()[0] - fs * 2.0).max(1.0),
            );
            let open =
                ui.collapsing_header(format!("{heading}###file"), TreeNodeFlags::DEFAULT_OPEN);
            if ui.is_item_hovered() {
                tooltip_text(ui, path);
            }
            if !open {
                continue;
            }
            for breakpoint in breakpoints {
                let _id = ui.push_id(&format!("breakpoint_{}", breakpoint.id));
                let mut enabled = breakpoint.enabled;
                if ui.checkbox("##enabled", &mut enabled) {
                    actions.push(Action::BreakpointEnabled(
                        path.clone(),
                        breakpoint.id,
                        enabled,
                    ));
                }
                let line = format!("Line {}", breakpoint.row + 1);
                same_line_if_fits(ui, button_width(&line));
                if ui.small_button(line) {
                    actions.push(Action::Navigate(path.clone(), breakpoint.row));
                }
                let status = if !enabled {
                    "Disabled"
                } else {
                    match breakpoint.status {
                        bed_editor_ui::BreakpointStatus::Verified => "Verified",
                        bed_editor_ui::BreakpointStatus::Rejected => "Unresolved",
                        bed_editor_ui::BreakpointStatus::Pending => "Pending",
                    }
                };
                same_line_if_fits(ui, ui.calc_text_size(status)[0]);
                ui.text_disabled(status);
                same_line_if_fits(ui, button_width("Remove"));
                if ui.small_button("Remove") {
                    actions.push(Action::RemoveBreakpoint(path.clone(), breakpoint.id));
                }
                if !breakpoint.message.is_empty() {
                    ui.text_wrapped(&breakpoint.message);
                }
                if self.source.changed(path) {
                    let _wrap = ui.push_text_wrap_pos(0.0);
                    ui.text_disabled(
                        "Changed since launch; moved/new breakpoints apply on restart.",
                    );
                }
            }
        }
    }
}

/// Keep form labels above full-width fields, including in narrow docks.
fn form_input(ui: &Ui, label: &str, value: &mut String, hint: &str) -> bool {
    ui.text_disabled(label);
    ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
    ui.input_text(format!("##{label}"), value)
        .hint(hint)
        .build()
}

fn draw_variables(
    ui: &Ui,
    session: &DebugSession,
    reference: i64,
    actions: &mut Vec<Action>,
    depth: usize,
) {
    let _id = ui.push_id(&format!("variables_{reference}"));
    if depth >= 16 {
        ui.text_disabled("Expand further using the console.");
        return;
    }
    if let Some(error) = session.variable_errors.get(&reference) {
        ui.text_wrapped(error);
        if ui.small_button("Retry") {
            actions.push(Action::RetryVariables(reference));
        }
        return;
    }
    let Some(variables) = session.variables.get(&reference) else {
        actions.push(Action::Variables(reference));
        ui.text_disabled("Loading…");
        return;
    };
    for (index, variable) in variables.iter().enumerate() {
        let _id = ui.push_id(&format!("variable_{reference}_{index}"));
        let detail = format!("{} = {}", variable.name, variable.value);
        let label = fit_text(
            ui,
            &detail,
            (ui.content_region_avail()[0] - ui.current_font_size() * 1.5).max(1.0),
        );
        let node = if variable.variables_reference > 0 {
            ui.tree_node_config("value").label(&label).push()
        } else {
            ui.text(&label);
            None
        };
        if ui.is_item_hovered() {
            bed_ui::util::popup_style::tooltip(ui, || {
                let _wrap = ui.push_text_wrap_pos(ui.current_font_size() * 36.0);
                if let Some(type_name) = &variable.type_name {
                    ui.text_disabled(type_name);
                }
                ui.text(&detail);
            });
        }
        if let Some(_node) = node {
            draw_variables(
                ui,
                session,
                variable.variables_reference,
                actions,
                depth + 1,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Context, FramePrepareOptions, WindowFlags, sys};
    use std::ffi::CStr;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn inspection_loads_only_locals_and_variable_errors_require_explicit_retry() {
        use dear_imgui_rs::MouseButton;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut session = DebugSession::launch_with_args(
            Path::new("/bin/sleep"),
            &["30".into()],
            LaunchConfig {
                program: "/unused/program".into(),
                ..Default::default()
            },
        )
        .unwrap();
        session.state = SessionState::Stopped;
        session.scopes = serde_json::from_value(json!([
            {"name":"Locals", "variablesReference":10, "expensive":false},
            {"name":"Globals", "variablesReference":20, "expensive":false},
            {"name":"Registers", "variablesReference":30, "expensive":false}
        ]))
        .unwrap();
        let mut debugger = Debugger {
            session: Some(session),
            ..Default::default()
        };
        for _ in 0..2 {
            context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut actions = Vec::new();
            ui.window("scope_defaults")
                .position([0.0, 0.0], Condition::Always)
                .size([600.0, 320.0], Condition::Always)
                .build(|| debugger.draw_inspection(ui, true, &mut actions));
            assert!(matches!(actions.as_slice(), [Action::Variables(10)]));
            drop(context.render_legacy());
        }

        let session = debugger.session.as_mut().unwrap();
        session.variable_errors.insert(
            10,
            "Loading variables timed out. Try again when needed.".into(),
        );
        session
            .variable_errors
            .insert(20, "The adapter could not read these variables.".into());

        fn error_frame(
            context: &mut Context,
            session: &DebugSession,
        ) -> (Vec<Action>, [[f32; 2]; 2], [u32; 2]) {
            context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut actions = Vec::new();
            let mut points = [[0.0; 2]; 2];
            let mut ids = [0; 2];
            ui.window("variable_errors")
                .position([0.0, 0.0], Condition::Always)
                .size([420.0, 260.0], Condition::Always)
                .flags(WindowFlags::NO_TITLE_BAR)
                .build(|| {
                    for (index, reference) in [10, 20].into_iter().enumerate() {
                        draw_variables(ui, session, reference, &mut actions, 0);
                        let min = ui.item_rect_min();
                        let max = ui.item_rect_max();
                        points[index] = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                        ids[index] = ui.with_bound_context(|| unsafe { sys::igGetItemID() });
                    }
                });
            drop(context.render_legacy());
            (actions, points, ids)
        }
        for _ in 0..2 {
            let (actions, _, ids) = error_frame(&mut context, session);
            assert!(actions.is_empty(), "Errors must not retry every frame");
            assert_ne!(ids[0], ids[1], "Each Retry button needs a distinct ID");
        }
        let (_, points, _) = error_frame(&mut context, session);
        context.io_mut().add_mouse_pos_event(points[1]);
        assert!(error_frame(&mut context, session).0.is_empty());
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        assert!(error_frame(&mut context, session).0.is_empty());
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        let actions = error_frame(&mut context, session).0;
        assert!(matches!(actions.as_slice(), [Action::RetryVariables(20)]));
    }

    #[test]
    fn setup_and_breakpoints_remain_usable_in_narrow_panels() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut debugger = Debugger::default();
        debugger.profiles.push(DebugProfile {
            args: vec!["a literal argument with spaces".into()],
            env: [("VERY_LONG_VARIABLE_NAME".into(), "value".into())].into(),
            source_map: vec![["/original/build/path".into(), "/local/source/path".into()]],
            ..Default::default()
        });
        debugger
            .source
            .toggle("/project/a/very/long/path/to/source.cpp", 12, 0);
        for size in [[240.0, 360.0], [320.0, 260.0], [800.0, 220.0]] {
            for setup in [true, false] {
                for frame in 0..3 {
                    context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
                    let ui = context.frame();
                    ui.window("debug_tool_layout")
                        .position([0.0; 2], Condition::Always)
                        .size(size, Condition::Always)
                        .flags(WindowFlags::NO_TITLE_BAR)
                        .build(|| {
                            let mut actions = Vec::new();
                            if setup {
                                ui.with_bound_context(|| unsafe {
                                    let window = sys::igGetCurrentWindow();
                                    sys::ImGuiStorage_SetInt(
                                        (*window).DC.StateStorage,
                                        sys::igGetID_Str(c"Advanced".as_ptr()),
                                        1,
                                    );
                                });
                                debugger.draw_configuration(
                                    ui,
                                    Path::new("/project"),
                                    &mut actions,
                                );
                            } else {
                                debugger.draw_breakpoints(ui, Path::new("/project"), &mut actions);
                            }
                            assert!(actions.is_empty());
                            if frame > 1 {
                                ui.with_bound_context(|| unsafe {
                                    assert!(
                                        (*sys::igGetCurrentWindow()).ScrollMax.x <= 4.0,
                                        "Horizontal overflow: {size:?}, setup={setup}"
                                    );
                                });
                            }
                        });
                    drop(context.render_legacy());
                }
            }
        }
    }

    #[test]
    fn compact_panel_keeps_inspection_visible_with_errors_and_source_notices() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut debugger = Debugger::default();
        debugger.profiles.push(DebugProfile::default());
        debugger.terminal = Some(1);
        debugger.source.toggle("main.cpp", 1, 0);
        let error = "Failed to attach to the target process.\nThe debugger adapter could not connect to the program terminal; check the launch output and try again.";
        let notice = "No source for this runtime or assembly frame. Continue (F5) to reach a breakpoint, or select a frame with source.";
        for (size, source_notice) in [
            ([520.0, 260.0], false),
            ([300.0, 340.0], false),
            ([520.0, 260.0], true),
            ([300.0, 340.0], true),
            ([240.0, 360.0], false),
            ([900.0, 210.0], true),
        ] {
            debugger.error = (!source_notice).then(|| error.to_owned());
            debugger.source_notice = source_notice.then(|| notice.to_owned());
            for frame_index in 0..3 {
                context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
                let ui = context.frame();
                ui.window("compact_debug_panel")
                    .position([0.0, 0.0], Condition::Always)
                    .size(size, Condition::Always)
                    .flags(WindowFlags::NO_TITLE_BAR)
                    .build(|| {
                        assert!(debugger.draw(ui, Path::new(".")).is_empty());
                        ui.with_bound_context(|| unsafe {
                            let parent = sys::igGetCurrentWindow();
                            if frame_index > 0 {
                                // Native tab bars can include a few pixels of
                                // trailing item spacing in their content size.
                                assert!((*parent).ScrollMax.x <= 4.0, "size {size:?}, horizontal scroll {}, content width {}, inner width {}", (*parent).ScrollMax.x, (*parent).ContentSize.x, (*parent).InnerRect.Max.x - (*parent).InnerRect.Min.x);
                            }
                            let children = std::slice::from_raw_parts(
                                (*parent).DC.ChildWindows.Data,
                                (*parent).DC.ChildWindows.Size as usize,
                            );
                            let inspection: Vec<_> = children
                                .iter()
                                .copied()
                                .filter(|child| {
                                    let name = CStr::from_ptr((**child).Name).to_string_lossy();
                                    name.contains("debug_stack") || name.contains("debug_variables")
                                })
                                .collect();
                            assert_eq!(inspection.len(), 2, "Both inspection panes must be drawn");
                            let stack = inspection[0];
                            let variables = inspection[1];
                            if size[0] < 400.0 {
                                assert!(
                                    (*variables).Pos.y >= (*stack).Pos.y + (*stack).Size.y,
                                    "Narrow panes must stack vertically"
                                );
                            } else {
                                assert_eq!(
                                    (*stack).Pos.y,
                                    (*variables).Pos.y,
                                    "Wide panes should share a row"
                                );
                            }
                            for child in inspection {
                                assert!(
                                    (*child).Size.y >= 40.0,
                                    "Header must leave inspection space"
                                );
                                assert!((*child).Pos.y >= (*parent).InnerRect.Min.y);
                                assert!((*child).Pos.x >= (*parent).InnerRect.Min.x);
                                assert!(
                                    (*child).Pos.x + (*child).Size.x <= (*parent).InnerRect.Max.x
                                );
                                assert!(
                                    (*child).Pos.y + (*child).Size.y <= (*parent).InnerRect.Max.y,
                                    "Inspection must remain inside the visible panel"
                                );
                            }
                        });
                    });
                assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
            }
        }
    }
}
