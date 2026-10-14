//! Shortcut editing controls shared by application-specific and generic settings views.
//! The component edits a caller-owned draft. It does not access ECS state or persistence.
use bedtime_keybindings::{Bindings, CollisionKind, Overrides, Platform, ShortcutAction, Stroke};
use dear_imgui_rs::{Key, StyleColor, StyleVar, Ui};
use std::collections::BTreeMap;

/// Per-view filtering and recording state. Overrides remain owned by the caller.
#[derive(Default)]
pub struct KeybindingEditor {
    filter: String,
    recording: Option<(String, Option<usize>)>,
    recording_frame: usize,
    #[cfg(test)]
    targets: BTreeMap<String, [f32; 2]>,
}

/// Drawing can edit a draft and consume input even when recording was cancelled.
#[derive(Default)]
pub struct KeybindingEdit {
    pub changed: bool,
    pub suppress_shortcuts: bool,
    pub error: Option<String>,
}

impl KeybindingEditor {
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn cancel_recording(&mut self) {
        self.recording = None;
    }

    /// Inputs from the frame that opened recording are ignored.
    pub fn begin_recording(&mut self, action: &str, index: Option<usize>, frame: usize) {
        self.recording = Some((action.to_owned(), index));
        self.recording_frame = frame;
    }

    /// Draw the registered actions against a mutable draft. Panel labels are supplied
    /// by the host; global and focused actions share the Common group.
    pub fn draw(
        &mut self,
        ui: &Ui,
        actions: &[ShortcutAction],
        groups: &BTreeMap<String, String>,
        overrides: &mut Overrides,
    ) -> KeybindingEdit {
        let mut draft = overrides.clone();
        let mut error = None;
        let was_recording = self.is_recording();
        #[cfg(test)]
        self.targets.clear();
        let size = ui.current_font_size();
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([size * 0.65, size * 0.55]));
        let _padding = ui.push_style_var(StyleVar::FramePadding([size * 0.6, size * 0.3]));
        ui.dummy([0.0, size * 0.5]);
        ui.text_wrapped("Common shortcuts include application commands and actions handled by the focused panel or widget.");
        ui.input_text("Search actions or shortcuts", &mut self.filter)
            .build();
        let bindings = Bindings::new(actions.to_vec(), &draft, Platform::current());
        let collisions = bindings.collisions();
        let filter = self.filter.to_lowercase();
        let mut edit: Option<(bedtime_keybindings::ShortcutAction, Option<usize>, Stroke)> = None;
        if let Some((id, index)) = self.recording.clone() {
            ui.separator();
            let action = actions.iter().find(|a| a.id == id);
            let position = ui.cursor_screen_pos();
            let pulse = (ui.time() as f32 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            ui.get_window_draw_list()
                .add_circle(
                    [position[0] + size * 0.35, position[1] + size * 0.5],
                    size * (0.22 + pulse * 0.05),
                    [0.95, 0.2, 0.22, 0.45 + pulse * 0.55],
                )
                .filled(true)
                .build();
            ui.dummy([size * 0.7, size]);
            ui.same_line();
            ui.text(format!(
                "Recording: {}",
                action.map_or(id.as_str(), |a| a.label.as_str())
            ));
            ui.text_disabled("Press a combination. Escape cancels.");
            let cancel =
                ui.button("Cancel recording") || ui.is_key_pressed_with_repeat(Key::Escape, false);
            ui.same_line();
            let escape = ui.button("Assign Escape");
            #[cfg(test)]
            self.target(ui, "assign_escape");
            if cancel {
                self.recording = None;
            } else if let Some(action) = action {
                let captured = if escape {
                    Some("Escape".parse().unwrap())
                } else if ui.frame_count() > self.recording_frame {
                    bedtime_keybindings::record(ui)
                } else {
                    None
                };
                if let Some(stroke) = captured {
                    edit = Some((action.clone(), index, stroke));
                    self.recording = None;
                }
            } else {
                self.cancel_recording();
            }
            ui.separator();
        }
        let mut group_ids = actions
            .iter()
            .map(|a| match &a.scope {
                bedtime_keybindings::Scope::Global | bedtime_keybindings::Scope::Focused => {
                    "Common".to_owned()
                }
                bedtime_keybindings::Scope::Panel(id) => id.clone(),
            })
            .collect::<Vec<_>>();
        group_ids.sort();
        group_ids.dedup();
        if let Some(index) = group_ids.iter().position(|g| g == "Common") {
            let common = group_ids.remove(index);
            group_ids.insert(0, common);
        }
        for group in group_ids {
            let header = groups.get(&group).cloned().unwrap_or_else(|| group.clone());
            let matching = actions
                .iter()
                .filter(|a| match &a.scope {
                    bedtime_keybindings::Scope::Global | bedtime_keybindings::Scope::Focused => {
                        group == "Common"
                    }
                    bedtime_keybindings::Scope::Panel(id) => id == &group,
                })
                .filter(|action| {
                    format!(
                        "{} {} {} {}",
                        action.id,
                        action.label,
                        action.owner,
                        bindings
                            .for_action(&action.id)
                            .iter()
                            .map(|b| b.shortcut.to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                    .to_lowercase()
                    .contains(&filter)
                })
                .collect::<Vec<_>>();
            if matching.is_empty() {
                continue;
            }
            ui.dummy([0.0, size * 0.35]);
            if !filter.is_empty() {
                ui.set_next_item_open(true);
            }
            if !ui.collapsing_header(&header, dear_imgui_rs::TreeNodeFlags::empty()) {
                continue;
            }
            ui.indent_by(size * 0.8);
            for action in matching {
                ui.dummy([0.0, size * 0.45]);
                let _id = ui.push_id(&action.id);
                ui.text(&action.label);
                ui.same_line();
                ui.text_disabled(if draft.actions.contains_key(&action.id) {
                    "Custom"
                } else {
                    "Default"
                });
                let shortcuts = draft
                    .actions
                    .get(&action.id)
                    .map(|v| v.iter().map(|a| a.shortcut).collect::<Vec<_>>())
                    .unwrap_or_else(|| action.defaults.clone());
                if shortcuts.is_empty() {
                    ui.text_disabled("Unbound");
                }
                let mut remove = None;
                for (index, stroke) in shortcuts.iter().enumerate() {
                    let _index = ui.push_id(index as i32);
                    if ui.button(stroke.label(Platform::current())) {
                        self.recording = Some((action.id.clone(), Some(index)));
                        self.recording_frame = ui.frame_count();
                    }
                    #[cfg(test)]
                    self.target(ui, &format!("{}/binding/{index}", action.id));
                    ui.same_line();
                    if ui.small_button("Remove") {
                        remove = Some(index);
                    }
                    #[cfg(test)]
                    self.target(ui, &format!("{}/remove/{index}", action.id));
                }
                if let Some(index) = remove {
                    let mut candidate = draft.clone();
                    match candidate.remove(action, index) {
                        Ok(()) => draft = candidate,
                        Err(failure) => error = Some(failure),
                    }
                }
                if ui.small_button("Add binding") {
                    self.recording = Some((action.id.clone(), None));
                    self.recording_frame = ui.frame_count();
                }
                #[cfg(test)]
                self.target(ui, &format!("{}/add", action.id));
                ui.same_line();
                if ui.small_button("Clear") {
                    draft.actions.insert(action.id.clone(), Vec::new());
                }
                #[cfg(test)]
                self.target(ui, &format!("{}/clear", action.id));
                ui.same_line();
                if ui.small_button("Restore defaults") {
                    draft.actions.remove(&action.id);
                }
                #[cfg(test)]
                self.target(ui, &format!("{}/defaults", action.id));
                if draft.actions.contains_key(&action.id) {
                    ui.text_disabled(format!(
                        "Defaults: {}",
                        if action.defaults.is_empty() {
                            "Unbound".into()
                        } else {
                            action
                                .defaults
                                .iter()
                                .map(|s| s.label(Platform::current()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ));
                }
                for warning in collisions.iter().filter(|c| {
                    c.kind != CollisionKind::Reuse
                        && (c.first == action.id || c.second == action.id)
                }) {
                    let other = if warning.first == action.id {
                        &warning.second
                    } else {
                        &warning.first
                    };
                    let other = actions
                        .iter()
                        .find(|a| &a.id == other)
                        .map_or(other.as_str(), |a| a.label.as_str());
                    let _font =
                        ui.push_font_with_size(None, ui.clone_style().font_size_base() * 0.85);
                    let _color = ui.push_style_color(StyleColor::Text, [0.78, 0.57, 0.34, 1.0]);
                    ui.text_wrapped(format!(
                        "{} is also assigned to {}.",
                        warning.shortcut, other
                    ));
                }
                ui.dummy([0.0, size * 0.3]);
                ui.separator();
            }
            ui.unindent_by(size * 0.8);
        }
        if actions.is_empty() {
            ui.text_disabled("No shortcut actions registered by this host.");
        }
        if let Some((action, index, stroke)) = edit {
            let mut candidate = draft.clone();
            match candidate.replace(&action, index, stroke) {
                Ok(()) => draft = candidate,
                Err(failure) => error = Some(failure),
            }
        }
        let changed = draft != *overrides;
        *overrides = draft;
        KeybindingEdit {
            changed,
            suppress_shortcuts: was_recording || self.is_recording(),
            error,
        }
    }

    #[cfg(test)]
    fn target(&mut self, ui: &Ui, name: &str) {
        let (min, max) = ui.item_rect();
        self.targets.insert(
            name.to_owned(),
            [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_keybindings::Scope;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton};
    use std::path::PathBuf;

    use crate::IMGUI_TEST_LOCK as IMGUI_LOCK;

    struct Fixture {
        context: Context,
        editor: KeybindingEditor,
        actions: Vec<ShortcutAction>,
        overrides: Overrides,
    }

    impl Fixture {
        fn new() -> Self {
            let mut context = Context::create();
            context.set_ini_filename(None::<PathBuf>).unwrap();
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            Self {
                context,
                editor: KeybindingEditor {
                    filter: "app.open".into(),
                    ..Default::default()
                },
                actions: vec![ShortcutAction::new(
                    "app.open",
                    "Open",
                    "app",
                    Scope::Global,
                    &["Primary+O"],
                )],
                overrides: Overrides::default(),
            }
        }

        fn frame(&mut self, before: impl FnOnce(&Ui, &mut KeybindingEditor)) -> KeybindingEdit {
            self.context
                .prepare_frame(FramePrepareOptions::new([1200.0, 1200.0], 1.0 / 60.0));
            let ui = self.context.frame();
            before(ui, &mut self.editor);
            let mut result = KeybindingEdit::default();
            ui.window("Shortcut fixture")
                .position([0.0; 2], Condition::Always)
                .size([1200.0, 1200.0], Condition::Always)
                .build(|| {
                    result =
                        self.editor
                            .draw(ui, &self.actions, &BTreeMap::new(), &mut self.overrides);
                });
            drop(self.context.render_legacy());
            result
        }

        fn click(&mut self, target: &str) -> KeybindingEdit {
            self.frame(|_, _| {});
            self.frame(|_, _| {});
            let position = *self.editor.targets.get(target).unwrap_or_else(|| {
                panic!(
                    "Missing shortcut control {target}; visible controls: {:?}",
                    self.editor.targets.keys()
                )
            });
            self.context.io_mut().add_mouse_pos_event(position);
            self.frame(|_, _| {});
            self.context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            self.frame(|_, _| {});
            self.context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            self.frame(|_, _| {})
        }
    }

    #[test]
    fn recording_ignores_the_opening_frame_and_suppresses_the_captured_frame() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut fixture = Fixture::new();
        fixture.context.io_mut().add_key_event(Key::ModCtrl, true);
        fixture.context.io_mut().add_key_event(Key::ModAlt, true);
        fixture.context.io_mut().add_key_event(Key::F12, true);
        let opening =
            fixture.frame(|ui, editor| editor.begin_recording("app.open", None, ui.frame_count()));
        assert!(!opening.changed);
        assert!(opening.suppress_shortcuts);
        assert!(fixture.editor.is_recording());
        fixture.context.io_mut().add_key_event(Key::F12, false);
        fixture.frame(|_, _| {});
        fixture.context.io_mut().add_key_event(Key::F11, true);
        let capture = fixture.frame(|_, _| {});
        assert!(capture.changed);
        assert!(capture.suppress_shortcuts);
        assert!(!fixture.editor.is_recording());
        assert_eq!(
            fixture.overrides.actions["app.open"][1]
                .shortcut
                .to_string(),
            "Ctrl+Alt+F11"
        );
    }

    #[test]
    fn escape_cancels_without_changes_and_can_be_assigned_explicitly() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut fixture = Fixture::new();
        fixture.editor.begin_recording("app.open", Some(0), 0);
        fixture.context.io_mut().add_key_event(Key::Escape, true);
        let cancelled = fixture.frame(|_, _| {});
        assert!(!cancelled.changed);
        assert!(cancelled.suppress_shortcuts);
        assert!(!fixture.editor.is_recording());
        assert_eq!(fixture.overrides, Overrides::default());
        fixture.context.io_mut().add_key_event(Key::Escape, false);
        fixture.frame(|_, _| {});
        fixture.editor.begin_recording("app.open", Some(0), 0);
        let assigned = fixture.click("assign_escape");
        assert!(assigned.changed);
        assert!(assigned.suppress_shortcuts);
        assert!(!fixture.editor.is_recording());
        assert_eq!(
            fixture.overrides.actions["app.open"][0]
                .shortcut
                .to_string(),
            "Escape"
        );
    }

    #[test]
    fn editing_and_restoring_actions_preserves_absent_plugins() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut fixture = Fixture::new();
        fixture
            .overrides
            .actions
            .insert("absent.plugin.action".into(), Vec::new());
        assert!(fixture.click("app.open/clear").changed);
        assert!(fixture.overrides.actions["app.open"].is_empty());
        assert!(fixture.click("app.open/defaults").changed);
        assert!(!fixture.overrides.actions.contains_key("app.open"));
        assert!(fixture.click("app.open/remove/0").changed);
        assert!(fixture.overrides.actions["app.open"].is_empty());
        let opening = fixture.click("app.open/add");
        assert!(opening.suppress_shortcuts);
        assert!(fixture.editor.is_recording());
        fixture.context.io_mut().add_key_event(Key::F12, true);
        assert!(fixture.frame(|_, _| {}).changed);
        fixture.context.io_mut().add_key_event(Key::F12, false);
        let replacing = fixture.click("app.open/binding/0");
        assert!(replacing.suppress_shortcuts);
        fixture.context.io_mut().add_key_event(Key::F11, true);
        assert!(fixture.frame(|_, _| {}).changed);
        assert_eq!(
            fixture.overrides.actions["app.open"][0]
                .shortcut
                .to_string(),
            "F11"
        );
        assert_eq!(
            fixture.overrides.actions["absent.plugin.action"],
            Vec::new()
        );
    }

    #[test]
    fn disappearing_actions_cancel_recording_without_creating_an_override() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut fixture = Fixture::new();
        fixture
            .editor
            .begin_recording("absent.plugin.action", None, 0);
        fixture.context.io_mut().add_key_event(Key::F12, true);
        let edit = fixture.frame(|_, _| {});
        assert!(!edit.changed);
        assert!(edit.suppress_shortcuts);
        assert!(!fixture.editor.is_recording());
        assert_eq!(fixture.overrides, Overrides::default());
    }
}
